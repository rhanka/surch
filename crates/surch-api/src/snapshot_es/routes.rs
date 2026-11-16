//! Axum handlers for the ES-parity `_snapshot` REST surface.
//!
//! Wire shape mirrors the Elasticsearch 7.17 / OpenSearch 2.x JSON
//! responses verbatim so `elasticsearch-py.snapshot.*`, Curator and the
//! Kibana snapshot-and-restore UI all keep working unchanged when
//! pointed at Surch. The body of each response is built explicitly
//! rather than via `serde` flattening, so the surface contract is
//! readable in one place and trivially diffable against the upstream
//! reference.
//!
//! Snapshot take, delete and restore move whole index directories (GiB at
//! the matchID scale): they run on the blocking pool, never on an async
//! worker.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::repository::{
    echo_settings, parse_byte_size, setting_bool, FsRepository, S3Repository, S3RepositoryConfig,
};
use super::service::{
    self, flatten_index_settings, normalize_index_key, RegisteredRepository, RestoreOptions,
    SnapshotRepositoryRegistry, SnapshotServiceError,
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

/// Body of `POST /_snapshot/{repository}/{snapshot}/_restore` (ES
/// `RestoreSnapshotRequest.source`). Booleans are accepted as JSON
/// booleans or `"true"` / `"false"` strings, as ES does.
#[derive(Debug, Default, Deserialize)]
pub struct RestoreRequest {
    #[serde(default)]
    pub indices: Option<Value>,
    #[serde(default)]
    pub ignore_unavailable: Option<Value>,
    #[serde(default)]
    pub include_global_state: Option<Value>,
    #[serde(default)]
    pub include_aliases: Option<Value>,
    #[serde(default)]
    pub partial: Option<Value>,
    #[serde(default)]
    pub rename_pattern: Option<String>,
    #[serde(default)]
    pub rename_replacement: Option<String>,
    #[serde(default)]
    pub index_settings: Option<Value>,
    #[serde(default)]
    pub ignore_index_settings: Option<Value>,
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
        BadRequest(message) => err(StatusCode::BAD_REQUEST, "snapshot_exception", message),
        IndexMissing { ref name } => err(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{name}] missing"),
        ),
        NoSuchIndex { ref name } => err(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("no such index [{name}]"),
        ),
        // ES `RepositoryException` / `SnapshotRestoreException` carry no
        // REST status of their own: 500.
        ref error @ ReadonlyRepository { .. } => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "repository_exception",
            error.to_string(),
        ),
        ref error @ Restore { .. } => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "snapshot_restore_exception",
            error.to_string(),
        ),
        ref error @ (Integrity(_) | DataDir(_)) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "snapshot_exception",
            error.to_string(),
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

fn join_error(error: tokio::task::JoinError) -> axum::response::Response {
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "snapshot_exception",
        format!("snapshot task failed: {error}"),
    )
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

/// ES lenient boolean: JSON boolean or `"true"` / `"false"`.
fn parse_bool(value: Option<&Value>, name: &str, default: bool) -> Result<bool, String> {
    match value {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) if s == "true" => Ok(true),
        Some(Value::String(s)) if s == "false" => Ok(false),
        Some(other) => Err(format!(
            "Failed to parse value [{}] as only [true] or [false] are allowed for [{name}]",
            other
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| other.to_string())
        )),
    }
}

