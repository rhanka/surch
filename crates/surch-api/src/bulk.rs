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
use crate::state::{AppState, DocumentWriteOperation, DocumentWriteResult};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<BulkItemError>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BulkItemError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub reason: String,
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
    #[error("bulk {action} source line at line {line} must be a JSON object")]
    SourceNotObject { line: usize, action: &'static str },
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
            let (items, errors) = apply_bulk_operations(&state, &operations);
            let response = BulkResponse {
                took: started_at.elapsed().as_millis() as u64,
                errors,
                items,
            };
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

            let (items, errors) = apply_bulk_operations(&state, &operations);
            let response = BulkResponse {
                took: started_at.elapsed().as_millis() as u64,
                errors,
                items,
            };
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

fn apply_bulk_operations(
    state: &AppState,
    operations: &[BulkOperation],
) -> (Vec<BulkResponseItem>, bool) {
    let mut items = vec![None; operations.len()];
    let mut writes = Vec::new();

    for (position, operation) in operations.iter().enumerate() {
        match build_bulk_write_operation(state, operation) {
            Ok((kind, write)) => writes.push((position, kind, write)),
            Err(item) => items[position] = Some(item),
        }
    }

    let write_results = state.apply_document_writes(
        writes
            .iter()
            .map(|(_, _, operation)| operation.clone())
            .collect(),
    );

    for ((position, kind, _), result) in writes.into_iter().zip(write_results) {
        items[position] = Some(bulk_item_from_write_result(kind, result));
    }

    let items = items
        .into_iter()
        .map(|item| item.expect("each bulk operation should yield a response item"))
        .collect::<Vec<_>>();
    let has_error = items.iter().any(item_has_error);

    (items, has_error)
}

#[derive(Clone, Copy)]
enum BulkActionKind {
    Index,
    Create,
    Delete,
    Update,
}

fn build_bulk_write_operation(
    state: &AppState,
    operation: &BulkOperation,
) -> Result<(BulkActionKind, DocumentWriteOperation), BulkResponseItem> {
    match operation {
        BulkOperation::Index { index, id, source } => build_index_like_write_operation(
            state,
            index.as_deref(),
            id.as_deref(),
            source,
            201,
            BulkActionKind::Index,
        ),
        BulkOperation::Update { index, id, source } => {
            let source = source.get("doc").cloned().unwrap_or_else(|| source.clone());
            build_index_like_write_operation(
                state,
                index.as_deref(),
                id.as_deref(),
                &source,
                200,
                BulkActionKind::Update,
            )
        }
        BulkOperation::Create { index, id, source } => {
            match resolve_bulk_target(state, index.as_deref(), id.as_deref()) {
                Ok(resolved) => {
                    let id = id
                        .as_ref()
                        .expect("resolved target ensures id is Some")
                        .clone();
                    Ok((
                        BulkActionKind::Create,
                        DocumentWriteOperation::Create {
                            index: resolved,
                            id,
                            source: source.clone(),
                        },
                    ))
                }
                Err(status) => Err(BulkResponseItem::Create(status)),
            }
        }
        BulkOperation::Delete { index, id } => {
            match resolve_bulk_target(state, index.as_deref(), id.as_deref()) {
                Ok(resolved) => {
                    let id = id
                        .as_ref()
                        .expect("resolved target ensures id is Some")
                        .clone();
                    Ok((
                        BulkActionKind::Delete,
                        DocumentWriteOperation::Delete {
                            index: resolved,
                            id,
                        },
                    ))
                }
                Err(status) => Err(BulkResponseItem::Delete(status)),
            }
        }
    }
}

fn resolve_bulk_target(
    state: &AppState,
    index: Option<&str>,
    id: Option<&str>,
) -> Result<String, BulkItemStatus> {
    let Some(id) = id else {
        return Err(item_status_with_error(
            index,
            None,
            400,
            "illegal_argument_exception",
            "missing _id in bulk operation metadata",
        ));
    };
    let Some(index) = index else {
        return Err(item_status_with_error(
            None,
            Some(id),
            400,
            "illegal_argument_exception",
            "missing _index in bulk operation metadata",
        ));
    };
    state.resolve_write_target(index).map_err(|reason| {
        item_status_with_error(
            Some(index),
            Some(id),
            400,
            "illegal_argument_exception",
            &reason,
        )
    })
}

fn build_index_like_write_operation(
    state: &AppState,
    index: Option<&str>,
    id: Option<&str>,
    source: &Value,
    status: u16,
    kind: BulkActionKind,
) -> Result<(BulkActionKind, DocumentWriteOperation), BulkResponseItem> {
    match resolve_bulk_target(state, index, id) {
        Ok(resolved) => {
            let id = id.expect("resolved target ensures id is Some").to_owned();
            Ok((
                kind,
                DocumentWriteOperation::Index {
                    index: resolved,
                    id,
                    source: source.clone(),
                    status,
                },
            ))
        }
        Err(status) => Err(match kind {
            BulkActionKind::Index => BulkResponseItem::Index(status),
            BulkActionKind::Create => BulkResponseItem::Create(status),
            BulkActionKind::Delete => BulkResponseItem::Delete(status),
            BulkActionKind::Update => BulkResponseItem::Update(status),
        }),
    }
}

fn bulk_item_from_write_result(
    kind: BulkActionKind,
    result: DocumentWriteResult,
) -> BulkResponseItem {
    let status = match result {
        DocumentWriteResult::Applied { index, id, status } => {
            item_status(&Some(index), &Some(id), status)
        }
        DocumentWriteResult::VersionConflict { index, id } => item_status_with_error(
            Some(index.as_str()),
            Some(id.as_str()),
            409,
            "version_conflict_engine_exception",
            "document already exists",
        ),
    };

    match kind {
        BulkActionKind::Index => BulkResponseItem::Index(status),
        BulkActionKind::Create => BulkResponseItem::Create(status),
        BulkActionKind::Delete => BulkResponseItem::Delete(status),
        BulkActionKind::Update => BulkResponseItem::Update(status),
    }
}

fn item_has_error(item: &BulkResponseItem) -> bool {
    match item {
        BulkResponseItem::Index(status)
        | BulkResponseItem::Create(status)
        | BulkResponseItem::Delete(status)
        | BulkResponseItem::Update(status) => status.error.is_some(),
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
        error: None,
    }
}

fn item_status_with_error(
    index: Option<&str>,
    id: Option<&str>,
    status: u16,
    error_type: &str,
    reason: &str,
) -> BulkItemStatus {
    BulkItemStatus {
        index: index.map(ToString::to_string),
        id: id.map(ToString::to_string),
        status,
        error: Some(BulkItemError {
            error_type: error_type.to_owned(),
            reason: reason.to_owned(),
        }),
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
            if value.is_empty() {
                return Err(BulkParseError::InvalidAction {
                    line,
                    reason: match field {
                        "_index" => "_index metadata must not be empty",
                        "_id" => "_id metadata must not be empty",
                        _ => "metadata field must not be empty",
                    },
                });
            }
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

    let source: Value = serde_json::from_str(source_line)
        .map_err(|source| BulkParseError::InvalidSourceJson { line, source })?;
    if !source.is_object() {
        return Err(BulkParseError::SourceNotObject { line, action });
    }

    Ok(source)
}
