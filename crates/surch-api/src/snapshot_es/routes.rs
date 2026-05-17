//! Axum handlers for the ES-parity `_snapshot` REST surface.
//!
//! Wire shape mirrors the Elasticsearch 7.17 / OpenSearch 2.x JSON
//! responses verbatim so `elasticsearch-py.snapshot.*`, Curator and the
//! Kibana snapshot-and-restore UI all keep working unchanged when
//! pointed at Surch. The body of each response is built explicitly
//! rather than via `serde` flattening, so the surface contract is
//! readable in one place and trivially diffable against the upstream
//! reference.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::repository::{FsRepository, S3Repository, S3RepositoryConfig};
use super::service::{
    self, RegisteredRepository, SnapshotRepositoryRegistry, SnapshotServiceError,
};
use crate::{state::AppState, OpenSearchError};

/// Body of `PUT /_snapshot/{repository}`.
#[derive(Debug, Deserialize)]
pub struct RegisterRepositoryRequest {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub settings: Value,
}

/// Body of `PUT /_snapshot/{repository}/{snapshot}`.
#[derive(Debug, Default, Deserialize)]
pub struct CreateSnapshotRequest {
    #[serde(default)]
    pub indices: Option<Value>,
    #[serde(default)]
    pub include_global_state: bool,
}

/// Body of `POST /_snapshot/{repository}/{snapshot}/_restore`.
#[derive(Debug, Default, Deserialize)]
pub struct RestoreRequest {
    #[serde(default)]
    pub indices: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WaitForCompletionParams {
    /// Reserved for the future async-take path — today the take is
    /// synchronous so the param is accepted-and-ignored to stay
    /// surface-compatible with the ES clients that always set it.
    #[allow(dead_code)]
    pub wait_for_completion: Option<bool>,
}

fn err(status: StatusCode, kind: &str, reason: impl Into<String>) -> axum::response::Response {
    OpenSearchError::new(status, kind, reason).into_response()
}

fn from_service_error(error: SnapshotServiceError) -> axum::response::Response {
    use SnapshotServiceError::*;
    match error {
        RepositoryMissing { ref name } => err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{name}] missing"),
        ),
        SnapshotMissing { ref repo, ref snap } => err(
            StatusCode::NOT_FOUND,
            "snapshot_missing_exception",
            format!("snapshot [{repo}:{snap}] missing"),
        ),
        SnapshotAlreadyExists { ref repo, ref snap } => err(
            StatusCode::BAD_REQUEST,
            "invalid_snapshot_name_exception",
            format!("snapshot [{repo}:{snap}] already exists"),
        ),
        UnsupportedRepositoryType { ref kind } => err(
            StatusCode::BAD_REQUEST,
            "repository_exception",
            format!("repository type [{kind}] is not supported (only `fs` and `s3` are wired in)"),
        ),
        BadRequest(message) => err(
            StatusCode::BAD_REQUEST,
            "snapshot_exception",
            message,
        ),
        IndexMissing { ref name } => err(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{name}] missing"),
        ),
        Repository(repo_err) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "repository_exception",
            repo_err.to_string(),
        ),
        Serde(message) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "snapshot_exception",
            message,
        ),
    }
}

/// Bundle the two pieces of state every snapshot handler needs.
#[derive(Clone)]
pub struct SnapshotAppState {
    pub app: AppState,
    pub registry: SnapshotRepositoryRegistry,
}

fn repository_metadata_response(name: &str, repo: &dyn super::SnapshotRepository) -> Value {
    json!({
        name: {
            "type": repo.kind(),
            "settings": repo.settings(),
        }
    })
}

