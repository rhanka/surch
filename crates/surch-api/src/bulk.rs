use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Instant;
use thiserror::Error;

use crate::index::validate_index_name;
use crate::state::AppState;

/// One parsed OpenSearch `_bulk` NDJSON operation.
#[derive(Clone, Debug, PartialEq)]
pub enum BulkOperation {
    Index {
        index: Option<String>,
        id: Option<String>,
        source: Value,
    },
    Create {
        index: Option<String>,
        id: Option<String>,
        source: Value,
    },
    Delete {
        index: Option<String>,
        id: Option<String>,
    },
    Update {
        index: Option<String>,
        id: Option<String>,
        source: Value,
    },
}

/// OpenSearch-compatible `_bulk` response for parsed operations.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BulkResponse {
    pub took: u64,
    pub errors: bool,
    pub items: Vec<BulkResponseItem>,
}

/// One item in an OpenSearch-compatible `_bulk` response.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BulkResponseItem {
    Index(BulkItemStatus),
    Create(BulkItemStatus),
    Delete(BulkItemStatus),
    Update(BulkItemStatus),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BulkItemStatus {
    #[serde(rename = "_index", skip_serializing_if = "Option::is_none")]
    pub index: Option<String>,
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub status: u16,
}

/// Error returned when `_bulk` NDJSON cannot be parsed into valid operations.
#[derive(Debug, Error)]
pub enum BulkParseError {
    #[error("invalid bulk action json at line {line}: {source}")]
    InvalidActionJson {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid bulk source json at line {line}: {source}")]
    InvalidSourceJson {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid bulk action at line {line}: {reason}")]
    InvalidAction { line: usize, reason: &'static str },
    #[error("unknown bulk action `{action}` at line {line}")]
    UnknownAction { line: usize, action: String },
    #[error("missing source line after `{action}` action at line {line}")]
    MissingSource { line: usize, action: &'static str },
}

/// Build a deterministic P0 OpenSearch-compatible `_bulk` response.
pub fn build_bulk_response(operations: &[BulkOperation], took: u64) -> BulkResponse {
    let items = operations
        .iter()
        .map(|operation| match operation {
            BulkOperation::Index { index, id, .. } => {
                BulkResponseItem::Index(item_status(index, id, 201))
            }
            BulkOperation::Create { index, id, .. } => {
                BulkResponseItem::Create(item_status(index, id, 201))
            }
            BulkOperation::Delete { index, id } => {
                BulkResponseItem::Delete(item_status(index, id, 200))
            }
            BulkOperation::Update { index, id, .. } => {
                BulkResponseItem::Update(item_status(index, id, 200))
            }
        })
        .collect();

    BulkResponse {
        took,
        errors: false,
        items,
    }
}

/// Axum handler for the OpenSearch-compatible `_bulk` endpoint.
pub async fn bulk_state_handler(State(state): State<AppState>, body: String) -> impl IntoResponse {
    let started_at = Instant::now();
    match parse_bulk_ndjson(&body) {
        Ok(operations) => {
            apply_bulk_operations(&state, &operations);
            let response =
                build_bulk_response(&operations, started_at.elapsed().as_millis() as u64);
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => bulk_parse_error_response(error),
    }
}

/// Axum handler for `POST|PUT /:index/_bulk` with path-index default.
pub async fn index_bulk_state_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }

    let started_at = Instant::now();
    match parse_bulk_ndjson(&body) {
        Ok(operations) => {
            let operations: Vec<BulkOperation> = operations
                .into_iter()
                .map(|operation| apply_default_index(&operation, Some(index.as_str())))
                .collect();

            apply_bulk_operations(&state, &operations);
            let response =
                build_bulk_response(&operations, started_at.elapsed().as_millis() as u64);
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => bulk_parse_error_response(error),
    }
}

/// State-less `_bulk` handler used by parser/response bootstrap tests.
pub async fn bulk_handler(body: String) -> impl IntoResponse {
    match parse_bulk_ndjson(&body) {
        Ok(operations) => {
            let response = build_bulk_response(&operations, 0);
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => bulk_parse_error_response(error),
    }
}

fn apply_bulk_operations(state: &AppState, operations: &[BulkOperation]) {
    for operation in operations {
        apply_bulk_operation(state, operation);
    }
}

fn apply_bulk_operation(state: &AppState, operation: &BulkOperation) {
    match operation {
        BulkOperation::Index { index, id, source }
        | BulkOperation::Create { index, id, source } => {
            if let (Some(index), Some(id)) = (index, id) {
                state.index_document(index, id, source.clone());
            }
        }
        BulkOperation::Update { index, id, source } => {
            if let (Some(index), Some(id)) = (index, id) {
                let source = source.get("doc").cloned().unwrap_or_else(|| source.clone());
                state.index_document(index, id, source);
            }
        }
        BulkOperation::Delete { index, id } => {
            if let (Some(index), Some(id)) = (index, id) {
                state.delete_document(index, id);
            }
        }
    }
}

fn apply_default_index(operation: &BulkOperation, default_index: Option<&str>) -> BulkOperation {
    match operation {
        BulkOperation::Index { index, id, source } => BulkOperation::Index {
            index: index.clone().or_else(|| default_index.map(str::to_owned)),
            id: id.clone(),
            source: source.clone(),
        },
        BulkOperation::Create { index, id, source } => BulkOperation::Create {
            index: index.clone().or_else(|| default_index.map(str::to_owned)),
            id: id.clone(),
            source: source.clone(),
        },
        BulkOperation::Delete { index, id } => BulkOperation::Delete {
            index: index.clone().or_else(|| default_index.map(str::to_owned)),
            id: id.clone(),
        },
        BulkOperation::Update { index, id, source } => BulkOperation::Update {
            index: index.clone().or_else(|| default_index.map(str::to_owned)),
            id: id.clone(),
            source: source.clone(),
        },
    }
}

fn bulk_parse_error_response(error: BulkParseError) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "type": "parse_exception",
                "reason": error.to_string(),
            },
            "status": 400,
        })),
    )
        .into_response()
}

