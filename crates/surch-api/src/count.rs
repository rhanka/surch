use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::Value;

use crate::{state::AppState, OpenSearchError};

/// OpenSearch-compatible `_count` request body for the P0 bootstrap surface.
#[derive(Clone, Debug, PartialEq)]
pub struct CountRequest {
    pub query: Option<CountQuery>,
}

/// Supported P0 `_count` queries.
#[derive(Clone, Debug, PartialEq)]
pub enum CountQuery {
    MatchAll,
}

/// OpenSearch-compatible `_count` response for the bootstrap engine-less API.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CountResponse {
    pub count: u64,
    #[serde(rename = "_shards")]
    pub shards: CountShards,
}

/// OpenSearch-compatible shard summary for `_count`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CountShards {
    pub total: u64,
    pub successful: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// Build a deterministic P0 OpenSearch-compatible `_count` response.
pub fn build_count_response(count: u64) -> CountResponse {
    CountResponse {
        count,
        shards: CountShards {
            total: 1,
            successful: 1,
            skipped: 0,
            failed: 0,
        },
    }
}

/// Axum handler for the OpenSearch-compatible `/{index}/_count` endpoint.
pub async fn count_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
    body: String,
) -> impl IntoResponse {
    match parse_count_request(&body) {
        Ok(_request) => (
            StatusCode::OK,
            Json(build_count_response(state.count(&index))),
        )
            .into_response(),
        Err(error) => error.into_response(),
    }
}

fn parse_count_request(body: &str) -> Result<CountRequest, OpenSearchError> {
    if body.trim().is_empty() {
        return Ok(CountRequest { query: None });
    }

    let value: Value = serde_json::from_str(body).map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            error.to_string(),
        )
    })?;

    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "count request body must be an object",
        )
    })?;

    let query = object.get("query").map(parse_count_query).transpose()?;

    Ok(CountRequest { query })
}

fn parse_count_query(value: &Value) -> Result<CountQuery, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "count query must be an object",
        )
    })?;

    if object.len() != 1 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "count query must contain exactly one query type",
        ));
    }

    let (query_type, query_body) = object.iter().next().expect("object has one query type");
    match query_type.as_str() {
        "match_all" if query_body.as_object().is_some_and(|body| body.is_empty()) => {
            Ok(CountQuery::MatchAll)
        }
        "match_all" => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "match_all query body must be an empty object",
        )),
        unknown => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("unsupported count query `{unknown}`"),
        )),
    }
}
