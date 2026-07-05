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
        action: &'static str,
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

/// A `_bulk` item that failed to parse, pinned to its original position so
/// the rest of the body can still be applied instead of failing the whole
/// request. Mirrors OpenSearch's per-item bulk error semantics: only the
/// item at fault is rejected (HTTP 200, `errors:true`, a 400 item status),
/// the remaining well-formed items are applied normally.
#[derive(Debug)]
pub struct BulkParseItemError {
    pub index: Option<String>,
    pub id: Option<String>,
    pub error: BulkParseError,
}

/// One parsed `_bulk` NDJSON item: a valid operation ready to be applied,
/// or a `BulkParseItemError` for a line that failed to parse. `parse_bulk_ndjson`
/// yields one of these per action/source pair in body order, so downstream
/// response building can slot per-item errors into the response `items`
/// array at the same position a successfully applied item would occupy.
pub type BulkItemParseResult = Result<BulkOperation, BulkParseItemError>;

/// Build a deterministic P0 OpenSearch-compatible `_bulk` response.
pub fn build_bulk_response(operations: &[BulkItemParseResult], took: u64) -> BulkResponse {
    let items = operations
        .iter()
        .map(|operation| match operation {
            Ok(BulkOperation::Index { index, id, .. }) => {
                BulkResponseItem::Index(item_status(index, id, 201))
            }
            Ok(BulkOperation::Create { index, id, .. }) => {
                BulkResponseItem::Create(item_status(index, id, 201))
            }
            Ok(BulkOperation::Delete { index, id }) => {
                BulkResponseItem::Delete(item_status(index, id, 200))
            }
            Ok(BulkOperation::Update { index, id, .. }) => {
                BulkResponseItem::Update(item_status(index, id, 200))
            }
            Err(parse_error) => bulk_response_item_for_parse_error(parse_error),
        })
        .collect::<Vec<_>>();
    let errors = items.iter().any(item_has_error);

    BulkResponse {
        took,
        errors,
        items,
    }
}

