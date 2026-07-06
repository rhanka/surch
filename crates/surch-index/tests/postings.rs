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
    assert_eq!(search_postings[1].doc_id, 7);
    assert_eq!(search_postings[1].freq, 2);
}

#[test]
fn skip_iter_advance_to_matches_naive_linear_walk() {
    // Parity guard for the galloping + partition_point `advance_to` (the
    // conjunction leapfrog hot loop). A multi-block corpus (> 3*BLOCK_SIZE)
    // with irregular gaps exercises both the skip-list block jumps AND the
    // intra-block galloping. For ANY non-decreasing target sequence, the
    // galloping must yield the EXACT same "first doc_id >= target" sequence as
    // a naive linear walk — bit-identical, or the conjunction set diverges.
    let mut builder = PostingsBuilder::new();
    // i*7 + i%5: strictly increasing (min step 3), irregular gaps (3 or 8).
    let doc_ids: Vec<u32> = (0..(BLOCK_SIZE * 3 + 17) as u32)
        .map(|i| i * 7 + i % 5)
        .collect();
    for &doc_id in &doc_ids {
        builder
            .add("body", "t", doc_id, vec![1])
            .expect("add posting");
    }
    let dictionary = builder.build();
    let list = dictionary
        .postings_with_block_metas("body", "t")
        .expect("postings list");

    // Non-decreasing target stream: every doc_id, plus near-misses around it,
    // plus a past-the-end probe.
    let mut targets: Vec<u32> = Vec::new();
    for &doc_id in &doc_ids {
        targets.push(doc_id.saturating_sub(1));
        targets.push(doc_id);
        targets.push(doc_id + 1);
    }
    targets.push(doc_ids.last().unwrap() + 1000);
    targets.sort_unstable();

    let mut iter = list
        .skip_iter()
        .expect("block skip list builds")
        .expect("non-empty postings");
    let mut naive_pos = 0usize;
    for &target in &targets {
        let got = iter.advance_to(target);
        // Naive reference: first doc_id >= target from the current cursor,
        // consuming past it (same return-and-consume semantics as advance_to).
        let want = loop {
            let Some(&doc_id) = doc_ids.get(naive_pos) else {
                break None;
            };
            naive_pos += 1;
            if doc_id >= target {
                break Some(doc_id);
            }
        };
        assert_eq!(got, want, "advance_to({target}) diverged from linear walk");
    }
}