fn str_setting(settings: &Value, key: &str) -> Option<String> {
    match settings.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// `settings` of an `s3` repository -> client configuration (ES
/// `repository-s3` names: `bucket`, `region`, `endpoint`, `protocol`,
/// `path_style_access`, `client`, `base_path`, `readonly`, `buffer_size`).
pub fn s3_config_from_settings(settings: &Value) -> Result<S3RepositoryConfig, String> {
    let bucket = match settings.get("bucket").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s.to_owned(),
        _ => return Err("s3 repository requires a non-empty `settings.bucket` string".to_owned()),
    };
    let bool_of = |key: &str| setting_bool(settings, key).map_err(|error| error.to_string());
    let endpoint = str_setting(settings, "endpoint");
    let buffer_size = match settings.get("buffer_size") {
        None | Some(Value::Null) => 0,
        Some(Value::Number(n)) => n
            .as_u64()
            .ok_or_else(|| format!("invalid s3 `buffer_size` [{n}]"))?,
        Some(Value::String(s)) => {
            parse_byte_size(s).ok_or_else(|| format!("invalid s3 `buffer_size` [{s}]"))?
        }
        Some(other) => return Err(format!("invalid s3 `buffer_size` [{other}]")),
    };
    // Flexible Checksums (CRC trailers on PUT) are an AWS extension that
    // several S3-compatible stores reject; with a custom endpoint the SDK
    // only checksums when the API requires it, unless the operator sets
    // `disable_request_checksum: false`. Integrity of snapshot blobs is
    // enforced end to end by blake3 anyway (`segments.rs`).
    let disable_request_checksum = match settings
        .get("disable_request_checksum")
        .or_else(|| settings.get("_disable_request_checksum"))
    {
        None | Some(Value::Null) => endpoint.is_some(),
        Some(_) => bool_of("disable_request_checksum")?
            .or(bool_of("_disable_request_checksum")?)
            .unwrap_or(false),
    };
    Ok(S3RepositoryConfig {
        bucket,
        region: str_setting(settings, "region"),
        endpoint,
        access_key: str_setting(settings, "access_key"),
        secret_key: str_setting(settings, "secret_key"),
        session_token: str_setting(settings, "session_token"),
        base_path: str_setting(settings, "base_path"),
        disable_request_checksum,
        path_style_access: bool_of("path_style_access")?,
        protocol: str_setting(settings, "protocol"),
        client: str_setting(settings, "client"),
        readonly: bool_of("readonly")?.unwrap_or(false),
        buffer_size,
        echo: Some(echo_settings(settings)),
    })
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
            let readonly = match setting_bool(&body.settings, "readonly") {
                Ok(value) => value.unwrap_or(false),
                Err(error) => {
                    return err(
                        StatusCode::BAD_REQUEST,
                        "repository_exception",
                        error.to_string(),
                    )
                }
            };
            let fs_repo = match FsRepository::new(location.into()) {
                Ok(repo) => repo.with_readonly(readonly),
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
            let config = match s3_config_from_settings(&body.settings) {
                Ok(config) => config,
                Err(reason) => return err(StatusCode::BAD_REQUEST, "repository_exception", reason),
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
pub async fn list_repositories_handler(State(state): State<SnapshotAppState>) -> impl IntoResponse {
    let mut body = serde_json::Map::new();
    for (name, repo) in state.registry.list() {
        if let Value::Object(map) = repository_metadata_response(&name, repo.as_ref()) {
            body.extend(map);
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
    let include_global_state = request.include_global_state;
    let app = state.app.clone();
    let task_repo_name = repo_name.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        service::create_snapshot_with_report(
            repo.as_ref(),
            &task_repo_name,
            &snap_name,
            &indices,
            include_global_state,
            &app,
        )
    })
    .await;
    match outcome {
        Ok(Ok((entry, report))) => Json(json!({
            "snapshot": {
                "snapshot": entry.name,
                "uuid": entry.uuid,
                "state": entry.state.as_str(),
                "indices": entry.indices,
                "start_time_in_millis": entry.start_time_millis,
                "end_time_in_millis": entry.end_time_millis,
                "include_global_state": include_global_state,
                "surch": report,
            }
        }))
        .into_response(),
        Ok(Err(error)) => from_service_error(error),
        Err(error) => join_error(error),
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
    let entries = if snap_name == "_all" {
        service::list_snapshots(repo.as_ref())
    } else {
        service::get_snapshot(repo.as_ref(), &repo_name, &snap_name).map(|entry| vec![entry])
    };
    match entries {
        Ok(entries) => Json(json!({
            "snapshots": entries.into_iter().map(|entry| json!({
                "snapshot": entry.name,
                "uuid": entry.uuid,
                "state": entry.state.as_str(),
                "indices": entry.indices,
                "start_time_in_millis": entry.start_time_millis,
                "end_time_in_millis": entry.end_time_millis,
            })).collect::<Vec<_>>()
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
    let outcome = tokio::task::spawn_blocking(move || {
        service::delete_snapshot(repo.as_ref(), &repo_name, &snap_name)
    })
    .await;
    match outcome {
        Ok(Ok(())) => Json(json!({ "acknowledged": true })).into_response(),
        Ok(Err(error)) => from_service_error(error),
        Err(error) => join_error(error),
    }
}

/// Parse a `_restore` body into [`RestoreOptions`].
pub fn parse_restore_request(request: &RestoreRequest) -> Result<RestoreOptions, String> {
    let index_settings = match &request.index_settings {
        None => Default::default(),
        Some(value) => flatten_index_settings(value)?,
    };
    let ignore_index_settings = parse_indices_value(request.ignore_index_settings.as_ref())
        .iter()
        .map(|key| normalize_index_key(key))
        .collect();
    Ok(RestoreOptions {
        indices: parse_indices_value(request.indices.as_ref()),
        ignore_unavailable: parse_bool(
            request.ignore_unavailable.as_ref(),
            "ignore_unavailable",
            false,
        )?,
        include_global_state: parse_bool(
            request.include_global_state.as_ref(),
            "include_global_state",
            false,
        )?,
        include_aliases: parse_bool(request.include_aliases.as_ref(), "include_aliases", true)?,
        partial: parse_bool(request.partial.as_ref(), "partial", false)?,
        rename_pattern: request.rename_pattern.clone(),
        rename_replacement: request.rename_replacement.clone(),
        index_settings,
        ignore_index_settings,
    })
}

/// `wait_for_completion` query flag: `None` when absent, ES boolean
/// parsing otherwise (`?wait_for_completion` alone = `true`).
fn wait_for_completion(params: &HashMap<String, String>) -> Result<Option<bool>, String> {
    match params.get("wait_for_completion").map(String::as_str) {
        None => Ok(None),
        Some("" | "true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(other) => Err(format!(
            "Failed to parse value [{other}] as only [true] or [false] are allowed."
        )),
    }
}

/// `POST /_snapshot/{repository}/{snapshot}/_restore`.
///
/// `wait_for_completion=true` (and, unlike ES, an absent flag — the
/// historical Surch behaviour, kept so existing callers do not change)
/// answers once every index is live with the ES `RestoreInfo` shape.
/// `wait_for_completion=false` validates the request, answers
/// `{"accepted": true}` and restores in the background; the restored
/// index appears atomically when ready (`_cluster/health/<idx>
/// ?wait_for_status=green` waits for it).
pub async fn restore_snapshot_handler(
    State(state): State<SnapshotAppState>,
    Path((repo_name, snap_name)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
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
    let options = match parse_restore_request(&request) {
        Ok(options) => options,
        Err(reason) => {
            return err(
                StatusCode::BAD_REQUEST,
                "illegal_argument_exception",
                reason,
            )
        }
    };
    let wait = match wait_for_completion(&params) {
        Ok(wait) => wait.unwrap_or(true),
        Err(reason) => {
            return err(
                StatusCode::BAD_REQUEST,
                "illegal_argument_exception",
                reason,
            )
        }
    };
    let Some(repo) = state.registry.get(&repo_name) else {
        return err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        );
    };
    let app = state.app.clone();
    let plan = {
        let (repo, app, repo_name, snap_name) = (
            Arc::clone(&repo),
            app.clone(),
            repo_name.clone(),
            snap_name.clone(),
        );
        tokio::task::spawn_blocking(move || {
            service::plan_restore(repo.as_ref(), &repo_name, &snap_name, &options, &app)
        })
        .await
    };
    let plan = match plan {
        Ok(Ok(plan)) => plan,
        Ok(Err(error)) => return from_service_error(error),
        Err(error) => return join_error(error),
    };
    if !wait {
        let indices = plan.restored_indices();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = service::execute_restore(repo.as_ref(), &plan, &app) {
                tracing::error!(target: "surch_api::snapshot", repository = %repo_name,
                    snapshot = %snap_name, ?indices, %error, "background restore failed");
            }
        });
        return Json(json!({ "accepted": true })).into_response();
    }
    match tokio::task::spawn_blocking(move || service::execute_restore(repo.as_ref(), &plan, &app))
        .await
    {
        Ok(Ok(outcome)) => {
            let total = outcome.indices.len();
            Json(json!({
                "snapshot": {
                    "snapshot": outcome.snapshot,
                    "indices": outcome.indices,
                    "shards": {
                        "total": total,
                        "failed": 0,
                        "successful": total,
                    }
                }
            }))
            .into_response()
        }
        Ok(Err(error)) => from_service_error(error),
        Err(error) => join_error(error),
    }
}

pub async fn verify_repository_handler(
    State(state): State<SnapshotAppState>,
    Path(repo_name): Path<String>,
) -> impl IntoResponse {
    let Some(repo) = state.registry.get(&repo_name) else {
        return err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        );
    };
    let probe_key = format!("verify-{}.dat", now_nanos());
    let probe = b"surch-snapshot-verify".as_slice();
    if let Err(error) = repo.put_object(&probe_key, axum::body::Bytes::from_static(probe)) {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "repository_verification_exception",
            format!("repository [{repo_name}] write probe failed: {error}"),
        );
    }
    let echo = match repo.get_object(&probe_key) {
        Ok(bytes) => bytes,
        Err(error) => {
            // Best-effort cleanup on the failure path.
            let _ = repo.delete_object(&probe_key);
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "repository_verification_exception",
                format!("repository [{repo_name}] read probe failed: {error}"),
            );
        }
    };
    let _ = repo.delete_object(&probe_key);
    if echo.as_ref() != probe {
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "repository_verification_exception",
            format!("repository [{repo_name}] probe contents mismatched"),
        );
    }
    Json(json!({
        "nodes": {
            "local": { "name": "surch" }
        }
    }))
    .into_response()
}

/// Build the per-snapshot ES `_status` envelope from a `SnapshotEntry`.
///
/// Surch takes are synchronous: a `SUCCESS` entry maps to a `done == total`
/// shard summary with one logical shard per snapshotted index.
fn snapshot_status_envelope(repo: &str, entry: &super::SnapshotEntry) -> Value {
    let total = entry.indices.len().max(1) as u64;
    let done = if matches!(entry.state, super::SnapshotState::Success) {
        total
    } else {
        0
    };
    let failed = match entry.state {
        super::SnapshotState::Failed => total,
        _ => 0,
    };
    let duration_ms = entry
        .end_time_millis
        .saturating_sub(entry.start_time_millis)
        .max(0);
    let mut indices = serde_json::Map::new();
    for name in &entry.indices {
        indices.insert(
            name.clone(),
            json!({
                "shards_stats": {
                    "initializing": 0,
                    "started": 0,
                    "finalizing": 0,
                    "done": if matches!(entry.state, super::SnapshotState::Success) { 1 } else { 0 },
                    "failed": if matches!(entry.state, super::SnapshotState::Failed) { 1 } else { 0 },
                    "total": 1,
                },
                "stats": {
                    "incremental": { "file_count": 0, "size_in_bytes": 0 },
                    "total":       { "file_count": 0, "size_in_bytes": 0 },
                    "start_time_in_millis": entry.start_time_millis,
                    "time_in_millis": duration_ms,
                },
                "shards": {}
            }),
        );
    }
    json!({
        "snapshot": entry.name,
        "repository": repo,
        "uuid": entry.uuid,
        "state": entry.state.as_str(),
        "include_global_state": false,
        "shards_stats": {
            "initializing": 0,
            "started": 0,
            "finalizing": 0,
            "done": done,
            "failed": failed,
            "total": total,
        },
        "stats": {
            "incremental": { "file_count": 0, "size_in_bytes": 0 },
            "total":       { "file_count": 0, "size_in_bytes": 0 },
            "start_time_in_millis": entry.start_time_millis,
            "time_in_millis": duration_ms,
        },
        "indices": Value::Object(indices),
    })
}

/// `GET /_snapshot/_status`.
///
/// ES returns only currently running snapshots. Surch takes are
/// synchronous so this is always the empty envelope; clients that poll
/// here are kept compatible without paying the cost of walking every
/// repository.
pub async fn snapshot_status_all_handler(
    State(_state): State<SnapshotAppState>,
) -> impl IntoResponse {
    Json(json!({ "snapshots": [] })).into_response()
}

/// `GET /_snapshot/{repository}/_status`.
///
/// Same semantics as `_snapshot/_status` scoped to one repository — only
/// running snapshots, which is always empty for Surch's synchronous
/// take. Returns 404 if the repository name is unknown.
pub async fn snapshot_status_repo_handler(
    State(state): State<SnapshotAppState>,
    Path(repo_name): Path<String>,
) -> impl IntoResponse {
    if state.registry.get(&repo_name).is_none() {
        return err(
            StatusCode::NOT_FOUND,
            "repository_missing_exception",
            format!("repository [{repo_name}] missing"),
        );
    }
    Json(json!({ "snapshots": [] })).into_response()
}

/// `GET /_snapshot/{repository}/{snapshot}/_status`.
pub async fn snapshot_status_one_handler(
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
    let entries = if snap_name == "_all" {
        service::list_snapshots(repo.as_ref())
    } else {
        service::get_snapshot(repo.as_ref(), &repo_name, &snap_name).map(|entry| vec![entry])
    };
    match entries {
        Ok(entries) => Json(json!({
            "snapshots": entries
                .iter()
                .map(|e| snapshot_status_envelope(&repo_name, e))
                .collect::<Vec<_>>()
        }))
        .into_response(),
        Err(error) => from_service_error(error),
    }
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

// Silence the dead-code lint on `RegisteredRepository` when only the
// constructor branch is exercised by the public API surface.
#[allow(dead_code)]
fn _force_registered_repository_alive(_r: RegisteredRepository) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The repository body of the matchID prod restore Job.
    #[test]
    fn matchid_job_s3_settings_map_to_the_client_config() {
        let settings = json!({
            "bucket": "fichier-des-personnes-decedees-elasticsearch",
            "client": "default",
            "region": "fr-par",
            "endpoint": "s3.fr-par.scw.cloud",
            "path_style_access": true,
            "protocol": "https",
            "readonly": true
        });
        let config = s3_config_from_settings(&settings).unwrap();
        assert_eq!(
            config.bucket,
            "fichier-des-personnes-decedees-elasticsearch"
        );
        assert_eq!(config.region.as_deref(), Some("fr-par"));
        assert_eq!(config.client.as_deref(), Some("default"));
        assert!(config.readonly);
        assert_eq!(config.path_style_access, Some(true));
        assert!(
            config.disable_request_checksum,
            "custom endpoint: checksums when required"
        );
        assert_eq!(
            config.endpoint_url().unwrap().as_deref(),
            Some("https://s3.fr-par.scw.cloud")
        );
        assert_eq!(
            config.echo.as_ref().unwrap()["path_style_access"],
            json!("true")
        );
        // ES string booleans and byte sizes.
        let config = s3_config_from_settings(&json!({
            "bucket": "b", "readonly": "false", "buffer_size": "16mb"
        }))
        .unwrap();
        assert!(!config.readonly);
        assert_eq!(config.buffer_size, 16 << 20);
        assert!(
            !config.disable_request_checksum,
            "AWS endpoint keeps full checksums"
        );
        assert!(s3_config_from_settings(&json!({"bucket": "b", "readonly": "maybe"})).is_err());
        assert!(s3_config_from_settings(&json!({"region": "x"})).is_err());
    }

    #[test]
    fn restore_body_of_the_job_parses() {
        let request: RestoreRequest = serde_json::from_value(json!({
            "indices": "deces",
            "ignore_unavailable": true,
            "include_global_state": false,
            "rename_pattern": "^deces$",
            "rename_replacement": "deces-snap",
            "index_settings": {"index.number_of_replicas": 0}
        }))
        .unwrap();
        let options = parse_restore_request(&request).unwrap();
        assert_eq!(options.indices, vec!["deces".to_owned()]);
        assert!(options.ignore_unavailable);
        assert!(!options.include_global_state);
        assert!(options.include_aliases);
        assert_eq!(options.rename_pattern.as_deref(), Some("^deces$"));
        assert_eq!(
            options.index_settings.get("index.number_of_replicas"),
            Some(&json!(0))
        );
        let bad: RestoreRequest =
            serde_json::from_value(json!({"include_aliases": "nope"})).unwrap();
        assert!(parse_restore_request(&bad).is_err());
    }

    #[test]
    fn wait_for_completion_flag_parses_like_es() {
        let params = |v: &str| HashMap::from([("wait_for_completion".to_owned(), v.to_owned())]);
        assert_eq!(wait_for_completion(&HashMap::new()), Ok(None));
        assert_eq!(wait_for_completion(&params("true")), Ok(Some(true)));
        assert_eq!(wait_for_completion(&params("")), Ok(Some(true)));
        assert_eq!(wait_for_completion(&params("false")), Ok(Some(false)));
        assert!(wait_for_completion(&params("1")).is_err());
    }
}
