use surch_core::common::{Document, FieldValue, IndexMetadata};
use surch_core::search::{FuzzyQuery, Query};
use surch_core::storage::IndexStore;

#[test]
fn fuzzy_query_matches_persisted_documents_with_transposition() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let store = IndexStore::new(temp_dir.path()).expect("create store");

    store
        .create_index(IndexMetadata::new("books"))
        .expect("create index");

    store
        .index_document(
            "books",
            Document::new("1").with_field("title", FieldValue::Text("ba".to_string())),
        )
        .expect("index doc");

    let docs = store.get_all_documents("books").expect("get docs");
    let results = FuzzyQuery::new("title", "ab").with_fuzziness(1).execute(&docs);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].doc.id, "1");
}

#[test]
fn fuzzy_query_respects_prefix_length_on_persisted_documents() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let store = IndexStore::new(temp_dir.path()).expect("create store");

    store
        .create_index(IndexMetadata::new("books"))
        .expect("create index");

    store
        .index_document(
            "books",
            Document::new("1").with_field("title", FieldValue::Text("jello".to_string())),
        )
        .expect("index doc");

    let docs = store.get_all_documents("books").expect("get docs");
    let results = FuzzyQuery::new("title", "hello")
        .with_fuzziness(1)
        .with_prefix_length(1)
        .execute(&docs);

    assert!(results.is_empty());
}
