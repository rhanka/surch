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
fn index_mapping_accepts_short_and_byte_numeric_types() {
    // matchID `deces_index.yml` declares `AGE_DECES` as ES `short`; `byte`
    // is the narrower sibling. Both must parse and round-trip distinctly.
    let properties = serde_json::json!({
        "AGE_DECES": { "type": "short" },
        "tiny": { "type": "byte" }
    });

    let mapping = IndexMapping::from_properties_value(&properties).expect("mapping should parse");
    assert_eq!(
        mapping.field("AGE_DECES").expect("AGE_DECES exists").field_type,
        FieldType::Short
    );
    assert_eq!(
        mapping.field("tiny").expect("tiny exists").field_type,
        FieldType::Byte
    );
    assert_eq!(
        mapping.as_value()["properties"]["AGE_DECES"]["type"],
        serde_json::json!("short")
    );
    assert_eq!(
        mapping.as_value()["properties"]["tiny"]["type"],
        serde_json::json!("byte")
    );
}

#[test]
fn index_mapping_preserves_explicit_norms_option() {
    let properties = serde_json::json!({
        "body": { "type": "text", "norms": false }
    });

    let mapping = IndexMapping::from_properties_value(&properties).expect("mapping should parse");
    assert_eq!(
        mapping.as_value()["properties"]["body"]["norms"],
        serde_json::json!(false)
    );
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
fn index_mapping_infers_numeric_strings_as_keyword_fields() {
    let document = serde_json::json!({
        "postcode": "33000",
        "city_code": "75103",
        "label": "1 Rue Payenne 75003 Paris"
    });

    let mapping = IndexMapping::infer_from_document(&document);
    let postcode = mapping.field("postcode").expect("postcode exists");
    let city_code = mapping.field("city_code").expect("city_code exists");
    let label = mapping.field("label").expect("label exists");

    assert_eq!(postcode.field_type, FieldType::Keyword);
    assert_eq!(city_code.field_type, FieldType::Keyword);
    assert_eq!(label.field_type, FieldType::Text);
}

#[test]
fn index_mapping_accepts_custom_analyzer_name() {
    // A1/A13: a non-builtin analyzer name (e.g. a user-defined
    // `autocomplete_analyzer`) is accepted and kept verbatim as
    // `custom_analyzer`; it resolves at index/query time against the index
    // `settings.analysis` block (not in scope at field-parse time). The
    // builtin `analyzer` slot stays `None`.
    let properties = serde_json::json!({
        "title": { "type": "text", "analyzer": "autocomplete_analyzer" }
    });

    let mapping = IndexMapping::from_properties_value(&properties).expect("custom analyzer parses");
    let title = mapping.field("title").expect("title exists");
    assert_eq!(title.analyzer, None);
    assert_eq!(
        title.custom_analyzer.as_deref(),
        Some("autocomplete_analyzer")
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
