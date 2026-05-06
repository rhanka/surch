use std::path::PathBuf;

use opensearch_oracle::files::{FixtureFileError, FixtureRoot};
use serde_json::Value;

fn fixture_root() -> FixtureRoot {
    FixtureRoot::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/opensearch_compat/oracle/files"),
    )
}

#[test]
fn reads_text_fixture() {
    let root = fixture_root();

    let text = root
        .read_text("notes/simple.txt")
        .expect("text fixture should be readable");

    assert_eq!(text, "OpenSearch oracle fixture\n");
}

#[test]
fn reads_json_fixture() {
    let root = fixture_root();

    let json: Value = root
        .read_json("json/simple.json")
        .expect("json fixture should be readable");

    assert_eq!(json["name"], "products_basic");
    assert_eq!(json["enabled"], true);
}

#[test]
fn rejects_parent_directory_traversal() {
    let root = fixture_root();

    let err = root
        .resolve_relative("../datasets/products_basic.json")
        .expect_err("parent traversal must be rejected");

    assert!(matches!(err, FixtureFileError::InvalidPath { .. }));
}

#[test]
fn rejects_absolute_paths() {
    let root = fixture_root();

    let err = root
        .resolve_relative("/etc/passwd")
        .expect_err("absolute paths must be rejected");

    assert!(matches!(err, FixtureFileError::InvalidPath { .. }));
}

#[test]
fn rejects_invalid_json_fixture() {
    let root = fixture_root();

    let err = root
        .read_json::<Value>("json/invalid.json")
        .expect_err("invalid JSON should surface as JSON error");

    assert!(matches!(err, FixtureFileError::Json { .. }));
}
