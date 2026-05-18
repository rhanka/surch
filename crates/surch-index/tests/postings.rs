use std::fs;
use std::path::PathBuf;

use surch_codec::postings_block::FOR_BLOCK_SIZE;
use surch_index::postings::{PostingsBuilder, PostingsError, BLOCK_SIZE};

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
fn term_dictionary_block_metas_match_postings_chunks() {
    // Insert enough postings to span three 128-doc blocks (full + full +
    // partial) and check the BlockMeta Vec is correctly sized and aligned
    // with `postings.chunks(BLOCK_SIZE)` — each meta's `min_doc_id` /
    // `max_doc_id` is the first / last doc_id of the corresponding chunk
    // in the (ascending-doc_id) posting list.
    let mut builder = PostingsBuilder::new();
    let total_docs: u32 = (BLOCK_SIZE as u32) * 2 + 17;
    // Push doc_ids in scrambled order so the test exercises the
    // ascending-doc_id sort performed by `PostingsBuilder::build()`.
    for offset in 0..total_docs {
        let doc_id = (offset * 7 + 3) % total_docs;
        builder
            .add("body", "hit", doc_id, vec![0])
            .expect("hit posting");
    }

    let dictionary = builder.build();
    let postings: Vec<_> = dictionary
        .postings("body", "hit")
        .expect("hit postings")
        .collect();
    assert_eq!(postings.len(), total_docs as usize);

    let metas = dictionary
        .block_metas("body", "hit")
        .expect("hit block_metas");
    assert_eq!(
        metas.len(),
        3,
        "two full blocks + one partial trailing block"
    );

    for (chunk, meta) in postings.chunks(BLOCK_SIZE).zip(metas.iter()) {
        assert_eq!(meta.min_doc_id, chunk.first().expect("chunk").doc_id);
        assert_eq!(meta.max_doc_id, chunk.last().expect("chunk").doc_id);
        // Single-position postings -> freq 1 everywhere.
        assert_eq!(meta.max_term_freq, 1);
    }

    // Unknown field / term yields `None`, consistent with `postings()`.
    assert!(dictionary.block_metas("body", "miss").is_none());
    assert!(dictionary.block_metas("missing", "hit").is_none());
}

#[test]
fn term_dictionary_block_metas_follow_codec_block_size() {
    assert_eq!(
        BLOCK_SIZE, FOR_BLOCK_SIZE,
        "surch-index block metas must stay aligned with the codec FoR block size"
    );

    let source = include_str!("../src/postings.rs");
    assert!(
        !source.contains("pub const BLOCK_SIZE: usize = 128;"),
        "surch-index must not duplicate the codec block size literal"
    );

    let mut builder = PostingsBuilder::new();
    let total_docs = FOR_BLOCK_SIZE * 2 + 17;
    for doc_id in 0..total_docs {
        let positions = if doc_id == FOR_BLOCK_SIZE + 3 {
            vec![0, 1, 2, 3]
        } else {
            vec![0]
        };
        builder
            .add("body", "codec-sized", doc_id as u32, positions)
            .expect("codec-sized posting");
    }

    let dictionary = builder.build();
    let postings = dictionary
        .postings("body", "codec-sized")
        .expect("codec-sized postings")
        .collect::<Vec<_>>();
    let metas = dictionary
        .block_metas("body", "codec-sized")
        .expect("codec-sized block_metas");

    let chunk_lengths = postings
        .chunks(FOR_BLOCK_SIZE)
        .map(<[_]>::len)
        .collect::<Vec<_>>();
    assert_eq!(chunk_lengths, vec![FOR_BLOCK_SIZE, FOR_BLOCK_SIZE, 17]);
    assert_eq!(metas.len(), chunk_lengths.len());
    assert_eq!(metas[0].min_doc_id, 0);
    assert_eq!(metas[0].max_doc_id, (FOR_BLOCK_SIZE - 1) as u32);
    assert_eq!(metas[0].max_term_freq, 1);
    assert_eq!(metas[1].min_doc_id, FOR_BLOCK_SIZE as u32);
    assert_eq!(metas[1].max_doc_id, (FOR_BLOCK_SIZE * 2 - 1) as u32);
    assert_eq!(metas[1].max_term_freq, 4);
    assert_eq!(metas[2].min_doc_id, (FOR_BLOCK_SIZE * 2) as u32);
    assert_eq!(metas[2].max_doc_id, (total_docs - 1) as u32);
    assert_eq!(metas[2].max_term_freq, 1);
}

#[test]
fn term_dictionary_block_metas_max_freq_per_block() {
    // Build a posting list where one doc per block carries an unusually
    // high term frequency, so each `BlockMeta::max_term_freq` reflects
    // that single high-frequency entry and not the surrounding 1-freq
    // postings. This is the property `maxscore_match` relies on for
    // its Block-Max WAND upper bound.
    let mut builder = PostingsBuilder::new();
    let total_docs: u32 = (BLOCK_SIZE as u32) * 2 + 30;
    for doc_id in 0..total_docs {
        // First block (0..=127): the high-freq doc is doc_id == 10,
        // freq = 9 (positions 0..=8).
        // Second block (128..=255): the high-freq doc is doc_id == 200,
        // freq = 5.
        // Third block (256..): the high-freq doc is doc_id == 270,
        // freq = 7.
        let positions = if doc_id == 10 {
            (0..9).collect::<Vec<_>>()
        } else if doc_id == 200 {
            (0..5).collect::<Vec<_>>()
        } else if doc_id == 270 {
            (0..7).collect::<Vec<_>>()
        } else {
            vec![0]
        };
        builder
            .add("body", "varied", doc_id, positions)
            .expect("varied posting");
    }

    let dictionary = builder.build();
    let metas = dictionary
        .block_metas("body", "varied")
        .expect("varied block_metas");
    assert_eq!(metas.len(), 3);

    // Block 0 covers doc_ids 0..=127 inclusive; max freq is 9 (doc 10).
    assert_eq!(metas[0].min_doc_id, 0);
    assert_eq!(metas[0].max_doc_id, 127);
    assert_eq!(metas[0].max_term_freq, 9);

    // Block 1 covers doc_ids 128..=255 inclusive; max freq is 5 (doc 200).
    assert_eq!(metas[1].min_doc_id, 128);
    assert_eq!(metas[1].max_doc_id, 255);
    assert_eq!(metas[1].max_term_freq, 5);

    // Block 2 covers doc_ids 256..=285 inclusive (partial); max freq is 7 (doc 270).
    assert_eq!(metas[2].min_doc_id, 256);
    assert_eq!(metas[2].max_doc_id, total_docs - 1);
    assert_eq!(metas[2].max_term_freq, 7);
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
