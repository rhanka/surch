use surch_index::document_index::{DocumentIndex, DocumentIndexError};
use surch_index::stored_fields::StoredValue;

const DOCUMENT_INDEX_CLASSIC: &str =
    include_str!("../../../tests/lucene_parity/index/document_index_classic.tsv");

#[test]
fn document_index_stores_documents_and_indexes_terms_with_positions_by_field() {
    let mut index = DocumentIndex::new();

    for row in fixture_rows() {
        index
            .add_document(row.doc_id, [("title", row.title), ("body", row.body)])
            .expect("add fixture document");
    }

    assert_eq!(index.doc_ids(), vec![0, 2]);
    assert_eq!(index.live_doc_count(), 2);
    assert_eq!(
        index
            .stored_document(0)
            .and_then(|document| document.get("title")),
        Some(&StoredValue::String("Surch Search".to_owned()))
    );
    assert_eq!(
        index
            .stored_document(2)
            .map(|document| document.field_names()),
        Some(vec!["body", "title"])
    );

    assert_eq!(
        index.terms("body").collect::<Vec<_>>(),
        ["engine", "fast", "rust", "search"]
    );
    assert_eq!(
        index.terms("title").collect::<Vec<_>>(),
        ["index", "search", "surch"]
    );

    let body_search = index
        .postings("body", "search")
        .expect("body/search postings")
        .collect::<Vec<_>>();
    assert_eq!(body_search.len(), 2);
    assert_eq!(body_search[0].doc_id, 0);
    assert_eq!(body_search[0].freq, 2);
    assert_eq!(body_search[0].positions, [0, 3]);
    assert_eq!(body_search[1].doc_id, 2);
    assert_eq!(body_search[1].freq, 1);
    assert_eq!(body_search[1].positions, [3]);

    let title_surch = index
        .postings("title", "surch")
        .expect("title/surch postings")
        .collect::<Vec<_>>();
    assert_eq!(title_surch.len(), 1);
    assert_eq!(title_surch[0].doc_id, 0);
    assert_eq!(title_surch[0].positions, [0]);
}

#[test]
fn document_index_rejects_duplicate_doc_ids() {
    let mut index = DocumentIndex::new();
    index
        .add_document(7, [("title", "First")])
        .expect("first document");

    let err = index
        .add_document(7, [("title", "Duplicate")])
        .expect_err("duplicate doc_id rejected");

    assert_eq!(err, DocumentIndexError::DuplicateDocId { doc_id: 7 });
}

#[test]
fn document_index_rejects_empty_field_names() {
    let mut index = DocumentIndex::new();

    let err = index
        .add_document(1, [("", "missing name")])
        .expect_err("empty field rejected");
    assert_eq!(err, DocumentIndexError::EmptyFieldName);

    let err = index
        .add_document(2, [("   ", "blank name")])
        .expect_err("blank field rejected");
    assert_eq!(err, DocumentIndexError::EmptyFieldName);
}

#[test]
fn document_index_stores_empty_text_without_postings() {
    let mut index = DocumentIndex::new();

    index
        .add_document(1, [("title", ""), ("body", "Rust")])
        .expect("empty text is storable");

    assert_eq!(
        index
            .stored_document(1)
            .and_then(|document| document.get("title")),
        Some(&StoredValue::String(String::new()))
    );
    assert_eq!(index.terms("title").collect::<Vec<_>>(), Vec::<&str>::new());
    assert_eq!(index.terms("body").collect::<Vec<_>>(), ["rust"]);
    assert!(index.postings("title", "").is_none());
}

struct FixtureRow {
    doc_id: u32,
    title: String,
    body: String,
}

fn fixture_rows() -> Vec<FixtureRow> {
    DOCUMENT_INDEX_CLASSIC
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(parse_fixture_row)
        .collect()
}

fn parse_fixture_row(line: &str) -> FixtureRow {
    let mut columns = line.split('\t');
    let doc_id = columns
        .next()
        .expect("doc_id")
        .parse()
        .expect("doc_id is u32");
    let title = columns.next().expect("title").to_owned();
    let body = columns.next().expect("body").to_owned();
    assert_eq!(columns.next(), None, "unexpected extra fixture columns");

    FixtureRow {
        doc_id,
        title,
        body,
    }
}
