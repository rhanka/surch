//! OpenSearch-compatible `_cat` admin endpoints.

use axum::{
    extract::{Path, State},
    http::{header::CONTENT_TYPE, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{index::validate_index_name, state::AppState, OpenSearchError};

const CLUSTER_NAME: &str = "surch-cluster";

/// Axum handler for `GET /_cat/indices`.
///
/// Always returns JSON for now (P0). OpenSearch defaults to plain text;
/// `?format=json` opts into JSON. We tolerate both by always returning JSON.
pub async fn cat_indices_handler(State(state): State<AppState>) -> impl IntoResponse {
    let rows: Vec<Value> = state
        .index_names()
        .into_iter()
        .map(|index| {
            let docs_count = state.count(&index);
            json!({
                "health": "green",
                "status": "open",
                "index": index,
                "uuid": "_na_",
                "pri": "1",
                "rep": "0",
                "docs.count": docs_count.to_string(),
                "docs.deleted": "0",
                "store.size": "0b",
                "pri.store.size": "0b",
            })
        })
        .collect();

    cat_response(rows)
}

/// Axum handler for `GET /_cat/health`.
pub async fn cat_health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let shard_count = state.index_names().len() as u64;
    let row = json!({
        "epoch": current_epoch_secs().to_string(),
        "timestamp": current_timestamp_hms(),
        "cluster": CLUSTER_NAME,
        "status": "green",
        "node.total": "1",
        "node.data": "1",
        "discovered_master": "true",
        "shards": shard_count.to_string(),
        "pri": shard_count.to_string(),
        "relo": "0",
        "init": "0",
        "unassign": "0",
        "pending_tasks": "0",
        "max_task_wait_time": "-",
        "active_shards_percent": "100.0%",
    });
    cat_response(vec![row])
}

/// Axum handler for `GET /_cat/aliases`.
pub async fn cat_aliases_handler(State(state): State<AppState>) -> impl IntoResponse {
    let rows = collect_alias_rows(&state, None);
    cat_response(rows)
}

/// Axum handler for `GET /_cat/aliases/{name}`.
pub async fn cat_aliases_by_name_handler(
    State(state): State<AppState>,
    Path(alias): Path<String>,
) -> impl IntoResponse {
    if !state.alias_exists(&alias) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "aliases_not_found_exception",
            format!("alias [{alias}] missing"),
        )
        .into_response();
    }
    let rows = collect_alias_rows(&state, Some(alias.as_str()));
    cat_response(rows)
}

/// Axum handler for `GET /_cat/count`.
pub async fn cat_count_handler(State(state): State<AppState>) -> impl IntoResponse {
    let total: u64 = state
        .index_names()
        .iter()
        .map(|index| state.count(index))
        .sum();
    cat_count_response(total)
}

/// Axum handler for `GET /_cat/count/{index}`.
pub async fn cat_count_index_handler(
    State(state): State<AppState>,
    Path(target): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&target) {
        return error.into_response();
    }
    let indices = state.resolve_index(&target);
    if indices.is_empty() {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{target}] missing"),
        )
        .into_response();
    }
    let total: u64 = indices.iter().map(|index| state.count(index)).sum();
    cat_count_response(total)
}

fn cat_count_response(total: u64) -> axum::response::Response {
    let row = json!({
        "epoch": current_epoch_secs().to_string(),
        "timestamp": current_timestamp_hms(),
        "count": total.to_string(),
    });
    cat_response(vec![row])
}

fn collect_alias_rows(state: &AppState, filter: Option<&str>) -> Vec<Value> {
    state
        .all_aliases()
        .into_iter()
        .filter(|(alias, _)| filter.is_none_or(|name| alias == name))
        .flat_map(|(alias, indices)| {
            indices.into_iter().map(move |index| {
                json!({
                    "alias": alias,
                    "index": index,
                    "filter": "-",
                    "routing.index": "-",
                    "routing.search": "-",
                    "is_write_index": "-",
                })
            })
        })
        .collect()
}

fn cat_response(rows: Vec<Value>) -> axum::response::Response {
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        Json(rows),
    )
        .into_response()
}

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn current_timestamp_hms() -> String {
    let secs = current_epoch_secs();
    let seconds = secs % 60;
    let minutes = (secs / 60) % 60;
    let hours = (secs / 3600) % 24;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}
