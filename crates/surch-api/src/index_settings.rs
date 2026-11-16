//! `GET` / `PUT /<index>/_settings` (L6.2: the matchID restore Job sets
//! `index.number_of_replicas: 0` on the restored index with `curl -f`, so
//! the route must exist and acknowledge).
//!
//! Surch is a single node: replica and allocation settings have no effect
//! besides being stored and echoed (`GET /<index>/_settings`,
//! `_cluster/health/<index>` `number_of_replicas`). Static settings are
//! refused on an open index as ES 8.6.1 does
//! (`MetadataUpdateSettingsService`: "Can't update non dynamic settings").

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Map, Value};

use crate::{
    index::validate_index_name,
    snapshot_es::service::{apply_index_settings, flatten_index_settings},
    state::AppState,
    OpenSearchError,
};

/// Settings ES fixes at index creation (`IndexMetadata` / analysis): a
/// `PUT _settings` on an open index naming one of them is refused.
const STATIC_SETTINGS: &[&str] = &[
    "index.number_of_shards",
    "index.number_of_routing_shards",
    "index.codec",
    "index.version.created",
    "index.uuid",
    "index.creation_date",
    "index.provided_name",
    "index.history.uuid",
];

fn is_static(key: &str) -> bool {
    STATIC_SETTINGS.contains(&key) || key.starts_with("index.analysis.")
}

fn missing(target: &str) -> axum::response::Response {
    OpenSearchError::new(
        StatusCode::NOT_FOUND,
        "index_not_found_exception",
        format!("no such index [{target}]"),
    )
    .into_response()
}

/// `GET /<index>/_settings`.
pub async fn get_settings_handler(
    State(state): State<AppState>,
    Path(target): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&target) {
        return error.into_response();
    }
    let indices = state.resolve_index(&target);
    if indices.is_empty() {
        return missing(&target);
    }
    let mut out = Map::new();
    for index in indices {
        if let Some(metadata) = state.index_metadata(&index) {
            out.insert(index, json!({ "settings": { "index": metadata.settings } }));
        }
    }
    (StatusCode::OK, Json(Value::Object(out))).into_response()
}

/// `PUT /<index>/_settings`.
pub async fn put_settings_handler(
    State(state): State<AppState>,
    Path(target): Path<String>,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&target) {
        return error.into_response();
    }
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parse_exception",
                format!("request body is required and must be JSON: {error}"),
            )
            .into_response()
        }
    };
    // ES accepts the settings at the root or wrapped in `settings`.
    let source = parsed.get("settings").cloned().unwrap_or(parsed);
    let changes = match flatten_index_settings(&source) {
        Ok(changes) if !changes.is_empty() => changes,
        Ok(_) => {
            return OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "action_request_validation_exception",
                "Validation Failed: 1: no settings to update;",
            )
            .into_response()
        }
        Err(reason) => {
            return OpenSearchError::new(StatusCode::BAD_REQUEST, "parse_exception", reason)
                .into_response()
        }
    };
    let indices = state.resolve_index(&target);
    if indices.is_empty() {
        return missing(&target);
    }
    let refused: Vec<&String> = changes.keys().filter(|key| is_static(key)).collect();
    if !refused.is_empty() {
        let names = refused
            .iter()
            .map(|key| key.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "illegal_argument_exception",
            format!(
                "Can't update non dynamic settings [[{names}]] for open indices [[{}]]",
                indices.join(", ")
            ),
        )
        .into_response();
    }
    for index in &indices {
        state.update_index_settings(index, |settings| {
            apply_index_settings(settings, &changes, &[]);
        });
    }
    (StatusCode::OK, Json(json!({ "acknowledged": true }))).into_response()
}