fn item_status(index: &Option<String>, id: &Option<String>, status: u16) -> BulkItemStatus {
    BulkItemStatus {
        index: index.clone(),
        id: id.clone(),
        status,
    }
}

#[derive(Clone, Debug, PartialEq)]
struct BulkMetadata {
    index: Option<String>,
    id: Option<String>,
}

/// Parse an OpenSearch-compatible `_bulk` NDJSON body into typed operations.
pub fn parse_bulk_ndjson(input: &str) -> Result<Vec<BulkOperation>, BulkParseError> {
    let mut lines = input.lines().enumerate().peekable();
    let mut operations = Vec::new();

    while let Some((action_line_index, action_line)) = lines.next() {
        let line = action_line_index + 1;
        let (action, metadata) = parse_action_line(action_line, line)?;

        match action.as_str() {
            "index" => {
                let source = parse_required_source(&mut lines, line, "index")?;
                operations.push(BulkOperation::Index {
                    index: metadata.index,
                    id: metadata.id,
                    source,
                });
            }
            "create" => {
                let source = parse_required_source(&mut lines, line, "create")?;
                operations.push(BulkOperation::Create {
                    index: metadata.index,
                    id: metadata.id,
                    source,
                });
            }
            "delete" => {
                operations.push(BulkOperation::Delete {
                    index: metadata.index,
                    id: metadata.id,
                });
            }
            "update" => {
                let source = parse_required_source(&mut lines, line, "update")?;
                operations.push(BulkOperation::Update {
                    index: metadata.index,
                    id: metadata.id,
                    source,
                });
            }
            _ => {
                return Err(BulkParseError::UnknownAction { line, action });
            }
        }
    }

    Ok(operations)
}

fn parse_action_line(
    action_line: &str,
    line: usize,
) -> Result<(String, BulkMetadata), BulkParseError> {
    let value: Value = serde_json::from_str(action_line)
        .map_err(|source| BulkParseError::InvalidActionJson { line, source })?;

    let object = value.as_object().ok_or(BulkParseError::InvalidAction {
        line,
        reason: "action line must be a JSON object",
    })?;

    if object.len() != 1 {
        return Err(BulkParseError::InvalidAction {
            line,
            reason: "action line must contain exactly one action",
        });
    }

    let (action, metadata) = object.iter().next().expect("object has one action");
    let metadata = parse_metadata(metadata, line)?;

    Ok((action.clone(), metadata))
}

fn parse_metadata(value: &Value, line: usize) -> Result<BulkMetadata, BulkParseError> {
    let object = value.as_object().ok_or(BulkParseError::InvalidAction {
        line,
        reason: "action metadata must be a JSON object",
    })?;

    let index = optional_string_field(object.get("_index"), line, "_index")?;
    let id = optional_string_field(object.get("_id"), line, "_id")?;

    Ok(BulkMetadata { index, id })
}

fn optional_string_field(
    value: Option<&Value>,
    line: usize,
    field: &'static str,
) -> Result<Option<String>, BulkParseError> {
    match value {
        Some(Value::String(value)) => {
            if field == "_index" && validate_index_name(value).is_err() {
                return Err(BulkParseError::InvalidAction {
                    line,
                    reason: "_index metadata must be a valid index name",
                });
            }
            Ok(Some(value.clone()))
        }
        Some(_) => Err(BulkParseError::InvalidAction {
            line,
            reason: match field {
                "_index" => "_index metadata must be a string",
                "_id" => "_id metadata must be a string",
                _ => "metadata field must be a string",
            },
        }),
        None => Ok(None),
    }
}

fn parse_required_source<'a, I>(
    lines: &mut std::iter::Peekable<I>,
    action_line: usize,
    action: &'static str,
) -> Result<Value, BulkParseError>
where
    I: Iterator<Item = (usize, &'a str)>,
{
    let (source_line_index, source_line) = lines.next().ok_or(BulkParseError::MissingSource {
        line: action_line,
        action,
    })?;
    let line = source_line_index + 1;

    serde_json::from_str(source_line)
        .map_err(|source| BulkParseError::InvalidSourceJson { line, source })
}
