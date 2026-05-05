use serde_json::Value;
use thiserror::Error;

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
        Some(Value::String(value)) => Ok(Some(value.clone())),
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
