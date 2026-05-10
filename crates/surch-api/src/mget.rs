//! OpenSearch-compatible `/_mget` and `/{index}/_mget` endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};

use crate::{
    index::validate_index_name,
    search::{apply_source_filter, parse_source_filter, SourceFilter},
    state::AppState,
    OpenSearchError,
};

#[derive(Clone, Debug, PartialEq)]
struct MgetItem {
    index: String,
    id: String,
    source: Option<SourceFilter>,
}

/// Axum handler for `POST /_mget` (no path index).
pub async fn mget_state_handler(State(state): State<AppState>, body: String) -> impl IntoResponse {
    handle_mget(&state, None, &body)
}

/// Axum handler for `POST /{index}/_mget`.
pub async fn index_mget_state_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }
    handle_mget(&state, Some(index.as_str()), &body)
}

fn handle_mget(
    state: &AppState,
    default_index: Option<&str>,
    body: &str,
) -> axum::response::Response {
    match parse_request(body, default_index) {
        Ok((items, root_source)) => {
            let docs = items
                .into_iter()
                .map(|item| build_response_item(state, item, root_source.as_ref()))
                .collect::<Vec<_>>();
            (StatusCode::OK, Json(json!({ "docs": docs }))).into_response()
        }
        Err(error) => error.into_response(),
    }
}

fn parse_request(
    body: &str,
    default_index: Option<&str>,
) -> Result<(Vec<MgetItem>, Option<SourceFilter>), OpenSearchError> {
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
            "_mget request body must be an object",
        )
    })?;

    let root_source = object.get("_source").map(parse_source_filter).transpose()?;

    if let Some(docs) = object.get("docs") {
        let docs = docs.as_array().ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `docs` must be an array",
            )
        })?;
        if docs.is_empty() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `docs` must not be empty",
            ));
        }
        let items = docs
            .iter()
            .map(|doc| parse_doc_entry(doc, default_index))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((items, root_source));
    }

    if let Some(ids) = object.get("ids") {
        let index = default_index.ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `ids` requires an index in the request path",
            )
        })?;
        let ids = ids.as_array().ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `ids` must be an array",
            )
        })?;
        if ids.is_empty() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `ids` must not be empty",
            ));
        }
        let items = ids
            .iter()
            .map(|id| {
                let id = id.as_str().ok_or_else(|| {
                    OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        "_mget `ids` entries must be strings",
                    )
                })?;
                if id.is_empty() {
                    return Err(OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        "_mget `ids` entries must not be empty",
                    ));
                }
                Ok(MgetItem {
                    index: index.to_owned(),
                    id: id.to_owned(),
                    source: None,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((items, root_source));
    }

    Err(OpenSearchError::new(
        StatusCode::BAD_REQUEST,
        "parsing_exception",
        "_mget request must contain `docs` or `ids`",
    ))
}

fn parse_doc_entry(
    value: &Value,
    default_index: Option<&str>,
) -> Result<MgetItem, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_mget `docs` entries must be objects",
        )
    })?;

    let index = match object.get("_index") {
        Some(Value::String(index)) if !index.is_empty() => {
            validate_index_name(index)?;
            index.clone()
        }
        Some(Value::String(_)) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `_index` must not be empty",
            ));
        }
        Some(_) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `_index` must be a string",
            ));
        }
        None => default_index
            .ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "_mget item is missing `_index`",
                )
            })?
            .to_owned(),
    };

    let id = match object.get("_id") {
        Some(Value::String(id)) if !id.is_empty() => id.clone(),
        Some(Value::String(_)) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `_id` must not be empty",
            ));
        }
        Some(_) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget `_id` must be a string",
            ));
        }
        None => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_mget item is missing `_id`",
            ));
        }
    };

    let source = object.get("_source").map(parse_source_filter).transpose()?;

    Ok(MgetItem { index, id, source })
}

fn build_response_item(
    state: &AppState,
    item: MgetItem,
    root_source: Option<&SourceFilter>,
) -> Value {
    let filter = item.source.as_ref().or(root_source);
    match state.get_document(&item.index, &item.id) {
        Some(source) => {
            let mut response = json!({
                "_index": item.index,
                "_id": item.id,
                "found": true,
            });
            if let Some(filtered) = apply_source_filter(&source, filter) {
                response
                    .as_object_mut()
                    .expect("response object")
                    .insert("_source".to_owned(), filtered);
            }
            response
        }
        None => json!({
            "_index": item.index,
            "_id": item.id,
            "found": false,
        }),
    }
}
