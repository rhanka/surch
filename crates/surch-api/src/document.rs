use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use serde_json::Value;

use crate::OpenSearchError;

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
    Path((index, id)): Path<(String, String)>,
    body: String,
) -> impl IntoResponse {
    match parse_document_source(&body) {
        Ok(_source) => (
            StatusCode::CREATED,
            Json(build_document_response(index, id)),
        )
            .into_response(),
        Err(error) => error.into_response(),
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
