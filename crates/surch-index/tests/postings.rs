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
        vec!["rust".to_string(), "search".to_string()]
    );
    assert_eq!(
        dictionary.terms("title").collect::<Vec<_>>(),
        vec!["surch".to_string()]
    );
    assert_eq!(
        dictionary.terms("missing").collect::<Vec<_>>(),
        Vec::<String>::new()
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
        vec![
            "engine".to_string(),
            "rust".to_string(),
            "search".to_string()
        ]
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

#[test]
fn term_dictionary_fst_lookup_returns_postings_by_term() {
    // Exercise the FST lookup path: insert several terms with prefix
    // overlap (the FST shares the "rue " prefix), then fetch each
    // posting list and check the FST round-trip is lossless. Also
    // assert that a missing term yields `None` — this is the contract
    // every BM25/BoolMust caller depends on.
    let mut builder = PostingsBuilder::new();
    builder
        .add("address", "rue de la paix", 1, vec![0, 1, 2, 3])
        .expect("address/rue de la paix");
    builder
        .add("address", "rue de la liberte", 2, vec![0, 1, 2, 3])
        .expect("address/rue de la liberte");
    builder
        .add("address", "rue mozart", 3, vec![0, 1])
        .expect("address/rue mozart");
    builder
        .add("address", "avenue victor hugo", 4, vec![0, 1, 2])
        .expect("address/avenue victor hugo");

    let dictionary = builder.build();

    let paix = dictionary
        .postings("address", "rue de la paix")
        .expect("rue de la paix postings")
        .collect::<Vec<_>>();
    assert_eq!(paix.len(), 1);
    assert_eq!(paix[0].doc_id, 1);
    assert_eq!(paix[0].positions, [0, 1, 2, 3]);

    let mozart = dictionary
        .postings("address", "rue mozart")
        .expect("rue mozart postings")
        .collect::<Vec<_>>();
    assert_eq!(mozart.len(), 1);
    assert_eq!(mozart[0].doc_id, 3);

    let avenue = dictionary
        .postings("address", "avenue victor hugo")
        .expect("avenue victor hugo postings")
        .collect::<Vec<_>>();
    assert_eq!(avenue.len(), 1);
    assert_eq!(avenue[0].doc_id, 4);

    assert!(dictionary
        .postings("address", "rue de la republique")
        .is_none());
    assert!(dictionary.postings("missing_field", "rue mozart").is_none());
}

#[test]
fn term_dictionary_fst_terms_returns_lex_sorted() {
    // Insert terms in non-lexicographic order; the FST builder
    // contract requires lex order on the way in, so this test
    // exercises the `PostingsBuilder::build()` sort.
    let mut builder = PostingsBuilder::new();
    builder.add("body", "zebra", 1, vec![0]).expect("zebra");
    builder.add("body", "alpha", 2, vec![0]).expect("alpha");
    builder.add("body", "mango", 3, vec![0]).expect("mango");
    builder.add("body", "alfa", 4, vec![0]).expect("alfa");
    builder.add("body", "beta", 5, vec![0]).expect("beta");

    let dictionary = builder.build();

    let terms = dictionary.terms("body").collect::<Vec<_>>();
    assert_eq!(
        terms,
        vec![
            "alfa".to_string(),
            "alpha".to_string(),
            "beta".to_string(),
            "mango".to_string(),
            "zebra".to_string(),
        ]
    );

    // The empty-field case must yield an empty iterator (not panic
    // and not return a single empty string).
    let missing = dictionary.terms("nope").collect::<Vec<_>>();
    assert!(missing.is_empty());
}
