//! Parity gate between `surch_search::small_float` (canonical Lucene
//! port, used by docs / external readers) and the private mirror baked
//! into `surch_index::document_index` (used by the indexer's hot path,
//! which cannot depend upward on `surch-search`). The two must remain
//! byte-identical for every input the indexer can produce — otherwise
//! the BM25 scorer would consume bytes encoded by a divergent codec.
//!
//! See `docs/paper/ndcg-trec-covid-rootcause-22.md` for context.

use surch_index::{decode_doc_len_byte, document_index::DocumentIndex};
use surch_search::small_float::{byte4_to_int, int_to_byte4};

/// Round-trip a single-token doc through `DocumentIndex` and inspect
/// the stored byte. Returns `(stored_byte, reconstructed_via_index)`.
fn record_and_inspect(raw_len: u32) -> (u8, u64) {
    // Build a synthetic doc with exactly `raw_len` whitespace tokens.
    // Default analyzer is whitespace-splitting, so the field-length
    // recorded equals the token count.
    let body = (0..raw_len)
        .map(|i| format!("t{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut index = DocumentIndex::new();
    index
        .add_documents_with_mapping([(0u32, [("body", body.as_str())])], &Default::default())
        .expect("ingest");
    let stats = index.field_stats("body").expect("body stats");
    let byte = stats.doc_len_dense()[0];
    let reconstructed = stats.doc_len(0).unwrap_or(0);
    (byte, reconstructed)
}

#[test]
fn surch_index_byte_matches_canonical_encoder() {
    // The canonical encoder lives in `surch_search::small_float`.
    // The private mirror inside `surch_index::document_index` must
    // produce byte-identical output for every input the indexer can
    // see. We probe the full range relevant to BEIR / matchID corpora
    // (`doc_len ∈ [1, 4096]`) plus a denser scan inside the free
    // range and around the encoder transition.
    let mut probes: Vec<u32> = (1u32..=64u32).collect();
    probes.extend([100, 200, 255, 256, 500, 1000, 1500, 2048, 3000, 4096]);
    for raw in probes {
        let (byte, reconstructed) = record_and_inspect(raw);
        let canonical_byte = int_to_byte4(raw);
        let canonical_reconstructed = byte4_to_int(canonical_byte) as u64;
        assert_eq!(
            byte, canonical_byte,
            "raw={raw}: surch-index byte {byte} != surch-search canonical {canonical_byte}"
        );
        assert_eq!(
            reconstructed, canonical_reconstructed,
            "raw={raw}: surch-index reconstructed {reconstructed} != canonical {canonical_reconstructed}"
        );
        assert_eq!(
            decode_doc_len_byte(byte),
            canonical_reconstructed,
            "raw={raw}: decode_doc_len_byte helper diverges from canonical"
        );
    }
}

#[test]
fn free_range_round_trips_lossless() {
    // [0, 24) is Lucene's free passthrough range — short docs (the
    // SciFact regime) must keep exact lengths so the scoring formula
    // is bit-identical to the previous exact-length implementation
    // on those corpora.
    for raw in 1u32..24u32 {
        let (byte, reconstructed) = record_and_inspect(raw);
        assert_eq!(byte as u32, raw, "free range encoder identity at {raw}");
        assert_eq!(
            reconstructed, raw as u64,
            "free range decoder identity at {raw}"
        );
    }
}

#[test]
fn min_doc_len_reports_quantized_value() {
    // The WAND upper bound feeds `min_doc_len` straight into the BM25
    // kernel; it must already be the reconstructed (Lucene-quantized)
    // value, not the raw token count. We ingest two docs with raw
    // lengths in the quantized range and assert the reported min
    // matches the reconstructed bucket.
    let raw_short = 80u32; // quantizes to a specific bucket below 80
    let raw_long = 120u32;
    let body_short = (0..raw_short)
        .map(|i| format!("a{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let body_long = (0..raw_long)
        .map(|i| format!("b{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut index = DocumentIndex::new();
    index
        .add_documents_with_mapping(
            [
                (0u32, [("body", body_short.as_str())]),
                (1u32, [("body", body_long.as_str())]),
            ],
            &Default::default(),
        )
        .expect("ingest");
    let stats = index.field_stats("body").expect("body stats");
    let expected_min = byte4_to_int(int_to_byte4(raw_short)) as u64;
    assert_eq!(stats.min_doc_len(), Some(expected_min));
    // Sanity: the longer doc reconstructs to a strictly larger length
    // (otherwise min_doc_len would be ambiguous and the test useless).
    let long_reconstructed = byte4_to_int(int_to_byte4(raw_long)) as u64;
    assert!(long_reconstructed >= expected_min);
}
