//! OpenSearch-compatible `/_cluster/health` endpoint family.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;

use crate::{index::validate_index_name, state::AppState, OpenSearchError};

const CLUSTER_NAME: &str = "surch-cluster";

/// Axum handler for `GET /_cluster/health`.
pub async fn cluster_health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let indices = state.index_names();
    let response = build_health_response(&indices, None);
    (StatusCode::OK, Json(response)).into_response()
}

/// Axum handler for `GET /_cluster/health/{index}`.
pub async fn cluster_health_index_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }
    if !state.index_exists(&index) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{index}] missing"),
        )
        .into_response();
    }

    let response = build_health_response(std::slice::from_ref(&index), Some(index.as_str()));
    (StatusCode::OK, Json(response)).into_response()
}

fn build_health_response(indices: &[String], focus_index: Option<&str>) -> serde_json::Value {
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
            indices_map.insert(index.clone(), index_health_entry());
        }
        object.insert("indices".to_owned(), serde_json::Value::Object(indices_map));
    }

    response
}

fn index_health_entry() -> serde_json::Value {
    json!({
        "status": "green",
        "number_of_shards": 1,
        "number_of_replicas": 0,
        "active_primary_shards": 1,
        "active_shards": 1,
        "relocating_shards": 0,
        "initializing_shards": 0,
        "unassigned_shards": 0,
    })
}
