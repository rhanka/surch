use surch_index::mapping::{AnalyzerName, FieldMapping, FieldType, IndexMapping, MappingError};

#[test]
fn index_mapping_from_properties_value_builds_fields_with_text_default_and_analyzer() {
    let properties = serde_json::json!({
        "title": { "type": "text", "analyzer": "whitespace" },
        "year": { "type": "integer" }
    });

    let mapping = IndexMapping::from_properties_value(&properties).expect("mapping should parse");
    let title = mapping.field("title").expect("title field exists");
    let year = mapping.field("year").expect("year field exists");

    assert_eq!(title.field_type, FieldType::Text);
    assert_eq!(title.analyzer, Some(AnalyzerName::Whitespace));
    assert_eq!(year.field_type, FieldType::Integer);
    assert_eq!(year.analyzer, None);
}

#[test]
fn index_mapping_infer_from_document_indexes_scalar_and_array_types() {
    let document = serde_json::json!({
        "title": "Rust Search",
        "stock": 10,
        "meta": { "flag": true },
        "tags": ["a", "b"]
    });

    let mapping = IndexMapping::infer_from_document(&document);
    let title = mapping.field("title").expect("title exists");
    let stock = mapping.field("stock").expect("stock exists");
    let tags = mapping.field("tags").expect("tags exists");

    assert_eq!(title.field_type, FieldType::Text);
    assert_eq!(stock.field_type, FieldType::Integer);
    assert_eq!(tags.field_type, FieldType::Text);
}

#[test]
fn index_mapping_rejects_unsupported_field_types() {
    let properties = serde_json::json!({
        "title": { "type": "text", "analyzer": "magic" }
    });

    let error = IndexMapping::from_properties_value(&properties).expect_err("unsupported analyzer");
    assert!(
        matches!(error, MappingError::UnsupportedAnalyzer { .. }),
        "unexpected mapping error: {error:?}"
    );
}

#[test]
fn index_mapping_rejects_analyzer_on_non_text_field() {
    let properties = serde_json::json!({
        "year": { "type": "integer", "analyzer": "simple" }
    });

    let error = IndexMapping::from_properties_value(&properties).expect_err("analyzer on non-text");
    assert!(
        matches!(error, MappingError::AnalyzerNotSupported { .. }),
        "unexpected mapping error: {error:?}"
    );
}

#[test]
fn index_mapping_adds_default_mapping_for_new_fields() {
    let mut mapping = IndexMapping::default();
    mapping.set_field_mapping(
        "title",
        FieldMapping::new(FieldType::Text, Some(AnalyzerName::Standard)),
    );

    let document = serde_json::json!({
        "title": "one",
        "year": 2026
    });

    mapping.ensure_fields(&document);
    assert!(mapping.has_field("year"));
    assert_eq!(
        mapping.field("year").expect("year exists").field_type,
        FieldType::Integer
    );
}
