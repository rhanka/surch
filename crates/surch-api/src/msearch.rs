//! OpenSearch-compatible `/_msearch` and `/{index}/_msearch` endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};

use crate::{
    index::validate_index_name,
    search::{parse_search_request, run_search},
    state::AppState,
    OpenSearchError,
};

/// Axum handler for `POST /_msearch` (NDJSON, no default index).
pub async fn msearch_state_handler(
    State(state): State<AppState>,
    body: String,
) -> impl IntoResponse {
    handle_msearch(&state, None, &body)
}

/// Axum handler for `POST /{index}/_msearch` with the path index as default.
pub async fn index_msearch_state_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }
    handle_msearch(&state, Some(index.as_str()), &body)
}

fn handle_msearch(
    state: &AppState,
    default_index: Option<&str>,
    body: &str,
) -> axum::response::Response {
    match parse_msearch_pairs(body, default_index) {
        Ok(pairs) => {
            let responses: Vec<Value> = pairs
                .into_iter()
                .map(|pair| build_pair_response(state, pair))
                .collect();
            (StatusCode::OK, Json(json!({ "responses": responses }))).into_response()
        }
        Err(error) => error.into_response(),
    }
}

#[derive(Debug)]
enum PairResolution {
    Ready { index: String, query_body: String },
    HeaderError(OpenSearchError),
}

fn parse_msearch_pairs(
    body: &str,
    default_index: Option<&str>,
) -> Result<Vec<PairResolution>, OpenSearchError> {
    let lines: Vec<&str> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_msearch request must contain at least one search",
        ));
    }
    if !lines.len().is_multiple_of(2) {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_msearch NDJSON must contain header/body pairs",
        ));
    }

    let mut pairs = Vec::with_capacity(lines.len() / 2);
    for chunk in lines.chunks(2) {
        let header = chunk[0];
        let query_body = chunk[1].to_owned();
        match resolve_header(header, default_index) {
            Ok(index) => pairs.push(PairResolution::Ready { index, query_body }),
            Err(error) => pairs.push(PairResolution::HeaderError(error)),
        }
    }

    Ok(pairs)
}

fn resolve_header(header: &str, default_index: Option<&str>) -> Result<String, OpenSearchError> {
    let value: Value = serde_json::from_str(header).map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("invalid _msearch header: {error}"),
        )
    })?;

    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_msearch header must be a JSON object",
        )
    })?;

    let header_index = match object.get("index") {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(Value::String(_)) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_msearch header `index` must not be empty",
            ));
        }
        Some(_) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_msearch header `index` must be a string",
            ));
        }
        None => None,
    };

    let resolved = header_index
        .or_else(|| default_index.map(str::to_owned))
        .ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_msearch header is missing `index`",
            )
        })?;

    validate_index_name(&resolved)?;
    Ok(resolved)
}

fn build_pair_response(state: &AppState, pair: PairResolution) -> Value {
    match pair {
        PairResolution::HeaderError(error) => error_to_value(&error),
        PairResolution::Ready { index, query_body } => {
            let resolved = state.resolve_index(&index);
            if resolved.is_empty() {
                return error_to_value(&OpenSearchError::new(
                    StatusCode::NOT_FOUND,
                    "index_not_found_exception",
                    format!("index [{index}] missing"),
                ));
            }
            match parse_search_request(&query_body) {
                Ok(request) => {
                    let response = run_search(state, &resolved, &request);
                    let mut value =
                        serde_json::to_value(response).expect("search response should serialize");
                    value
                        .as_object_mut()
                        .expect("search response is a json object")
                        .insert("status".to_owned(), json!(200));
                    value
                }
                Err(error) => error_to_value(&error),
            }
        }
    }
}

fn error_to_value(error: &OpenSearchError) -> Value {
    serde_json::to_value(error).expect("error envelope should serialize")
}
