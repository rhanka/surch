use surch_core::common::{Document, FieldValue, IndexMetadata};
use surch_core::search::{BoolQuery, MatchOperator, MatchQuery, Query, QueryType, RangeQuery, TermQuery, TermsQuery};
use surch_core::storage::IndexStore;

#[test]
fn query_dsl_core_filters_persisted_documents_end_to_end() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let store = IndexStore::new(temp_dir.path()).expect("create store");

    store
        .create_index(IndexMetadata::new("books"))
        .expect("create index");

    store
        .index_document(
            "books",
            Document::new("1")
                .with_field("title", FieldValue::Text("rust search".to_string()))
                .with_field("status", FieldValue::Keyword("published".to_string()))
                .with_field("year", FieldValue::Integer(2024)),
        )
        .expect("index doc 1");

    store
        .index_document(
            "books",
            Document::new("2")
                .with_field("title", FieldValue::Text("search engine".to_string()))
                .with_field("status", FieldValue::Keyword("published".to_string()))
                .with_field("year", FieldValue::Integer(2023)),
        )
        .expect("index doc 2");

    store
        .index_document(
            "books",
            Document::new("3")
                .with_field("title", FieldValue::Text("rust cookbook".to_string()))
                .with_field("status", FieldValue::Keyword("draft".to_string()))
                .with_field("year", FieldValue::Integer(2025)),
        )
        .expect("index doc 3");

    let docs = store.get_all_documents("books").expect("get docs");

    let query = BoolQuery::new()
        .must(QueryType::Term(TermQuery::new("status", "published")))
        .filter(QueryType::Range(
            RangeQuery::new("year").lte(surch_core::search::Bound::Integer(2024)),
        ))
        .should(QueryType::Match(
            MatchQuery::new("title", "rust search").with_operator(MatchOperator::And),
        ));

    let results = query.execute(&docs);
    let ids: Vec<&str> = results.iter().map(|result| result.doc.id.as_str()).collect();

    assert_eq!(ids, vec!["1", "2"]);
}

#[test]
fn terms_query_matches_multiple_persisted_values_end_to_end() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let store = IndexStore::new(temp_dir.path()).expect("create store");

    store
        .create_index(IndexMetadata::new("books"))
        .expect("create index");

    for (id, status) in [("1", "published"), ("2", "draft"), ("3", "archived")] {
        store
            .index_document(
                "books",
                Document::new(id)
                    .with_field("status", FieldValue::Keyword(status.to_string()))
                    .with_field("title", FieldValue::Text(format!("doc {id}"))),
            )
            .expect("index doc");
    }

    let docs = store.get_all_documents("books").expect("get docs");
    let results = TermsQuery::new(
        "status",
        vec!["published".to_string(), "archived".to_string()],
    )
    .execute(&docs);

    let mut ids: Vec<&str> = results.iter().map(|result| result.doc.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec!["1", "3"]);
}
