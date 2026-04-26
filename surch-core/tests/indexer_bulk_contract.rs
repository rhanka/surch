use std::collections::HashMap;

use surch_core::common::{BulkAction, FieldValue};
use surch_core::indexer::{build_bulk_operations, BulkOperation};

#[test]
fn build_bulk_operations_pairs_write_actions_with_documents() {
    let operations = build_bulk_operations(&[
        BulkAction::Index {
            index: "books".to_string(),
            id: Some("doc-1".to_string()),
        },
        BulkAction::Document(HashMap::from([(
            "title".to_string(),
            FieldValue::Text("Hello".to_string()),
        )])),
        BulkAction::Create {
            index: "books".to_string(),
            id: Some("doc-2".to_string()),
        },
        BulkAction::Document(HashMap::from([(
            "title".to_string(),
            FieldValue::Text("World".to_string()),
        )])),
        BulkAction::Delete {
            index: "books".to_string(),
            id: "doc-3".to_string(),
        },
    ])
    .expect("bulk operations should parse");

    assert_eq!(operations.len(), 3);
    assert!(matches!(
        &operations[0],
        BulkOperation::Index { index, id, .. } if index == "books" && id.as_deref() == Some("doc-1")
    ));
    assert!(matches!(
        &operations[1],
        BulkOperation::Create { index, id, .. } if index == "books" && id.as_deref() == Some("doc-2")
    ));
    assert!(matches!(
        &operations[2],
        BulkOperation::Delete { index, id } if index == "books" && id == "doc-3"
    ));
}

#[test]
fn build_bulk_operations_rejects_missing_document_after_write_action() {
    let error = build_bulk_operations(&[BulkAction::Index {
        index: "books".to_string(),
        id: Some("doc-1".to_string()),
    }])
    .expect_err("missing document should fail");

    assert!(error.to_string().contains("missing document"));
}