#[test]
fn disk_cursor_matches_ram_leapfrog() {
    // Lot C `C1b` sous-pas 1 parity guard
    // (`docs/paper/c1b-disk-backed-design-2026-07-02.md`): the new,
    // purely-additive `DiskPostingsCursor` (`pread` + block-directory
    // skip, decoding at most one NEW block per call) must reproduce
    // `PostingsBlockSkipIter` (the RAM leapfrog conjunction/scan already
    // in production) bit-for-bit, INCLUDING the matched `freq`, for
    // every target in a non-decreasing sequence. Same corpus shape (3
    // full blocks + one partial trailing block, irregular gaps) as
    // `skip_iter_advance_to_matches_naive_linear_walk` above, so both
    // tests exercise the same skip-list block boundaries — only the
    // comparison target differs (disk cursor vs RAM leapfrog here,
    // RAM leapfrog vs naive walk there).
    let mut builder = PostingsBuilder::new();
    let total: u32 = (BLOCK_SIZE * 3 + 17) as u32;
    // i*7 + i%5: strictly increasing (min step 3), irregular gaps (3 or
    // 8) so the block-local delta reset AND the directory's byte offsets
    // are genuinely exercised, not just a constant stride.
    let doc_ids: Vec<u32> = (0..total).map(|i| i * 7 + i % 5).collect();
    for (i, &doc_id) in doc_ids.iter().enumerate() {
        // Variable freq (1..=5) so the freq channel — decoded from the
        // SAME block payload, right after the doc_id deltas — is
        // genuinely exercised too, not just a constant 1 everywhere.
        let freq_len = (i as u32 % 5) + 1;
        let positions: Vec<u32> = (0..freq_len).collect();
        builder
            .add("body", "t", doc_id, positions)
            .expect("add posting");
    }
    let dictionary = builder.build();

    let list = dictionary
        .postings_with_block_metas("body", "t")
        .expect("postings list");
    let mut ram_iter = list
        .skip_iter()
        .expect("block skip list builds")
        .expect("non-empty postings");
    let mut disk_cursor = dictionary
        .disk_cursor("body", "t")
        .expect("strictly-increasing doc_ids get full disk coverage");

    // Non-decreasing target stream: every doc_id, plus near-misses
    // around it (testing landing in a gap, since the sequence above has
    // real gaps of 3 or 8), plus a past-the-end probe.
    let mut targets: Vec<u32> = Vec::new();
    for &doc_id in &doc_ids {
        targets.push(doc_id.saturating_sub(1));
        targets.push(doc_id);
        targets.push(doc_id + 1);
    }
    targets.push(doc_ids.last().unwrap() + 1000);
    targets.sort_unstable();

    for &target in &targets {
        let ram_doc_id = ram_iter.advance_to(target);
        let disk_doc_id = disk_cursor.advance_to(target);
        assert_eq!(
            disk_doc_id, ram_doc_id,
            "advance_to({target}) diverged between the disk cursor and the RAM leapfrog"
        );
        if ram_doc_id.is_some() {
            let ram_freq = list.freqs()[ram_iter.position() - 1];
            assert_eq!(
                disk_cursor.freq(),
                ram_freq,
                "freq diverged at doc_id {ram_doc_id:?} for target {target}"
            );
        }
    }
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
    assert_eq!(search_postings[1].doc_id, 3);
    assert_eq!(search_postings[1].freq, 1);
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
    assert_eq!(paix[0].freq, 4);

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
    // with `postings.chunks(BLOCK_SIZE)`. Lot C Phase 1 lever B: `BlockMeta`
    // no longer stores `posting_count`/`min_doc_id`/`max_doc_id` (derivable
    // at O(1) from `doc_ids`) — those ranges are now checked through
    // `block_skip_list()`'s derived entries instead of the struct fields.
    let mut builder = PostingsBuilder::new();
    let total_docs: u32 = (BLOCK_SIZE as u32) * 2 + 17;
    // Push doc_ids in scrambled order so the test exercises the
    // ascending-doc_id sort performed by `PostingsBuilder::build()`.
    for offset in 0..total_docs {
        // Multiplier MUST be coprime with `total_docs` so the modulo is a
        // bijection (distinct doc_ids). 273 = 3*7*13, so `*7` would collapse
        // to 39 values each repeated 7x — a term with duplicate doc_ids,
        // which `block_skip_list()` rightly rejects as non-monotonic. 11 is
        // coprime with 273 → a permutation → distinct, valid postings.
        let doc_id = (offset * 11 + 3) % total_docs;
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
    // Single-position postings -> freq 1 everywhere.
    assert!(metas.iter().all(|meta| meta.max_term_freq == 1));

    let list = dictionary
        .postings_with_block_metas("body", "hit")
        .expect("hit postings list");
    assert_eq!(list.doc_freq_from_block_metas(), total_docs as usize);

    let skip_list = list
        .block_skip_list()
        .expect("skip list builds")
        .expect("non-empty postings");
    for (chunk, entry) in postings.chunks(BLOCK_SIZE).zip(skip_list.entries()) {
        assert_eq!(entry.min_doc_id, chunk.first().expect("chunk").doc_id);
        assert_eq!(entry.max_doc_id, chunk.last().expect("chunk").doc_id);
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
    assert_eq!(metas[0].max_term_freq, 1);
    assert_eq!(metas[1].max_term_freq, 4);
    assert_eq!(metas[2].max_term_freq, 1);

    // Lot C Phase 1 lever B: `posting_count`/`min_doc_id`/`max_doc_id` are
    // no longer stored on `BlockMeta` — derived at O(1) from `doc_ids` via
    // `doc_freq_from_block_metas()` / `block_skip_list()` (also exercised
    // by the AND-leapfrog hot path).
    let list = dictionary
        .postings_with_block_metas("body", "codec-sized")
        .expect("codec-sized postings list");
    assert_eq!(list.doc_freq_from_block_metas(), total_docs);
    let skip_list = list
        .block_skip_list()
        .expect("skip list builds")
        .expect("non-empty postings");
    let entries = skip_list.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].min_doc_id, 0);
    assert_eq!(entries[0].max_doc_id, (FOR_BLOCK_SIZE - 1) as u32);
    assert_eq!(entries[1].min_doc_id, FOR_BLOCK_SIZE as u32);
    assert_eq!(entries[1].max_doc_id, (FOR_BLOCK_SIZE * 2 - 1) as u32);
    assert_eq!(entries[2].min_doc_id, (FOR_BLOCK_SIZE * 2) as u32);
    assert_eq!(entries[2].max_doc_id, (total_docs - 1) as u32);
}

