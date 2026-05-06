use surch_index::live_docs::{LiveDocs, LiveDocsError};

const LIVE_DOCS_CLASSIC: &str =
    include_str!("../../../tests/lucene_parity/index/live_docs_classic.tsv");

#[test]
fn live_docs_starts_with_every_doc_live() {
    let live_docs = LiveDocs::new(4);

    assert_eq!(live_docs.max_doc(), 4);
    assert_eq!(live_docs.live_count(), 4);
    assert_eq!(live_docs.deleted_count(), 0);
    assert_eq!(
        live_docs.iter_live_doc_ids().collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );

    for doc_id in 0..4 {
        assert_eq!(live_docs.is_live(doc_id), Ok(true));
    }
}

#[test]
fn live_docs_deletes_documents_idempotently() {
    let mut live_docs = LiveDocs::new(5);

    assert_eq!(live_docs.delete(1), Ok(true));
    assert_eq!(live_docs.delete(3), Ok(true));
    assert_eq!(live_docs.delete(1), Ok(false));

    assert_eq!(live_docs.live_count(), 3);
    assert_eq!(live_docs.deleted_count(), 2);
    assert_eq!(live_docs.is_live(1), Ok(false));
    assert_eq!(live_docs.is_live(3), Ok(false));
    assert_eq!(live_docs.iter_live_doc_ids().collect::<Vec<_>>(), [0, 2, 4]);
}

#[test]
fn live_docs_rejects_doc_ids_outside_max_doc() {
    let mut live_docs = LiveDocs::new(2);

    assert_eq!(
        live_docs.is_live(2),
        Err(LiveDocsError::DocIdOutOfRange {
            doc_id: 2,
            max_doc: 2
        })
    );
    assert_eq!(
        live_docs.delete(7),
        Err(LiveDocsError::DocIdOutOfRange {
            doc_id: 7,
            max_doc: 2
        })
    );
    assert_eq!(live_docs.live_count(), 2);
    assert_eq!(live_docs.deleted_count(), 0);
}

#[test]
fn live_docs_lucene_parity_fixture_matches_classic_shape() {
    let row = parse_fixture_row(LIVE_DOCS_CLASSIC);
    let mut live_docs = LiveDocs::new(row.max_doc);

    for doc_id in row.deleted_doc_ids {
        assert_eq!(live_docs.delete(doc_id), Ok(true));
    }

    assert_eq!(live_docs.max_doc(), row.max_doc);
    assert_eq!(
        live_docs.live_count(),
        row.expected_live_doc_ids.len() as u32
    );
    assert_eq!(
        live_docs.deleted_count(),
        row.max_doc - row.expected_live_doc_ids.len() as u32
    );
    assert_eq!(
        live_docs.iter_live_doc_ids().collect::<Vec<_>>(),
        row.expected_live_doc_ids
    );
}

struct FixtureRow {
    max_doc: u32,
    deleted_doc_ids: Vec<u32>,
    expected_live_doc_ids: Vec<u32>,
}

fn parse_fixture_row(fixture: &str) -> FixtureRow {
    let line = fixture
        .lines()
        .skip(1)
        .find(|line| !line.trim().is_empty())
        .expect("fixture row");
    let mut columns = line.split('\t');
    let max_doc = columns
        .next()
        .expect("max_doc")
        .parse()
        .expect("max_doc is u32");
    let deleted_doc_ids = parse_doc_ids(columns.next().expect("deleted_doc_ids"));
    let expected_live_doc_ids = parse_doc_ids(columns.next().expect("live_doc_ids"));
    assert_eq!(columns.next(), None, "unexpected extra fixture columns");

    FixtureRow {
        max_doc,
        deleted_doc_ids,
        expected_live_doc_ids,
    }
}

fn parse_doc_ids(value: &str) -> Vec<u32> {
    value
        .split(',')
        .filter(|doc_id| !doc_id.is_empty())
        .map(|doc_id| doc_id.parse().expect("doc_id is u32"))
        .collect()
}
