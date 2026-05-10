//! OpenSearch-compatible `_cat` admin endpoints.

use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};

use crate::state::AppState;

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

    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        Json(rows),
    )
        .into_response()
}