fn parse_indices_value(value: Option<&Value>) -> Vec<String> {
    match value {
        None => Vec::new(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|s| s.trim())
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

/// `PUT /_snapshot/{repository}`.
pub async fn register_repository_handler(
    State(state): State<SnapshotAppState>,
    Path(repo_name): Path<String>,
    Json(body): Json<RegisterRepositoryRequest>,
) -> impl IntoResponse {
    match body.kind.as_str() {
        "fs" => {
            let location = match body
                .settings
                .get("location")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                Some(s) if !s.is_empty() => s,
                _ => {
                    return err(
                        StatusCode::BAD_REQUEST,
                        "repository_exception",
                        "fs repository requires a non-empty `settings.location` path",
                    );
                }
            };
            let fs_repo = match FsRepository::new(location.into()) {
                Ok(repo) => repo,
                Err(error) => {
                    return err(
                        StatusCode::BAD_REQUEST,
                        "repository_exception",
                        error.to_string(),
                    );
                }
            };
            state.registry.insert(repo_name.clone(), Arc::new(fs_repo));
            Json(json!({ "acknowledged": true })).into_response()
        }
        "s3" => {
            // Strict static-credentials shape (IAM role chaining,
            // STS, IMDS are intentionally out of scope for the MVP).
            let bucket = match body
                .settings
                .get("bucket")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                Some(s) if !s.is_empty() => s,
                _ => {
                    return err(
                        StatusCode::BAD_REQUEST,
                        "repository_exception",
                        "s3 repository requires a non-empty `settings.bucket` string",
                    );
                }
            };
            let config = S3RepositoryConfig {
                bucket,
                region: body
                    .settings
                    .get("region")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                endpoint: body
                    .settings
                    .get("endpoint")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                access_key: body
                    .settings
                    .get("access_key")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                secret_key: body
                    .settings
                    .get("secret_key")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                session_token: body
                    .settings
                    .get("session_token")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                base_path: body
                    .settings
                    .get("base_path")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            // The S3 client is built eagerly inside `S3Repository::new`
            // so misconfiguration surfaces here (and not on the first
            // take). Construction itself touches a single-thread
            // runtime — keep it off the async axum worker via
            // `spawn_blocking` so we don't get a "Cannot start a runtime
            // from within a runtime" panic.
            let s3_repo = match tokio::task::spawn_blocking(move || S3Repository::new(config))
                .await
                .map_err(|join_err| {
                    err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "repository_exception",
                        format!("s3 repository init task panicked: {join_err}"),
                    )
                }) {
                Ok(Ok(repo)) => repo,
                Ok(Err(error)) => {
                    return err(
                        StatusCode::BAD_REQUEST,
                        "repository_exception",
                        error.to_string(),
                    );
                }
                Err(resp) => return resp,
            };
            state.registry.insert(repo_name.clone(), Arc::new(s3_repo));
            Json(json!({ "acknowledged": true })).into_response()
        }
        other => from_service_error(SnapshotServiceError::UnsupportedRepositoryType {
            kind: other.to_owned(),
        }),
    }
}

/// `GET /_snapshot` and `GET /_snapshot/{repository}`.
pub async fn list_repositories_handler(
    State(state): State<SnapshotAppState>,
) -> impl IntoResponse {
    let mut body = serde_json::Map::new();
    for (name, repo) in state.registry.list() {
        if let Some(Value::Object(map)) =
            repository_metadata_response(&name, repo.as_ref()).as_object().cloned().map(Value::Object).as_ref()
        {
            for (k, v) in map {
                body.insert(k.clone(), v.clone());
            }
        }
    }
    Json(Value::Object(body)).into_response()
}

/// `GET /_snapshot/{repository}`.
pub async fn get_repository_handler(
    State(state): State<SnapshotAppState>,
    Path(repo_name): Path<String>,
) -> impl IntoResponse {
    match state.registry.get(&repo_name) {
        Some(repo) => Json(repository_metadata_response(&repo_name, repo.as_ref())).into_response(),
        None => err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        ),
    }
}

/// `DELETE /_snapshot/{repository}`.
pub async fn delete_repository_handler(
    State(state): State<SnapshotAppState>,
    Path(repo_name): Path<String>,
) -> impl IntoResponse {
    if state.registry.remove(&repo_name) {
        Json(json!({ "acknowledged": true })).into_response()
    } else {
        err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        )
    }
}

