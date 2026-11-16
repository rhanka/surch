//! OpenSearch-compatible `/_cluster/health` endpoint family.
//!
//! Surch is a single node holding every index whole: an index that exists
//! is open and fully searchable, i.e. `green`. The wait parameters follow
//! ES 8.6.1 `TransportClusterHealthAction`:
//!
//! - `wait_for_status=green|yellow|red` is satisfied at once by any
//!   existing target (green satisfies every level);
//! - a target that names a missing index can never satisfy the request:
//!   the call waits until `timeout` (default `30s`) for it to appear — a
//!   background `_restore` (`wait_for_completion=false`) makes its index
//!   appear atomically — then answers `status: red`, `timed_out: true`,
//!   HTTP 408 (`ClusterHealthResponse.status()`: `REQUEST_TIMEOUT` when
//!   timed out). `timeout=0` answers at once.

use std::time::{Duration, Instant};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::{index::validate_index_name, state::AppState, OpenSearchError};

const CLUSTER_NAME: &str = "surch-cluster";
/// ES `ClusterHealthRequest` default timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Poll period while waiting for a missing index.
const POLL: Duration = Duration::from_millis(50);

/// Query parameters of `_cluster/health` (others are accepted and ignored).
#[derive(Debug, Default, Deserialize)]
pub struct HealthParams {
    pub wait_for_status: Option<String>,
    pub timeout: Option<String>,
}

fn bad_request(reason: String) -> axum::response::Response {
    OpenSearchError::new(
        StatusCode::BAD_REQUEST,
        "illegal_argument_exception",
        reason,
    )
    .into_response()
}

/// ES `TimeValue.parseTimeValue`: `<n>(nanos|micros|ms|s|m|h|d)`, or a bare
/// `0` / `-1`.
pub fn parse_time_value(text: &str) -> Option<Duration> {
    let text = text.trim();
    if text == "0" || text == "-1" {
        return Some(Duration::ZERO);
    }
    let split = text.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (number, unit) = text.split_at(split);
    let value: f64 = number.parse().ok()?;
    let seconds = match unit {
        "nanos" => value / 1e9,
        "micros" => value / 1e6,
        "ms" => value / 1e3,
        "s" => value,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        "d" => value * 86_400.0,
        _ => return None,
    };
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

fn parse_params(params: &HealthParams) -> Result<Duration, String> {
    if let Some(status) = params.wait_for_status.as_deref() {
        if !matches!(status, "green" | "yellow" | "red") {
            return Err(format!("unknown cluster health status [{status}]"));
        }
    }
    match params.timeout.as_deref() {
        None => Ok(DEFAULT_TIMEOUT),
        Some(text) => parse_time_value(text).ok_or_else(|| {
            format!(
                "failed to parse setting [timeout] with value [{text}] as a time value: \
                 unit is missing or unrecognized"
            )
        }),
    }
}

/// Axum handler for `GET /_cluster/health`.
pub async fn cluster_health_handler(
    State(state): State<AppState>,
    Query(params): Query<HealthParams>,
) -> impl IntoResponse {
    if let Err(reason) = parse_params(&params) {
        return bad_request(reason);
    }
    let indices = state.index_names();
    let response = build_health_response(&state, &indices, None);
    (StatusCode::OK, Json(response)).into_response()
}

/// Resolve `target` (comma list of indices and aliases) to indices;
/// `None` while any part names nothing.
fn resolve_targets(state: &AppState, parts: &[&str]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for part in parts {
        let resolved = state.resolve_index(part);
        if resolved.is_empty() {
            return None;
        }
        for index in resolved {
            if !out.contains(&index) {
                out.push(index);
            }
        }
    }
    Some(out)
}

/// Axum handler for `GET /_cluster/health/{index}`.
pub async fn cluster_health_index_handler(
    State(state): State<AppState>,
    Path(target): Path<String>,
    Query(params): Query<HealthParams>,
) -> impl IntoResponse {
    let timeout = match parse_params(&params) {
        Ok(timeout) => timeout,
        Err(reason) => return bad_request(reason),
    };
    let parts: Vec<&str> = target
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    for part in &parts {
        if let Err(error) = validate_index_name(part) {
            return error.into_response();
        }
    }
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(indices) = resolve_targets(&state, &parts) {
            let response = build_health_response(&state, &indices, Some(target.as_str()));
            return (StatusCode::OK, Json(response)).into_response();
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL.min(deadline.saturating_duration_since(Instant::now()))).await;
    }
    // ES: a missing index is RED and the wait can never be satisfied.
    let mut response = build_health_response(&state, &[], Some(target.as_str()));
    response["status"] = json!("red");
    response["timed_out"] = json!(true);
    (StatusCode::REQUEST_TIMEOUT, Json(response)).into_response()
}

fn build_health_response(
    state: &AppState,
    indices: &[String],
    focus_index: Option<&str>,
) -> serde_json::Value {
    let shard_count = indices.len() as u64;
    let mut response = json!({
        "cluster_name": CLUSTER_NAME,
        "status": "green",
        "timed_out": false,
        "number_of_nodes": 1,
        "number_of_data_nodes": 1,
        "active_primary_shards": shard_count,
        "active_shards": shard_count,
        "relocating_shards": 0,
        "initializing_shards": 0,
        "unassigned_shards": 0,
        "delayed_unassigned_shards": 0,
        "number_of_pending_tasks": 0,
        "number_of_in_flight_fetch": 0,
        "task_max_waiting_in_queue_millis": 0,
        "active_shards_percent_as_number": 100.0,
    });

    if focus_index.is_some() {
        let object = response
            .as_object_mut()
            .expect("health response is an object");
        let mut indices_map = serde_json::Map::new();
        for index in indices {
            indices_map.insert(index.clone(), index_health_entry(state, index));
        }
        object.insert("indices".to_owned(), serde_json::Value::Object(indices_map));
    }

    response
}

/// Declared `number_of_replicas` (flat or under `index`), echoed like ES;
/// replicas are never allocated on a single node, so `0` when absent.
fn declared_replicas(state: &AppState, index: &str) -> u64 {
    let Some(metadata) = state.index_metadata(index) else {
        return 0;
    };
    let settings = &metadata.settings;
    let value = settings
        .get("number_of_replicas")
        .or_else(|| settings.get("index")?.get("number_of_replicas"))
        .or_else(|| settings.get("index.number_of_replicas"));
    match value {
        Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn index_health_entry(state: &AppState, index: &str) -> serde_json::Value {
    json!({
        "status": "green",
        "number_of_shards": 1,
        "number_of_replicas": declared_replicas(state, index),
        "active_primary_shards": 1,
        "active_shards": 1,
        "relocating_shards": 0,
        "initializing_shards": 0,
        "unassigned_shards": 0,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_time_value;
    use std::time::Duration;

    #[test]
    fn time_values_parse_like_es() {
        assert_eq!(parse_time_value("600s"), Some(Duration::from_secs(600)));
        assert_eq!(parse_time_value("1s"), Some(Duration::from_secs(1)));
        assert_eq!(parse_time_value("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_time_value("2m"), Some(Duration::from_secs(120)));
        assert_eq!(parse_time_value("0"), Some(Duration::ZERO));
        assert_eq!(parse_time_value("10"), None);
        assert_eq!(parse_time_value("abc"), None);
    }
}