/// Axum handler for the OpenSearch-compatible `_bulk` endpoint.
pub async fn bulk_state_handler(State(state): State<AppState>, body: String) -> impl IntoResponse {
    let started_at = Instant::now();
    match require_any_valid_operation(parse_bulk_ndjson(&body)) {
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
    match require_any_valid_operation(parse_bulk_ndjson(&body)) {
        Ok(operations) => {
            let operations: Vec<BulkItemParseResult> = operations
                .into_iter()
                .map(|operation| {
                    operation.map(|operation| apply_default_index(&operation, Some(index.as_str())))
                })
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
    match require_any_valid_operation(parse_bulk_ndjson(&body)) {
        Ok(operations) => {
            let response = build_bulk_response(&operations, 0);
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => bulk_parse_error_response(error),
    }
}

fn apply_bulk_operations(
    state: &AppState,
    operations: &[BulkItemParseResult],
) -> (Vec<BulkResponseItem>, bool) {
    let mut items = vec![None; operations.len()];
    let mut writes = Vec::new();

    for (position, item) in operations.iter().enumerate() {
        match item {
            Ok(operation) => match build_bulk_write_operation(state, operation) {
                Ok((kind, write)) => writes.push((position, kind, write)),
                Err(item) => items[position] = Some(item),
            },
            Err(parse_error) => {
                items[position] = Some(bulk_response_item_for_parse_error(parse_error));
            }
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

/// Render a `BulkParseItemError` as an OpenSearch-compatible response item
/// (HTTP 400, `parse_exception`). The wrapper key mirrors the recognized
/// action when the failure carries one (a bad `_index`/`_id` or a broken
/// source line still knows whether the caller meant `index`/`create`/
/// `update`); anything else (malformed JSON, an unknown action name) has no
/// action to key off of and defaults to `index`, the most common shape.
fn bulk_response_item_for_parse_error(item_error: &BulkParseItemError) -> BulkResponseItem {
    let status = item_status_with_error(
        item_error.index.as_deref(),
        item_error.id.as_deref(),
        400,
        "parse_exception",
        &item_error.error.to_string(),
    );

    match &item_error.error {
        BulkParseError::MissingSource { action, .. }
        | BulkParseError::SourceNotObject { action, .. }
        | BulkParseError::InvalidSourceJson { action, .. } => match *action {
            "create" => BulkResponseItem::Create(status),
            "update" => BulkResponseItem::Update(status),
            _ => BulkResponseItem::Index(status),
        },
        _ => BulkResponseItem::Index(status),
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

/// Guard between the per-item parse results and the OpenSearch-compatible
/// partial response: when at least one item parsed into a valid operation,
/// the body is applied item-by-item (`errors:true`, HTTP 200, a 400 status
/// on the faulty items) exactly like the rest of `_bulk`'s per-item error
/// handling. The sole remaining case for a top-level 400 is a body where
/// every single line failed to parse (nothing at all could be applied);
/// that case keeps surfacing the first parse error, matching the prior
/// tout-ou-rien behavior for a wholly-invalid payload. An empty body (no
/// lines at all) is not an error either way, so it is passed through.
fn require_any_valid_operation(
    operations: Vec<BulkItemParseResult>,
) -> Result<Vec<BulkItemParseResult>, BulkParseError> {
    if operations.is_empty() || operations.iter().any(Result::is_ok) {
        return Ok(operations);
    }

    let error = operations
        .into_iter()
        .find_map(|operation| operation.err().map(|item_error| item_error.error))
        .expect("a non-empty all-error operations list has at least one error");

    Err(error)
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

/// Parse an OpenSearch-compatible `_bulk` NDJSON body into per-item
/// results, one per action/source pair in body order. A malformed action
/// line or source line no longer aborts the whole request: it yields a
/// `BulkParseItemError` at its position and parsing resumes at the next
/// action line, matching OpenSearch's per-item bulk semantics (the caller
/// decides whether the resulting mix of `Ok`/`Err` items still contains
/// anything to apply; see `require_any_valid_operation`).
pub fn parse_bulk_ndjson(input: &str) -> Vec<BulkItemParseResult> {
    let mut lines = input.lines().enumerate().peekable();
    let mut items = Vec::new();

    while let Some((action_line_index, action_line)) = lines.next() {
        let line = action_line_index + 1;

        match parse_action_line(action_line, line) {
            Ok((action, metadata)) => match action.as_str() {
                "index" => items.push(match parse_required_source(&mut lines, line, "index") {
                    Ok(source) => Ok(BulkOperation::Index {
                        index: metadata.index,
                        id: metadata.id,
                        source,
                    }),
                    Err(error) => Err(BulkParseItemError {
                        index: metadata.index,
                        id: metadata.id,
                        error,
                    }),
                }),
                "create" => items.push(match parse_required_source(&mut lines, line, "create") {
                    Ok(source) => Ok(BulkOperation::Create {
                        index: metadata.index,
                        id: metadata.id,
                        source,
                    }),
                    Err(error) => Err(BulkParseItemError {
                        index: metadata.index,
                        id: metadata.id,
                        error,
                    }),
                }),
                "delete" => items.push(Ok(BulkOperation::Delete {
                    index: metadata.index,
                    id: metadata.id,
                })),
                "update" => items.push(match parse_required_source(&mut lines, line, "update") {
                    Ok(source) => Ok(BulkOperation::Update {
                        index: metadata.index,
                        id: metadata.id,
                        source,
                    }),
                    Err(error) => Err(BulkParseItemError {
                        index: metadata.index,
                        id: metadata.id,
                        error,
                    }),
                }),
                _ => items.push(Err(BulkParseItemError {
                    index: metadata.index,
                    id: metadata.id,
                    error: BulkParseError::UnknownAction { line, action },
                })),
            },
            Err((Some(action), error)) => {
                // The action name was recovered even though something else
                // about the line was invalid (typically a bad `_index`/
                // `_id`). When that action expects a source line, discard
                // the paired line now, so the next loop iteration resyncs
                // on the following action line instead of misreading an
                // orphaned source line as a new one.
                if matches!(action.as_str(), "index" | "create" | "update") {
                    let _ = lines.next();
                }
                items.push(Err(BulkParseItemError {
                    index: None,
                    id: None,
                    error,
                }));
            }
            Err((None, error)) => {
                // No action name could be recovered at all (malformed JSON,
                // or the wrong shape): there is no reliable arity to resync
                // against, so parsing simply resumes at the next line.
                items.push(Err(BulkParseItemError {
                    index: None,
                    id: None,
                    error,
                }));
            }
        }
    }

    items
}

/// Result of parsing just the action line: either the action name with its
/// fully validated metadata, or a failure that may still carry the
/// recovered action name (`Some`) when the JSON shape was recognizable
/// enough to identify which of `index`/`create`/`delete`/`update` was
/// requested, even though something else about the line was invalid.
type ActionLineResult = Result<(String, BulkMetadata), (Option<String>, BulkParseError)>;

fn parse_action_line(action_line: &str, line: usize) -> ActionLineResult {
    let value: Value = serde_json::from_str(action_line)
        .map_err(|source| (None, BulkParseError::InvalidActionJson { line, source }))?;

    let object = value.as_object().ok_or((
        None,
        BulkParseError::InvalidAction {
            line,
            reason: "action line must be a JSON object",
        },
    ))?;

    if object.len() != 1 {
        return Err((
            None,
            BulkParseError::InvalidAction {
                line,
                reason: "action line must contain exactly one action",
            },
        ));
    }

    let (action, metadata) = object.iter().next().expect("object has one action");

    match parse_metadata(metadata, line) {
        Ok(metadata) => Ok((action.clone(), metadata)),
        Err(error) => Err((Some(action.clone()), error)),
    }
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

    let source: Value =
        serde_json::from_str(source_line).map_err(|source| BulkParseError::InvalidSourceJson {
            line,
            action,
            source,
        })?;
    if !source.is_object() {
        return Err(BulkParseError::SourceNotObject { line, action });
    }

    Ok(source)
}
