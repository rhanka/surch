use serde_json::json;
use surch_api::{
    bulk::{build_bulk_response, BulkResponse},
    parse_bulk_ndjson, BulkOperation, BulkParseError,
};

#[test]
fn bulk_parses_index_and_delete_operations_in_order() {
    let body = r#"{"index":{"_index":"books","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"title":"Rust Search"}"#
        + "\n"
        + r#"{"delete":{"_index":"books","_id":"2"}}"#
        + "\n";
    let operations = parse_bulk_ndjson(&body).expect("bulk request should parse");

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
    let operations = parse_bulk_ndjson(&body).expect("final newline is optional");

    assert_eq!(operations.len(), 1);
}

#[test]
fn bulk_rejects_source_required_action_without_source_line() {
    let err = parse_bulk_ndjson(r#"{"index":{"_index":"books","_id":"1"}}"#)
        .expect_err("index action requires source line");

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
    let err = parse_bulk_ndjson(r#"{"noop":{"_index":"books","_id":"1"}}"#)
        .expect_err("unknown bulk action should be rejected");

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
    let err = parse_bulk_ndjson(r#"{"index":{"_index":"books""#)
        .expect_err("invalid action json should be rejected");

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

    let operations = parse_bulk_ndjson(&body).expect("delete should not consume a source line");

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
    let operations = parse_bulk_ndjson(&body).expect("create and update should parse");

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
    let err = parse_bulk_ndjson(&body).expect_err("invalid source json should be rejected");

    assert!(matches!(
        err,
        BulkParseError::InvalidSourceJson { line: 2, .. }
    ));
}

#[test]
fn bulk_rejects_invalid_index_name_in_metadata() {
    let body = r#"{"index":{"_index":"InvalidIndex","_id":"1"}}"#.to_owned()
        + "\n"
        + r#"{"title":"Rust Search"}"#;
    let err =
        parse_bulk_ndjson(&body).expect_err("bulk action with invalid index should be rejected");

    assert!(matches!(
        err,
        BulkParseError::InvalidAction {
            line: 1,
            reason
        } if reason == "_index metadata must be a valid index name"
    ));
}

#[test]
fn bulk_builds_opensearch_compatible_response_from_classic_fixture() {
    let body = include_str!("../../../tests/opensearch_compat/bulk/classic_bulk.ndjson");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/bulk/classic_bulk_response.json"
    ))
    .expect("response fixture should be valid json");
    let operations = parse_bulk_ndjson(body).expect("classic bulk fixture should parse");

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
