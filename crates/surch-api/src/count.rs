use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    state::{AppState, StoredDocument},
    OpenSearchError,
};

/// OpenSearch-compatible `_count` request body for the P0 bootstrap surface.
#[derive(Clone, Debug, PartialEq)]
pub struct CountRequest {
    pub query: Option<CountQuery>,
}

/// Supported P0 `_count` queries.
#[derive(Clone, Debug, PartialEq)]
pub enum CountQuery {
    MatchAll,
    Term { field: String, value: String },
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
        Ok(request) => {
            let count = state
                .documents(&index)
                .into_iter()
                .filter(|document| request_matches(&request, document))
                .count() as u64;

            (StatusCode::OK, Json(build_count_response(count))).into_response()
        }
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
        "term" => parse_term_query(query_body),
        unknown => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("unsupported count query `{unknown}`"),
        )),
    }
}

fn parse_term_query(value: &Value) -> Result<CountQuery, OpenSearchError> {
    let (field, value) = parse_single_field_query("term", value)?;
    let value = parse_term_value(value)?;

    Ok(CountQuery::Term { field, value })
}

fn parse_single_field_query<'a>(
    query_type: &str,
    value: &'a Value,
) -> Result<(String, &'a Value), OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{query_type} query body must be an object"),
        )
    })?;

    if object.len() != 1 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{query_type} query must contain exactly one field"),
        ));
    }

    let (field, value) = object.iter().next().expect("object has one field");
    Ok((field.clone(), value))
}

fn parse_term_value(value: &Value) -> Result<String, OpenSearchError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Object(object) => object
            .get("value")
            .ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "term field query object must contain `value`",
                )
            })
            .and_then(parse_term_value),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "term query value must be a scalar value",
        )),
    }
}

fn request_matches(request: &CountRequest, document: &StoredDocument) -> bool {
    match request.query.as_ref() {
        Some(query) => query_matches(query, &document.source),
        None => true,
    }
}

fn query_matches(query: &CountQuery, source: &Value) -> bool {
    match query {
        CountQuery::MatchAll => true,
        CountQuery::Term { field, value } => term_field_matches(source, field, value),
    }
}

fn term_field_matches(source: &Value, field: &str, query: &str) -> bool {
    let query = normalize_text(query);
    if query.is_empty() {
        return false;
    }

    field_text(source, field)
        .map(|value| {
            tokenize_for_search(&value)
                .iter()
                .any(|field_token| field_token == &query)
        })
        .unwrap_or(false)
}

fn field_text(source: &Value, field: &str) -> Option<String> {
    match source.get(field)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn tokenize_for_search(value: &str) -> Vec<String> {
    normalize_text(value)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn normalize_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold_search_char)
        .collect()
}

fn fold_search_char(character: char) -> char {
    match character {
        '\u{00e0}' | '\u{00e1}' | '\u{00e2}' | '\u{00e3}' | '\u{00e4}' | '\u{00e5}' => 'a',
        '\u{00e7}' => 'c',
        '\u{00e8}' | '\u{00e9}' | '\u{00ea}' | '\u{00eb}' => 'e',
        '\u{00ec}' | '\u{00ed}' | '\u{00ee}' | '\u{00ef}' => 'i',
        '\u{00f1}' => 'n',
        '\u{00f2}' | '\u{00f3}' | '\u{00f4}' | '\u{00f5}' | '\u{00f6}' => 'o',
        '\u{00f9}' | '\u{00fa}' | '\u{00fb}' | '\u{00fc}' => 'u',
        '\u{00fd}' | '\u{00ff}' => 'y',
        character if character.is_alphanumeric() => character,
        _ => ' ',
    }
}