/// `PUT /_snapshot/{repository}/{snapshot}`.
pub async fn create_snapshot_handler(
    State(state): State<SnapshotAppState>,
    Path((repo_name, snap_name)): Path<(String, String)>,
    Query(_params): Query<WaitForCompletionParams>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let request: CreateSnapshotRequest = if body.is_empty() {
        CreateSnapshotRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(parsed) => parsed,
            Err(error) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "parse_exception",
                    format!("invalid snapshot request body: {error}"),
                );
            }
        }
    };

    let Some(repo) = state.registry.get(&repo_name) else {
        return err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        );
    };

    let indices = parse_indices_value(request.indices.as_ref());
    match service::create_snapshot(
        repo.as_ref(),
        &repo_name,
        &snap_name,
        &indices,
        request.include_global_state,
        &state.app,
    ) {
        Ok(entry) => Json(json!({
            "snapshot": {
                "snapshot": entry.name,
                "uuid": entry.uuid,
                "state": entry.state.as_str(),
                "indices": entry.indices,
                "start_time_in_millis": entry.start_time_millis,
                "end_time_in_millis": entry.end_time_millis,
                "include_global_state": request.include_global_state,
            }
        }))
        .into_response(),
        Err(error) => from_service_error(error),
    }
}

/// `GET /_snapshot/{repository}/{snapshot}`.
pub async fn get_snapshot_handler(
    State(state): State<SnapshotAppState>,
    Path((repo_name, snap_name)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(repo) = state.registry.get(&repo_name) else {
        return err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        );
    };
    match service::get_snapshot(repo.as_ref(), &repo_name, &snap_name) {
        Ok(entry) => Json(json!({
            "snapshots": [{
                "snapshot": entry.name,
                "uuid": entry.uuid,
                "state": entry.state.as_str(),
                "indices": entry.indices,
                "start_time_in_millis": entry.start_time_millis,
                "end_time_in_millis": entry.end_time_millis,
            }]
        }))
        .into_response(),
        Err(error) => from_service_error(error),
    }
}

/// `DELETE /_snapshot/{repository}/{snapshot}`.
pub async fn delete_snapshot_handler(
    State(state): State<SnapshotAppState>,
    Path((repo_name, snap_name)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(repo) = state.registry.get(&repo_name) else {
        return err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        );
    };
    match service::delete_snapshot(repo.as_ref(), &repo_name, &snap_name) {
        Ok(()) => Json(json!({ "acknowledged": true })).into_response(),
        Err(error) => from_service_error(error),
    }
}

/// `POST /_snapshot/{repository}/{snapshot}/_restore`.
pub async fn restore_snapshot_handler(
    State(state): State<SnapshotAppState>,
    Path((repo_name, snap_name)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let request: RestoreRequest = if body.is_empty() {
        RestoreRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(parsed) => parsed,
            Err(error) => {
                return err(
                    StatusCode::BAD_REQUEST,
                    "parse_exception",
                    format!("invalid restore request body: {error}"),
                );
            }
        }
    };
    let Some(repo) = state.registry.get(&repo_name) else {
        return err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        );
    };
    let indices = parse_indices_value(request.indices.as_ref());
    let selector = if indices.is_empty() { None } else { Some(indices.as_slice()) };
    match service::restore_snapshot(repo.as_ref(), &repo_name, &snap_name, selector, &state.app) {
        Ok(restored) => Json(json!({
            "snapshot": {
                "snapshot": snap_name,
                "indices": restored,
                "shards": {
                    "total": 0,
                    "failed": 0,
                    "successful": 0,
                }
            }
        }))
        .into_response(),
        Err(error) => from_service_error(error),
    }
}

// Silence the dead-code lint on `RegisteredRepository` when only the
// constructor branch is exercised by the public API surface.
#[allow(dead_code)]
fn _force_registered_repository_alive(_r: RegisteredRepository) {}