#[test]
fn term_dictionary_runtime_postings_exposes_for_block_metadata() {
    let mut builder = PostingsBuilder::new();
    let total_docs = BLOCK_SIZE * 2 + 9;
    for doc_id in 0..total_docs {
        builder
            .add("body", "runtime", doc_id as u32, vec![0])
            .expect("runtime posting");
    }

    let dictionary = builder.build();
    let runtime_postings = dictionary
        .postings_with_block_metas("body", "runtime")
        .expect("runtime postings");

    assert_eq!(runtime_postings.doc_ids().len(), total_docs);
    assert_eq!(runtime_postings.freqs().len(), total_docs);
    assert_eq!(runtime_postings.block_metas().len(), 3);
    assert_eq!(runtime_postings.doc_freq_from_block_metas(), total_docs);
    // Lot C Phase 1 lever B: per-block posting counts are no longer stored
    // on `BlockMeta` — derived from `doc_ids().chunks(BLOCK_SIZE)` instead.
    assert_eq!(
        runtime_postings
            .doc_ids()
            .chunks(BLOCK_SIZE)
            .map(<[_]>::len)
            .collect::<Vec<_>>(),
        vec![BLOCK_SIZE, BLOCK_SIZE, 9]
    );
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
    assert_eq!(metas[0].max_term_freq, 9);
    // Block 1 covers doc_ids 128..=255 inclusive; max freq is 5 (doc 200).
    assert_eq!(metas[1].max_term_freq, 5);
    // Block 2 covers doc_ids 256..=285 inclusive (partial); max freq is 7 (doc 270).
    assert_eq!(metas[2].max_term_freq, 7);

    // Lot C Phase 1 lever B: `min_doc_id`/`max_doc_id` are no longer
    // stored on `BlockMeta` — derived at O(1) from `doc_ids` via
    // `block_skip_list()`.
    let list = dictionary
        .postings_with_block_metas("body", "varied")
        .expect("varied postings list");
    let skip_list = list
        .block_skip_list()
        .expect("skip list builds")
        .expect("non-empty postings");
    let entries = skip_list.entries();
    assert_eq!(entries[0].min_doc_id, 0);
    assert_eq!(entries[0].max_doc_id, 127);
    assert_eq!(entries[1].min_doc_id, 128);
    assert_eq!(entries[1].max_doc_id, 255);
    assert_eq!(entries[2].min_doc_id, 256);
    assert_eq!(entries[2].max_doc_id, total_docs - 1);
}

