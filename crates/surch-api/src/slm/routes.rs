//! Axum handlers for the ES-parity `_slm/policy/*` REST surface.
//!
//! Wire shape mirrors Elasticsearch 7.17 / OpenSearch 2.x so
//! `elasticsearch-py.slm.*` and the Kibana SLM UI keep working when
//! pointed at Surch.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use super::cron::CronSchedule;
use super::policy::{
    policy_to_es_json, SlmConfig, SlmExecutionState, SlmPolicy, SlmPolicyRegistry, SlmRetention,
};
use super::scheduler::execute_policy;
use crate::snapshot_es::service::SnapshotRepositoryRegistry;
use crate::state::AppState;
use crate::OpenSearchError;

/// Bundle of state every SLM handler reads. Cloning is `Arc`-cheap so
/// the axum `with_state` propagation stays free.
#[derive(Clone)]
pub struct SlmAppState {
    pub app: AppState,
    pub repositories: SnapshotRepositoryRegistry,
    pub policies: SlmPolicyRegistry,
}

/// Body of `PUT /_slm/policy/{id}`.
#[derive(Debug, Deserialize)]
pub struct PutSlmPolicyRequest {
    pub schedule: String,
    /// ES allows two shapes for the snapshot name pattern:
    /// - `"name": "<deces-{now/d}>"`
    /// - bare string without angle-brackets
    ///
    /// Both are accepted; the policy stores them as-is and the scheduler
    /// strips the brackets at fire time.
    #[serde(default)]
    pub name: Option<String>,
    pub repository: String,
    #[serde(default)]
    pub config: SlmConfigRequest,
    #[serde(default)]
    pub retention: Option<SlmRetention>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SlmConfigRequest {
    /// Accepts a `["a", "b"]` array, a `"a,b"` comma-string, or
    /// `"*"`. Stored normalised as a `Vec<String>`.
    #[serde(default)]
    pub indices: Option<Value>,
    #[serde(default)]
    pub include_global_state: bool,
}

fn err(status: StatusCode, kind: &str, reason: impl Into<String>) -> axum::response::Response {
    OpenSearchError::new(status, kind, reason).into_response()
}

fn parse_indices(value: Option<&Value>) -> Vec<String> {
    match value {
        None => Vec::new(),
        Some(Value::String(s)) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        Some(_) => Vec::new(),
    }
}

/// `PUT /_slm/policy/{id}`.
pub async fn put_policy_handler(
    State(state): State<SlmAppState>,
    Path(id): Path<String>,
    Json(body): Json<PutSlmPolicyRequest>,
) -> impl IntoResponse {
    if let Err(error) = CronSchedule::parse(&body.schedule) {
        return err(
            StatusCode::BAD_REQUEST,
            "illegal_argument_exception",
            format!("invalid schedule `{}`: {error}", body.schedule),
        );
    }
    let name_pattern = body
        .name
        .clone()
        .unwrap_or_else(|| format!("{id}-{{now/d}}"));
    let policy = SlmPolicy {
        name: id.clone(),
        schedule: body.schedule.clone(),
        name_pattern,
        repository: body.repository.clone(),
        config: SlmConfig {
            indices: parse_indices(body.config.indices.as_ref()),
            include_global_state: body.config.include_global_state,
        },
        retention: body.retention.clone(),
    };
    state.policies.upsert(policy.clone());

    // Seed `next_fire` so `GET /_slm/policy/{id}` returns a sensible
    // value before the scheduler's first tick.
    if let Ok(schedule) = CronSchedule::parse(&policy.schedule) {
        state
            .policies
            .set_next_fire(&policy.name, schedule.next_fire_after(Utc::now()));
    }

    Json(json!({ "acknowledged": true })).into_response()
}

/// `GET /_slm/policy`.
pub async fn list_policies_handler(State(state): State<SlmAppState>) -> impl IntoResponse {
    let mut body = serde_json::Map::new();
    for policy in state.policies.list() {
        let last = state.policies.last_execution(&policy.name);
        let next = state.policies.next_fire(&policy.name);
        body.insert(
            policy.name.clone(),
            policy_to_es_json(&policy, last.as_ref(), next),
        );
    }
    Json(Value::Object(body)).into_response()
}

/// `GET /_slm/policy/{id}`.
pub async fn get_policy_handler(
    State(state): State<SlmAppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.policies.get(&id) {
        Some(policy) => {
            let last = state.policies.last_execution(&id);
            let next = state.policies.next_fire(&id);
            Json(json!({
                id.clone(): policy_to_es_json(&policy, last.as_ref(), next),
            }))
            .into_response()
        }
        None => err(
            StatusCode::NOT_FOUND,
            "resource_not_found_exception",
            format!("snapshot lifecycle policy [{id}] not found"),
        ),
    }
}

/// `DELETE /_slm/policy/{id}`.
pub async fn delete_policy_handler(
    State(state): State<SlmAppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.policies.remove(&id) {
        Json(json!({ "acknowledged": true })).into_response()
    } else {
        err(
            StatusCode::NOT_FOUND,
            "resource_not_found_exception",
            format!("snapshot lifecycle policy [{id}] not found"),
        )
    }
}

/// `POST /_slm/policy/{id}/_execute`.
pub async fn execute_policy_handler(
    State(state): State<SlmAppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(policy) = state.policies.get(&id) else {
        return err(
            StatusCode::NOT_FOUND,
            "resource_not_found_exception",
            format!("snapshot lifecycle policy [{id}] not found"),
        );
    };
    let exec = execute_policy(
        &policy,
        Utc::now(),
        &state.repositories,
        &state.app,
        &state.policies,
    )
    .await;
    if exec.state == SlmExecutionState::Failed {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "snapshot_exception",
            exec.error.unwrap_or_else(|| "snapshot take failed".into()),
        );
    }
    Json(json!({ "snapshot_name": exec.snapshot_name })).into_response()
}

/// `GET /_slm/policy/{id}/_executions`.
pub async fn executions_handler(
    State(state): State<SlmAppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.policies.get(&id).is_none() {
        return err(
            StatusCode::NOT_FOUND,
            "resource_not_found_exception",
            format!("snapshot lifecycle policy [{id}] not found"),
        );
    }

    let executions = state
        .policies
        .executions(&id)
        .into_iter()
        .map(|e| {
            json!({
                "snapshot_name": e.snapshot_name,
                "state": e.state.as_str(),
                "start_time_millis": e.started_at_millis,
                "end_time_millis": e.finished_at_millis,
                "error": e.error,
            })
        })
        .collect::<Vec<_>>();

    Json(json!({ "executions": executions })).into_response()
}

/// `GET /_slm/status`.
pub async fn status_handler(State(state): State<SlmAppState>) -> impl IntoResponse {
    let count = state.policies.list().len();
    Json(json!({
        "operation_mode": "RUNNING",
        "policy_count": count,
    }))
    .into_response()
}
