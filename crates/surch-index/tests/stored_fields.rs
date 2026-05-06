use surch_index::stored_fields::{
    StoredDocument, StoredFieldsError, StoredFieldsWriter, StoredValue,
};

const STORED_FIELDS_CLASSIC: &str =
    include_str!("../../../tests/lucene_parity/index/stored_fields_classic.tsv");

#[test]
fn stored_fields_round_trip_documents_from_classic_fixture() {
    let mut writer = StoredFieldsWriter::new();

    for row in fixture_rows() {
        writer
            .add_document(
                row.doc_id,
                StoredDocument::from_fields([
                    ("body", StoredValue::String(row.body)),
                    ("title", StoredValue::String(row.title)),
                    ("views", StoredValue::Number(row.views)),
                ]),
            )
            .expect("add stored document");
    }

    let reader = writer.finish();

    assert_eq!(reader.doc_ids(), vec![0, 3]);
    assert_eq!(
        reader.get(0).expect("doc 0").field_names(),
        vec!["body", "title", "views"]
    );
    assert_eq!(
        reader.get(3).and_then(|doc| doc.get("title")),
        Some(&StoredValue::String("Surch Query Planning".to_owned()))
    );
    assert_eq!(
        reader.get(3).and_then(|doc| doc.get("views")),
        Some(&StoredValue::Number(7))
    );
    assert_eq!(reader.get(9), None);
}

#[test]
fn stored_fields_support_scalar_values() {
    let mut document = StoredDocument::new();
    document
        .insert("title", StoredValue::String("Scalar document".to_owned()))
        .expect("title");
    document
        .insert("views", StoredValue::Number(42))
        .expect("views");
    document
        .insert("published", StoredValue::Bool(true))
        .expect("published");
    document
        .insert("deleted_at", StoredValue::Null)
        .expect("deleted_at");

    let mut writer = StoredFieldsWriter::new();
    writer.add_document(5, document).expect("add document");
    let reader = writer.finish();
    let stored = reader.get(5).expect("doc 5");

    assert_eq!(
        stored.get("title"),
        Some(&StoredValue::String("Scalar document".to_owned()))
    );
    assert_eq!(stored.get("views"), Some(&StoredValue::Number(42)));
    assert_eq!(stored.get("published"), Some(&StoredValue::Bool(true)));
    assert_eq!(stored.get("deleted_at"), Some(&StoredValue::Null));
}

#[test]
fn stored_fields_reject_duplicate_doc_ids() {
    let mut writer = StoredFieldsWriter::new();
    writer
        .add_document(
            1,
            StoredDocument::from_fields([("title", StoredValue::String("one".to_owned()))]),
        )
        .expect("first doc");

    let err = writer
        .add_document(
            1,
            StoredDocument::from_fields([("title", StoredValue::String("duplicate".to_owned()))]),
        )
        .expect_err("duplicate doc id rejected");

    assert!(matches!(
        err,
        StoredFieldsError::DuplicateDocId { doc_id: 1 }
    ));
}

#[test]
fn stored_fields_reject_empty_field_names() {
    let mut document = StoredDocument::new();

    let err = document
        .insert("", StoredValue::String("missing name".to_owned()))
        .expect_err("empty field name rejected");

    assert!(matches!(err, StoredFieldsError::EmptyFieldName));
}

struct FixtureRow {
    doc_id: u32,
    title: String,
    body: String,
    views: i64,
}

fn fixture_rows() -> Vec<FixtureRow> {
    STORED_FIELDS_CLASSIC
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
    let views = columns
        .next()
        .expect("views")
        .parse()
        .expect("views is i64");
    assert_eq!(columns.next(), None, "unexpected extra fixture columns");

    FixtureRow {
        doc_id,
        title,
        body,
        views,
    }
}