#[test]
fn postings_segment_blocked_roundtrip_matches_ram() {
    // Lot C `C1a-batché` SHADOW segment: every term's FoR-blocked payload
    // must decode back bit-identical to the RAM source of truth
    // (`doc_ids_flat`/`freqs_flat`), across multiple fields, including a
    // term spanning >= 3 blocks (full + full + full + partial) to exercise
    // the block-boundary delta reset.
    let mut builder = PostingsBuilder::new();

    // Small single-block terms, spread across two fields.
    builder
        .add("title", "surch", 1, vec![0])
        .expect("title/surch");
    builder
        .add("title", "rust", 2, vec![0, 1])
        .expect("title/rust");
    builder
        .add("body", "engine", 3, vec![0])
        .expect("body/engine");

    // A term spanning >= 3 blocks: BLOCK_SIZE*3+17 postings with DISTINCT
    // doc_ids, inserted out of order. Multiplier MUST be coprime with the
    // doc count so the modulo stays a bijection (see
    // `term_dictionary_block_metas_match_postings_chunks` above for why a
    // non-coprime multiplier would collapse to duplicate doc_ids).
    let total_docs: u32 = (BLOCK_SIZE as u32) * 3 + 17;
    for offset in 0..total_docs {
        let doc_id = (offset * 11 + 3) % total_docs;
        builder
            .add("body", "wide", doc_id, vec![0, 1])
            .expect("wide posting");
    }

    let dictionary = builder.build();

    for (field, term) in [
        ("title", "surch"),
        ("title", "rust"),
        ("body", "engine"),
        ("body", "wide"),
    ] {
        let expected: Vec<_> = dictionary
            .postings(field, term)
            .unwrap_or_else(|| panic!("{field}/{term} postings"))
            .collect();
        let expected_doc_ids: Vec<u32> = expected.iter().map(|p| p.doc_id).collect();
        let expected_freqs: Vec<u32> = expected.iter().map(|p| p.freq).collect();

        let (decoded_doc_ids, decoded_freqs) = dictionary
            .decode_from_segment(field, term)
            .unwrap_or_else(|| panic!("{field}/{term} segment decode"));

        assert_eq!(
            decoded_doc_ids, expected_doc_ids,
            "{field}/{term} doc_ids mismatch between segment and RAM"
        );
        assert_eq!(
            decoded_freqs, expected_freqs,
            "{field}/{term} freqs mismatch between segment and RAM"
        );
    }

    // Unknown field/term yields `None`, consistent with `postings()`.
    assert!(dictionary.decode_from_segment("body", "missing").is_none());
    assert!(dictionary.decode_from_segment("missing", "wide").is_none());

    // The segment actually received bytes (batched per field — the
    // gauge exists precisely to make this observable in production).
    assert!(dictionary.postings_segment_bytes() > 0);
}

#[test]
fn postings_segment_encodes_after_duplicate_doc_ids_are_merged() {
    // Correction fix regression for the C1a-batché crash observed on the
    // real deces corpus (1.36 M docs, bulk OK, `count: 0` after
    // `_refresh`): a source `Value::Array` field gets exploded into
    // several `(field, value)` pairs of the SAME doc_id upstream
    // (`indexed_fields_for_document` in surch-api), so the same `(field,
    // term, doc_id)` triple can reach `PostingsBuilder::add` more than
    // once for one document. `build()` now merges those same-doc_id runs
    // (`dedup_merge_postings`) BEFORE the sort-derived encode below, so
    // `encode_postings_blocked` never sees a non-monotonic doc_id from
    // this cause any more — the SHADOW segment gets FULL disk coverage,
    // not a sentinel, and `postings_segment_skipped_terms()` stays 0.
    let mut builder = PostingsBuilder::new();

    // A normal term: unaffected by the other term's multi-valued merge
    // in the same field.
    builder
        .add("body", "clean", 1, vec![0])
        .expect("clean posting");

    // A term with TWO postings for the SAME doc_id (5) — the duplicate
    // doc_id that used to make `encode_postings_blocked(...).expect(...)`
    // panic inside `PostingsBuilder::build()` before the SHADOW
    // hardening, and that the hardening used to tolerate by dropping
    // disk coverage for the term. Positions `[0]` then `[1, 2]` mirror
    // two source array values both containing this term.
    builder
        .add("body", "dup", 5, vec![0])
        .expect("dup posting #1");
    builder
        .add("body", "dup", 5, vec![1, 2])
        .expect("dup posting #2 (same doc_id as #1)");

    let dictionary = builder.build();

    // RAM now holds the Lucene-shaped merge: ONE posting for doc 5, freq
    // = 1 + 2 = 3 (total occurrences across both source values).
    let dup_postings: Vec<_> = dictionary
        .postings("body", "dup")
        .expect("dup postings")
        .collect();
    assert_eq!(dup_postings.len(), 1);
    assert_eq!(dup_postings[0].doc_id, 5);
    assert_eq!(dup_postings[0].freq, 3);

    // The unrelated "clean" term is unaffected.
    let clean_postings: Vec<_> = dictionary
        .postings("body", "clean")
        .expect("clean postings")
        .collect();
    assert_eq!(clean_postings.len(), 1);
    assert_eq!(clean_postings[0].doc_id, 1);

    // "dup" now gets FULL disk coverage — the merge made its doc_ids
    // strictly increasing, so the SHADOW segment round-trips it exactly
    // like "clean".
    let (dup_doc_ids, dup_freqs) = dictionary
        .decode_from_segment("body", "dup")
        .expect("dup segment decode (no longer skipped)");
    assert_eq!(dup_doc_ids, vec![5]);
    assert_eq!(dup_freqs, vec![3]);
    assert!(dictionary.decode_from_segment("body", "clean").is_some());

    // No term failed to encode any more — the gauge backing
    // `surch_index_disk_postings_skipped_terms` reads 0.
    assert_eq!(dictionary.postings_segment_skipped_terms(), 0);
}

