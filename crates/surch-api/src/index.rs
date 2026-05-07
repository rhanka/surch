use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;

use crate::state::AppState;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateIndexResponse {
    pub acknowledged: bool,
    pub shards_acknowledged: bool,
    pub index: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AcknowledgedResponse {
    pub acknowledged: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RefreshResponse {
    #[serde(rename = "_shards")]
    pub shards: RefreshShards,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RefreshShards {
    pub total: u64,
    pub successful: u64,
    pub failed: u64,
}

pub async fn create_index_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
) -> impl IntoResponse {
    state.create_index(&index);

    (
        StatusCode::OK,
        Json(CreateIndexResponse {
            acknowledged: true,
            shards_acknowledged: true,
            index,
        }),
    )
}

pub async fn delete_index_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
) -> impl IntoResponse {
    state.delete_index(&index);

    (
        StatusCode::OK,
        Json(AcknowledgedResponse { acknowledged: true }),
    )
}

pub async fn refresh_index_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
) -> impl IntoResponse {
    state.refresh_index(&index);

    (
        StatusCode::OK,
        Json(RefreshResponse {
            shards: RefreshShards {
                total: 1,
                successful: 1,
                failed: 0,
            },
        }),
    )
}
