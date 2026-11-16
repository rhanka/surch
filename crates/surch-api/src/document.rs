use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::Value;

use crate::index::validate_index_name;
use crate::{state::AppState, OpenSearchError};

/// OpenSearch-compatible document index response for the P0 bootstrap API.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DocumentResponse {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_version")]
    pub version: u64,
    pub result: String,
    #[serde(rename = "_shards")]
    pub shards: DocumentShards,
    #[serde(rename = "_seq_no")]
    pub seq_no: u64,
    #[serde(rename = "_primary_term")]
    pub primary_term: u64,
}

/// OpenSearch-compatible shard summary for document writes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DocumentShards {
    pub total: u64,
    pub successful: u64,
    pub failed: u64,
}

/// Build a deterministic P0 OpenSearch-compatible document index response.
pub fn build_document_response(index: String, id: String) -> DocumentResponse {
    DocumentResponse {
        index,
        id,
        version: 1,
        result: "created".to_owned(),
        shards: DocumentShards {
            total: 1,
            successful: 1,
            failed: 0,
        },
        seq_no: 0,
        primary_term: 1,
    }
}

/// Axum handler for the OpenSearch-compatible `/{index}/_doc/{id}` endpoint.
pub async fn document_handler(
    State(state): State<AppState>,
    Path((target, id)): Path<(String, String)>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&target) {
        return error.into_response();
    }

    if id.trim().is_empty() {
        return OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "document id must not be empty",
        )
        .into_response();
    }

    let index = match state.resolve_write_target(&target) {
        Ok(index) => index,
        Err(reason) => {
            return OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "illegal_argument_exception",
                reason,
            )
            .into_response();
        }
    };

    match parse_document_source(&body) {
        Ok(source) => {
            state.index_document(&index, &id, source);
            (
                StatusCode::CREATED,
                Json(build_document_response(index, id)),
            )
                .into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// `GET /{index}/_doc/{id}` (ES get API). L6.2: the matchID restore Job
/// reads its marker document this way before deciding whether to restore;
/// without the route the marker always read as absent and every later
/// monthly restore was skipped.
pub async fn get_document_handler(
    State(state): State<AppState>,
    Path((target, id)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&target) {
        return error.into_response();
    }
    let indices = state.resolve_index(&target);
    let index = match indices.as_slice() {
        [] => {
            return OpenSearchError::new(
                StatusCode::NOT_FOUND,
                "index_not_found_exception",
                format!("no such index [{target}]"),
            )
            .into_response()
        }
        [single] => single.clone(),
        _ => {
            return OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "illegal_argument_exception",
                format!(
                    "alias [{target}] has more than one index associated with it [{}], can't \
                     execute a single index op",
                    indices.join(", ")
                ),
            )
            .into_response()
        }
    };
    match state.get_document(&index, &id) {
        Some(source) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "_index": index,
                "_id": id,
                "_version": 1,
                "_seq_no": 0,
                "_primary_term": 1,
                "found": true,
                "_source": source,
            })),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "_index": index, "_id": id, "found": false })),
        )
            .into_response(),
    }
}

fn parse_document_source(body: &str) -> Result<Value, OpenSearchError> {
    let value: Value = serde_json::from_str(body).map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            error.to_string(),
        )
    })?;

    if value.as_object().is_none() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "document source must be an object",
        ));
    }

    Ok(value)
}