#[test]
fn postings_builder_dedups_multivalue_doc_id() {
    // Direct unit test for `dedup_merge_postings`: a multi-valued source
    // field (e.g. a `Value::Array` exploded by `indexed_fields_for_document`
    // in surch-api into several `(field, value)` pairs of the same
    // doc_id) can make the SAME `(field, term, doc_id)` triple reach
    // `add` twice for one document — once per array value that contains
    // the term. `build()` must collapse that into ONE posting per
    // Lucene/ES multi-valued-field semantics: `df` counts the document
    // once, `freq` (`tf`) is the total occurrence count across every
    // value.
    let mut builder = PostingsBuilder::new();

    // Two other documents to prove the merge only touches the doc_id
    // that actually repeats, and that ordering/doc_ids stay correct
    // around it.
    builder.add("name", "jean", 1, vec![0]).expect("doc 1 jean");
    builder
        .add("name", "jean", 10, vec![0])
        .expect("doc 10 jean");

    // Doc 5: "jean" occurs in two array values, e.g. ["Jean Pierre",
    // "Jean Paul"] — each value's tokenization restarts positions at 0
    // (no position_increment_gap in surch), so this is `add`-ed twice
    // for the SAME (field="name", term="jean", doc_id=5).
    builder
        .add("name", "jean", 5, vec![0])
        .expect("doc 5 jean, value #1");
    builder
        .add("name", "jean", 5, vec![0])
        .expect("doc 5 jean, value #2");

    let dictionary = builder.build();

    let postings: Vec<_> = dictionary
        .postings("name", "jean")
        .expect("name/jean postings")
        .collect();

    // Exactly one posting per document: df == 3, not 4.
    assert_eq!(postings.len(), 3);

    let doc_ids: Vec<u32> = postings.iter().map(|p| p.doc_id).collect();
    assert_eq!(doc_ids, vec![1, 5, 10]);
    // Strictly increasing: no duplicate doc_id survives the merge.
    assert!(doc_ids.windows(2).all(|pair| pair[0] < pair[1]));

    // Doc 5's freq is the SUM of both values' occurrence counts (1 + 1 =
    // 2) — `tf` = total occurrences, matching Lucene/ES.
    let doc5 = postings
        .iter()
        .find(|p| p.doc_id == 5)
        .expect("doc 5 posting");
    assert_eq!(doc5.freq, 2);

    // The untouched single-value docs keep freq == 1.
    assert_eq!(postings[0].freq, 1);
    assert_eq!(postings[2].freq, 1);

    // The merge fed a strictly-increasing doc_id sequence to the FoR
    // encoder, so the SHADOW segment gets full disk coverage for this
    // term too — a second, end-to-end confirmation that
    // `postings_segment_skipped_terms()` stays 0 after a multi-valued
    // merge.
    assert!(dictionary.decode_from_segment("name", "jean").is_some());
    assert_eq!(dictionary.postings_segment_skipped_terms(), 0);
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

/// Plan segments S5 (`docs/paper/design-segments-pic-borne-2026-07-05.md`
/// §"Dimensionnement S5"): `segment_descriptors` was identified as the
/// single largest previously-ungauged, per-term (`T`-scaled) RAM
/// component — 16 B/term after Rust struct padding, invisible to every
/// existing gauge. Read-parity gate for its disk-back: builds the SAME
/// postings twice (flag off, then flag on, via `build_with_disk_flag` —
/// the per-call override, since the process-wide `SURCH_POSTINGS_DISK`
/// `OnceLock` cannot flip mid-process) and asserts every term's
/// `decode_from_segment` result stays bit-identical between the resident
/// array (flag off, UNCHANGED default) and the new disk-backed directory
/// plus one extra `pread` (flag on). The companion white-box test
/// `segment_descriptors_move_off_heap_when_disk_flag_is_on` (in
/// `crates/surch-index/src/postings.rs`'s own `#[cfg(test)]` module, which
/// has access to the private `FieldPostings` fields) covers the actual
/// RAM-shrink claim from the inside — the aggregate
/// `postings_directory_bytes()` gauge also carries the PRE-EXISTING C1b
/// `block_directory`/`block_dir_offsets` disk-mode cost, which is
/// orthogonal to this change and can outweigh it for single-block terms,
/// so comparing that gauge flag-off vs flag-on here would be misleading.
#[test]
fn postings_disk_backed_descriptors_match_resident_descriptors() {
    // 200 DISTINCT terms per field (T = 200) so the per-term CSR/directory
    // metadata is not noise — mirrors, at unit-test scale, the high term
    // cardinality an `edge_ngram`/`autocomplete` analyzer or
    // `index_prefixes` field produces on the real matchID corpus.
    fn populate(builder: &mut PostingsBuilder) {
        for i in 0..200u32 {
            builder
                .add("body", format!("term{i}"), i, vec![0])
                .expect("distinct term per doc");
            builder
                .add("title", format!("alt{i}"), i, vec![0, 1])
                .expect("distinct term per doc, second field");
        }
    }

    let mut builder_off = PostingsBuilder::new();
    populate(&mut builder_off);
    let dict_off = builder_off.build_with_disk_flag(false);

    let mut builder_on = PostingsBuilder::new();
    populate(&mut builder_on);
    let dict_on = builder_on.build_with_disk_flag(true);

    // Sanity: the flag-off dictionary carries the resident directory
    // metadata this whole mission is about (non-zero).
    assert!(
        dict_off.postings_directory_bytes() > 0,
        "flag-off dictionary should carry non-zero directory bytes (resident segment_descriptors)"
    );

    // Read parity: the disk-backed descriptor lookup (one extra `pread`)
    // must resolve to the exact same bytes the resident array would have,
    // for every term in both fields.
    for i in 0..200u32 {
        let term = format!("term{i}");
        let off = dict_off
            .decode_from_segment("body", &term)
            .unwrap_or_else(|| panic!("flag-off decode body/{term}"));
        let on = dict_on
            .decode_from_segment("body", &term)
            .unwrap_or_else(|| panic!("flag-on decode body/{term}"));
        assert_eq!(off, on, "decode_from_segment diverged for body/{term}");

        let alt = format!("alt{i}");
        let off_alt = dict_off
            .decode_from_segment("title", &alt)
            .unwrap_or_else(|| panic!("flag-off decode title/{alt}"));
        let on_alt = dict_on
            .decode_from_segment("title", &alt)
            .unwrap_or_else(|| panic!("flag-on decode title/{alt}"));
        assert_eq!(
            off_alt, on_alt,
            "decode_from_segment diverged for title/{alt}"
        );
    }
}
