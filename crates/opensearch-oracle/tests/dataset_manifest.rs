use opensearch_oracle::dataset::{DatasetManifest, DatasetOperationKind, ManifestValidationError};

#[test]
fn parses_products_basic_fixture() {
    let manifest = DatasetManifest::from_json_str(include_str!(
        "../../../tests/opensearch_compat/oracle/datasets/products_basic.json"
    ))
    .expect("fixture manifest should parse");

    assert_eq!(manifest.name, "products_basic");
    assert_eq!(manifest.description, "Basic product indexing dataset");
    assert_eq!(manifest.operations.len(), 3);

    assert_eq!(
        manifest.operations[0].kind,
        DatasetOperationKind::CreateIndex
    );
    assert_eq!(manifest.operations[0].path, "/products_basic");
    assert_eq!(manifest.operations[0].expected_status, Some(200));

    assert_eq!(manifest.operations[1].kind, DatasetOperationKind::Bulk);
    assert_eq!(manifest.operations[1].path, "/_bulk");
    assert_eq!(
        manifest.operations[1].body.as_deref(),
        Some("products_basic.ndjson")
    );

    assert_eq!(manifest.operations[2].kind, DatasetOperationKind::Refresh);
    assert_eq!(manifest.operations[2].path, "/products_basic/_refresh");
}

#[test]
fn rejects_empty_manifest_name() {
    let err = DatasetManifest::from_json_str(
        r#"{
            "name": " ",
            "description": "Invalid",
            "operations": [{"kind": "refresh", "path": "/idx/_refresh"}]
        }"#,
    )
    .expect_err("blank manifest name should be invalid");

    assert!(matches!(err, ManifestValidationError::EmptyName));
}

#[test]
fn rejects_manifest_without_operations() {
    let err = DatasetManifest::from_json_str(
        r#"{
            "name": "empty",
            "description": "Invalid",
            "operations": []
        }"#,
    )
    .expect_err("manifest must contain operations");

    assert!(matches!(err, ManifestValidationError::NoOperations));
}

#[test]
fn rejects_operation_path_without_leading_slash() {
    let err = DatasetManifest::from_json_str(
        r#"{
            "name": "bad_path",
            "description": "Invalid",
            "operations": [{"kind": "refresh", "path": "idx/_refresh"}]
        }"#,
    )
    .expect_err("operation paths must start with slash");

    assert!(matches!(
        err,
        ManifestValidationError::InvalidOperationPath { index: 0, .. }
    ));
}

#[test]
fn rejects_blank_body_path_when_present() {
    let err = DatasetManifest::from_json_str(
        r#"{
            "name": "bad_body",
            "description": "Invalid",
            "operations": [{"kind": "bulk", "path": "/_bulk", "body": " "}]
        }"#,
    )
    .expect_err("blank body path should be invalid");

    assert!(matches!(
        err,
        ManifestValidationError::InvalidBodyPath { index: 0 }
    ));
}

#[test]
fn rejects_expected_status_outside_http_range() {
    let err = DatasetManifest::from_json_str(
        r#"{
            "name": "bad_status",
            "description": "Invalid",
            "operations": [{"kind": "refresh", "path": "/idx/_refresh", "expected_status": 99}]
        }"#,
    )
    .expect_err("expected status must be an HTTP status code");

    assert!(matches!(
        err,
        ManifestValidationError::InvalidExpectedStatus {
            index: 0,
            status: 99
        }
    ));
}

#[test]
fn rejects_unknown_operation_kind() {
    let err = DatasetManifest::from_json_str(
        r#"{
            "name": "bad_kind",
            "description": "Invalid",
            "operations": [{"kind": "search", "path": "/idx/_search"}]
        }"#,
    )
    .expect_err("unknown operation kind should fail deserialization");

    assert!(matches!(err, ManifestValidationError::Json(_)));
}
