use crate::common::{BulkAction, FieldValue};
use crate::indexer::error::Error;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum BulkOperation {
    Index {
        index: String,
        id: Option<String>,
        document: HashMap<String, FieldValue>,
    },
    Create {
        index: String,
        id: Option<String>,
        document: HashMap<String, FieldValue>,
    },
    Delete {
        index: String,
        id: String,
    },
}

pub fn build_bulk_operations(actions: &[BulkAction]) -> Result<Vec<BulkOperation>, Error> {
    let mut operations = Vec::new();
    let mut cursor = 0;

    while cursor < actions.len() {
        match &actions[cursor] {
            BulkAction::Index { index, id } => {
                let BulkAction::Document(document) = actions.get(cursor + 1).ok_or_else(|| {
                    Error::Pipeline("missing document after bulk write action".to_string())
                })?
                else {
                    return Err(Error::Pipeline(
                        "missing document after bulk write action".to_string(),
                    ));
                };

                operations.push(BulkOperation::Index {
                    index: index.clone(),
                    id: id.clone(),
                    document: document.clone(),
                });
                cursor += 2;
            }
            BulkAction::Create { index, id } => {
                let BulkAction::Document(document) = actions.get(cursor + 1).ok_or_else(|| {
                    Error::Pipeline("missing document after bulk write action".to_string())
                })?
                else {
                    return Err(Error::Pipeline(
                        "missing document after bulk write action".to_string(),
                    ));
                };

                operations.push(BulkOperation::Create {
                    index: index.clone(),
                    id: id.clone(),
                    document: document.clone(),
                });
                cursor += 2;
            }
            BulkAction::Delete { index, id } => {
                operations.push(BulkOperation::Delete {
                    index: index.clone(),
                    id: id.clone(),
                });
                cursor += 1;
            }
            BulkAction::Document(_) => {
                return Err(Error::Pipeline(
                    "document action without preceding bulk write action".to_string(),
                ));
            }
        }
    }

    Ok(operations)
}
