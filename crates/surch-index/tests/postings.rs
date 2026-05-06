use std::fs;
use std::path::PathBuf;

use surch_index::postings::{PostingsBuilder, PostingsError};

#[test]
fn postings_builder_indexes_terms_and_postings_in_deterministic_order() {
    let mut builder = PostingsBuilder::new();

    builder
        .add("body", "search", 7, vec![4, 9])
        .expect("search in doc 7");
    builder
        .add("body", "rust", 3, vec![1])
        .expect("rust in doc 3");
    builder
        .add("body", "search", 2, vec![0, 5, 8])
        .expect("search in doc 2");
    builder
        .add("title", "surch", 1, vec![0])
        .expect("title term");

    let dictionary = builder.build();

    assert_eq!(
        dictionary.terms("body").collect::<Vec<_>>(),
        ["rust", "search"]
    );
    assert_eq!(dictionary.terms("title").collect::<Vec<_>>(), ["surch"]);
    assert_eq!(
        dictionary.terms("missing").collect::<Vec<_>>(),
        Vec::<&str>::new()
    );

    let search_postings = dictionary
        .postings("body", "search")
        .expect("body/search postings")
        .collect::<Vec<_>>();
    assert_eq!(search_postings.len(), 2);
    assert_eq!(search_postings[0].doc_id, 2);
    assert_eq!(search_postings[0].freq, 3);
    assert_eq!(search_postings[0].positions, [0, 5, 8]);
    assert_eq!(search_postings[1].doc_id, 7);
    assert_eq!(search_postings[1].freq, 2);
    assert_eq!(search_postings[1].positions, [4, 9]);
}

#[test]
fn postings_builder_derives_frequency_one_when_positions_are_absent() {
    let mut builder = PostingsBuilder::new();

    builder
        .add("body", "stored", 4, Vec::new())
        .expect("docs-only posting");

    let dictionary = builder.build();
    let posting = dictionary
        .postings("body", "stored")
        .expect("body/stored postings")
        .next()
        .expect("posting");

    assert_eq!(posting.doc_id, 4);
    assert_eq!(posting.freq, 1);
    assert!(posting.positions.is_empty());
}

#[test]
fn postings_builder_rejects_empty_field_and_term() {
    let mut builder = PostingsBuilder::new();

    let err = builder
        .add("", "search", 1, vec![0])
        .expect_err("empty field rejected");
    assert!(matches!(err, PostingsError::EmptyField));

    let err = builder
        .add("body", "", 1, vec![0])
        .expect_err("empty term rejected");
    assert!(matches!(err, PostingsError::EmptyTerm));
}

#[test]
fn postings_lucene_parity_fixture_matches_classic_shape() {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/lucene_parity/index/postings_classic.tsv");
    let fixture = fs::read_to_string(fixture_path).expect("fixture");
    let mut builder = PostingsBuilder::new();

    for line in fixture.lines().skip(1) {
        let columns = line.split('\t').collect::<Vec<_>>();
        assert_eq!(columns.len(), 5, "fixture row has five columns: {line}");

        let doc_id = columns[2].parse::<u32>().expect("doc_id");
        let expected_freq = columns[3].parse::<u32>().expect("freq");
        let positions = columns[4]
            .split(',')
            .filter(|position| !position.is_empty())
            .map(|position| position.parse::<u32>().expect("position"))
            .collect::<Vec<_>>();

        builder
            .add(columns[0], columns[1], doc_id, positions)
            .expect("fixture posting");

        assert!(
            expected_freq > 0,
            "fixture freq documents docs-only semantics"
        );
    }

    let dictionary = builder.build();

    assert_eq!(
        dictionary.terms("body").collect::<Vec<_>>(),
        ["engine", "rust", "search"]
    );

    let search_postings = dictionary
        .postings("body", "search")
        .expect("body/search postings")
        .collect::<Vec<_>>();
    assert_eq!(search_postings.len(), 2);
    assert_eq!(search_postings[0].doc_id, 1);
    assert_eq!(search_postings[0].freq, 2);
    assert_eq!(search_postings[0].positions, [0, 3]);
    assert_eq!(search_postings[1].doc_id, 3);
    assert_eq!(search_postings[1].freq, 1);
    assert_eq!(search_postings[1].positions, [5]);
}
