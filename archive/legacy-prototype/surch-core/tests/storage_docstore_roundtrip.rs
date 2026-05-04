use surch_core::common::{Document, FieldValue, IndexMetadata};
use surch_core::storage::IndexStore;

#[test]
fn index_store_round_trips_document_from_persisted_segment() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let store = IndexStore::new(temp_dir.path()).expect("create store");

    store
        .create_index(IndexMetadata::new("books"))
        .expect("create index");

    store
        .index_document(
            "books",
            Document::new("doc-1")
                .with_field("title", FieldValue::Text("Roundtrip".to_string()))
                .with_field("year", FieldValue::Integer(2024)),
        )
        .expect("index document");

    let doc = store
        .get_document("books", "doc-1")
        .expect("get document result")
        .expect("document should exist");

    assert_eq!(doc.get_text("title"), Some("Roundtrip".to_string()));
    assert_eq!(
        doc.get_field("year").and_then(FieldValue::as_i64),
        Some(2024)
    );
}
