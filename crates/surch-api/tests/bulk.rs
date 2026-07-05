use serde_json::json;
use surch_api::{
    bulk::{build_bulk_response, BulkResponse},
    parse_bulk_ndjson, BulkItemParseResult, BulkOperation, BulkParseError,
};

/// Expects every parsed item to be a valid operation, in order. Panics
/// (printing the offending parse error) if any item failed to parse —
/// used by tests that only exercise the happy path.
fn expect_operations(items: Vec<BulkItemParseResult>) -> Vec<BulkOperation> {
    items
        .into_iter()
        .map(|item| item.expect("bulk item should parse into a valid operation"))
        .collect()
}

/// Expects exactly one parsed item and expects it to be a parse error,
/// returning the underlying `BulkParseError` — used by tests that exercise
/// a single malformed action/source pair with nothing else in the body.
fn expect_single_error(items: Vec<BulkItemParseResult>) -> BulkParseError {
    assert_eq!(items.len(), 1, "expected exactly one parsed item");
    items
        .into_iter()
        .next()
        .expect("one item")
        .expect_err("expected a parse error")
        .error
}

#[test]
fn bulk_parses_index_and_delete_operations_in_order() {
    let body = r#"{"index":{"_index":"books","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"title":"Rust Search"}"#
        + "\n"
        + r#"{"delete":{"_index":"books","_id":"2"}}"#
        + "\n";
    let operations = expect_operations(parse_bulk_ndjson(&body));

    assert_eq!(operations.len(), 2);
    assert_eq!(
        operations[0],
        BulkOperation::Index {
            index: Some("books".to_owned()),
            id: Some("1".to_owned()),
            source: json!({"title": "Rust Search"}),
        }
    );
    assert_eq!(
        operations[1],
        BulkOperation::Delete {
            index: Some("books".to_owned()),
            id: Some("2".to_owned()),
        }
    );
}

#[test]
fn bulk_accepts_request_without_final_newline() {
    let body = r#"{"index":{"_index":"books","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"title":"Rust Search"}"#;
    let operations = expect_operations(parse_bulk_ndjson(&body));

    assert_eq!(operations.len(), 1);
}

#[test]
fn bulk_rejects_source_required_action_without_source_line() {
    let err = expect_single_error(parse_bulk_ndjson(
        r#"{"index":{"_index":"books","_id":"1"}}"#,
    ));

    assert!(matches!(
        err,
        BulkParseError::MissingSource {
            line: 1,
            action: "index"
        }
    ));
}

#[test]
fn bulk_rejects_unknown_action() {
    let err = expect_single_error(parse_bulk_ndjson(
        r#"{"noop":{"_index":"books","_id":"1"}}"#,
    ));

    assert!(matches!(
        err,
        BulkParseError::UnknownAction {
            line: 1,
            action
        } if action == "noop"
    ));
}

#[test]
fn bulk_rejects_invalid_action_json() {
    let err = expect_single_error(parse_bulk_ndjson(r#"{"index":{"_index":"books""#));

    assert!(matches!(
        err,
        BulkParseError::InvalidActionJson { line: 1, .. }
    ));
}

#[test]
fn bulk_delete_does_not_consume_following_source_line() {
    let body = r#"{"delete":{"_index":"books","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"index":{"_index":"books","_id":"2"}}"#
        + "\n"
        + r#"{"title":"Rust Search"}"#;

    let operations = expect_operations(parse_bulk_ndjson(&body));

    assert_eq!(
        operations,
        vec![
            BulkOperation::Delete {
                index: Some("books".to_owned()),
                id: Some("1".to_owned()),
            },
            BulkOperation::Index {
                index: Some("books".to_owned()),
                id: Some("2".to_owned()),
                source: json!({"title": "Rust Search"}),
            },
        ]
    );
}

#[test]
fn bulk_parses_create_and_update_operations() {
    let body = r#"{"create":{"_index":"books","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"title":"Rust Search"}"#
        + "\n"
        + r#"{"update":{"_index":"books","_id":"2"}}"#
        + "\n"
        + r#"{"doc":{"title":"Rust Search 2"}}"#;
    let operations = expect_operations(parse_bulk_ndjson(&body));

    assert_eq!(
        operations,
        vec![
            BulkOperation::Create {
                index: Some("books".to_owned()),
                id: Some("1".to_owned()),
                source: json!({"title": "Rust Search"}),
            },
            BulkOperation::Update {
                index: Some("books".to_owned()),
                id: Some("2".to_owned()),
                source: json!({"doc": {"title": "Rust Search 2"}}),
            },
        ]
    );
}

#[test]
fn bulk_rejects_non_json_source_line() {
    let body = r#"{"update":{"_index":"books","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"doc":"missing end quote}"#;
    let err = expect_single_error(parse_bulk_ndjson(&body));

    assert!(matches!(
        err,
        BulkParseError::InvalidSourceJson { line: 2, .. }
    ));
}

#[test]
fn bulk_rejects_non_object_source_line() {
    let body = "{\"create\":{\"_index\":\"books\",\"_id\":\"1\"}}\n123\n".to_owned();
    let err = expect_single_error(parse_bulk_ndjson(&body));

    assert!(matches!(
        err,
        BulkParseError::SourceNotObject {
            line: 2,
            action: "create"
        }
    ));
}

#[test]
fn bulk_rejects_invalid_index_name_in_metadata() {
    let body = r#"{"index":{"_index":"InvalidIndex","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"title":"Rust Search"}"#;
    let err = expect_single_error(parse_bulk_ndjson(&body));

    assert!(matches!(
        err,
        BulkParseError::InvalidAction {
            line: 1,
            reason
        } if reason == "_index metadata must be a valid index name"
    ));
}

/// The action name ("index") was recovered even though its `_index`
/// metadata was invalid, so the parser knows a source line was expected
/// and discards the orphaned `{"title":"Rust Search"}` line above instead
/// of misreading it as a fresh (and then unknown) action line. Without
/// that resync, this body would surface two errors instead of one.
#[test]
fn bulk_invalid_metadata_resyncs_by_discarding_its_paired_source_line() {
    let body = r#"{"index":{"_index":"InvalidIndex","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"title":"Rust Search"}"#;
    let items = parse_bulk_ndjson(&body);

    assert_eq!(
        items.len(),
        1,
        "the orphaned source line must not surface as a second error item"
    );
}

#[test]
fn bulk_builds_opensearch_compatible_response_from_classic_fixture() {
    let body = include_str!("../../../tests/opensearch_compat/bulk/classic_bulk.ndjson");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/bulk/classic_bulk_response.json"
    ))
    .expect("response fixture should be valid json");
    let operations = parse_bulk_ndjson(body);

    let response = build_bulk_response(&operations, 7);

    assert_eq!(
        response,
        BulkResponse {
            took: 7,
            errors: false,
            items: response.items.clone(),
        }
    );
    assert_eq!(
        serde_json::to_value(response).expect("bulk response should serialize"),
        expected_response
    );
}
