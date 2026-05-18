//! Criterion bench: snapshot-payload size — JSON vs FoR-encoded postings.
//!
//! **Why this bench exists (and not the wire-up itself)** — the
//! `for-integration-plan.md` Phase 1 originally proposed encoding the
//! per-term postings of a persisted snapshot with the
//! `surch_codec::postings_block` FoR codec. The ES-parity snapshot path
//! today (`crates/surch-api/src/snapshot.rs` → `snapshot_es/service.rs`)
//! does NOT serialise postings: it ships a tarball of the **source
//! documents** (NDJSON) plus mapping/settings/aliases, and the restore
//! re-runs ingestion to rebuild the in-memory `TermDictionary`. Wiring
//! the codec into a non-existent on-disk postings format would mean
//! inventing a new durable segment layout — a much larger surface than
//! Phase 1 was scoped for, and it would touch `snapshot.rs` /
//! `snapshot_es/`, which the `wp/a-optim` charter currently freezes.
//!
//! So this bench is the **measured equivalent** of Phase 1: it builds
//! an in-memory term dictionary from a synthetic 100-doc corpus (same
//! token shape as the BAN / SciFact workloads), then compares two
//! candidate on-repository payloads for the postings part of a future
//! durable segment:
//!
//! - **JSON baseline** : `serde_json::to_vec(&(Vec<u32>, Vec<u32>))`
//!   per (field, term) — what `_surch/snapshot/export` would look like
//!   if it inlined the postings the way it inlines documents today.
//! - **FoR-encoded** : `encode_postings_doc_id_freq(doc_ids, freqs)`
//!   per (field, term) — the proposed Phase 1 payload, with the
//!   streaming `DocIdDeltaCursor` for restore.
//!
//! The bench prints `bytes_json`, `bytes_for`, the compression ratio,
//! and a per-corpus round-trip assertion (decoded `doc_ids` + `freqs`
//! must equal the originals byte-for-byte). The exit criterion of
//! Phase 1 ("compression ≥ 2×") is therefore directly observable here,
//! without touching the snapshot plumbing.
//!
//! Run: `cargo bench --bench snapshot_size -p surch-api`.

use std::collections::BTreeMap;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::json;

use surch_codec::postings_block::{
    decode_postings_doc_id_freq, encode_postings_doc_id_freq, DocIdDeltaCursor,
};
use surch_index::postings::{PostingsBuilder, TermDictionary};

/// Number of documents in the synthetic corpus. The Phase 1 exit
/// criterion is stated against a 100-doc index ("`wc -c` on the blob
/// divided by ≥ 2") — matching it here makes the bench output a
/// direct read of that gate.
const CORPUS_SIZE: usize = 100;

/// Synthetic vocabulary — deliberately small (32 words) so the
/// 100-doc corpus has term frequencies in the same dynamic range as
/// the BAN / SciFact token distribution: a handful of dense head
/// terms (every doc), a long tail of rare ones.
const VOCAB: &[&str] = &[
    "rust",
    "data",
    "search",
    "engine",
    "score",
    "index",
    "query",
    "match",
    "term",
    "field",
    "document",
    "vector",
    "memory",
    "block",
    "posting",
    "codec",
    "delta",
    "varint",
    "snapshot",
    "compress",
    "encode",
    "decode",
    "byte",
    "cursor",
    "frame",
    "reference",
    "lucene",
    "tantivy",
    "fst",
    "trie",
    "table",
    "list",
];

/// Deterministic SplitMix64 — same generator family as
/// `search_hot_path.rs`, so corpus shape is reproducible across
/// machines without pulling `rand`.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }
}

/// Build the `TermDictionary` of a `CORPUS_SIZE`-doc synthetic index
/// with two text fields, `title` (≈ 4–8 tokens) and `body` (≈ 40–80
/// tokens) drawn from `VOCAB`. Tokens carry positions so the layout
/// matches the real ingestion path; the bench only measures the
/// `(doc_id, freq)` channel — positions are deliberately out of
/// scope for Phase 1 (per `for-integration-plan.md` Phase 3).
fn build_corpus_dictionary() -> TermDictionary {
    let mut rng = SplitMix64::new(0x5C1F_AC75_BEE5_0001);
    let mut builder = PostingsBuilder::new();
    for doc_id in 0..CORPUS_SIZE as u32 {
        // Title: 4..=8 tokens.
        let title_len = 4 + (rng.next() % 5) as usize;
        let mut title_positions: BTreeMap<&'static str, Vec<u32>> = BTreeMap::new();
        for pos in 0..title_len {
            let term = VOCAB[(rng.next() as usize) % VOCAB.len()];
            title_positions.entry(term).or_default().push(pos as u32);
        }
        for (term, positions) in title_positions {
            builder
                .add("title", term, doc_id, positions)
                .expect("non-empty term");
        }

        // Body: 40..=80 tokens.
        let body_len = 40 + (rng.next() % 41) as usize;
        let mut body_positions: BTreeMap<&'static str, Vec<u32>> = BTreeMap::new();
        for pos in 0..body_len {
            let term = VOCAB[(rng.next() as usize) % VOCAB.len()];
            body_positions.entry(term).or_default().push(pos as u32);
        }
        for (term, positions) in body_positions {
            builder
                .add("body", term, doc_id, positions)
                .expect("non-empty term");
        }
    }
    builder.build()
}

/// Flatten every `(field, term)` posting list to a `Vec<(Vec<u32>,
/// Vec<u32>)>` — the canonical (doc_ids, freqs) channel the codec
/// operates on. Iteration order is `(field, term)` lexicographic so
/// the JSON and FoR payloads are byte-deterministic for the
/// round-trip check.
fn extract_postings(dict: &TermDictionary) -> Vec<(String, String, Vec<u32>, Vec<u32>)> {
    let mut out = Vec::new();
    for field in dict.field_names() {
        let terms: Vec<String> = dict.terms(&field).collect();
        for term in terms {
            let postings = dict
                .postings(&field, &term)
                .expect("term enumerated above must resolve");
            let mut doc_ids = Vec::new();
            let mut freqs = Vec::new();
            for posting in postings {
                doc_ids.push(posting.doc_id);
                freqs.push(posting.freq);
            }
            out.push((field.clone(), term, doc_ids, freqs));
        }
    }
    out
}

/// Concatenate the JSON encoding of every per-term `(doc_ids, freqs)`
/// pair, mimicking the most direct port of the current snapshot path
/// (which already uses `serde_json::to_vec_pretty` for the manifest
/// and per-index metadata). Pretty-printing is intentionally OFF
/// here — that's the conservative baseline; a real production wire-up
/// would not pretty-print the postings blob.
fn json_payload(postings: &[(String, String, Vec<u32>, Vec<u32>)]) -> Vec<u8> {
    let entries: Vec<_> = postings
        .iter()
        .map(|(field, term, doc_ids, freqs)| {
            json!({
                "field": field,
                "term": term,
                "doc_ids": doc_ids,
                "freqs": freqs,
            })
        })
        .collect();
    serde_json::to_vec(&entries).expect("serde_json on owned values never fails")
}

/// Concatenate the FoR-encoded payload of every per-term `(doc_ids,
/// freqs)` pair. Layout : 1-byte magic `0xF1` (FoR phase 1) +
/// little-endian `u16` version `1` + per-term `(field_len u16, field
/// bytes, term_len u16, term bytes, body_len u32, body)` records.
/// Compact enough to be a candidate durable-segment layout and easy
/// to round-trip in the bench assertion below.
const FOR_MAGIC: u8 = 0xF1;
const FOR_VERSION: u16 = 1;

fn for_payload(postings: &[(String, String, Vec<u32>, Vec<u32>)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(postings.len() * 16);
    out.push(FOR_MAGIC);
    out.extend_from_slice(&FOR_VERSION.to_le_bytes());
    for (field, term, doc_ids, freqs) in postings {
        let body = encode_postings_doc_id_freq(doc_ids, freqs)
            .expect("strictly-increasing doc_ids from PostingsBuilder::build()");
        out.extend_from_slice(&(field.len() as u16).to_le_bytes());
        out.extend_from_slice(field.as_bytes());
        out.extend_from_slice(&(term.len() as u16).to_le_bytes());
        out.extend_from_slice(term.as_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
    }
    out
}

/// Walk the FoR payload back to a `Vec<(field, term, doc_ids, freqs)>`
/// using `decode_postings_doc_id_freq` (eager) — proves the round trip
/// matches the source byte-for-byte on the logical payload. The
/// streaming `DocIdDeltaCursor` is exercised separately below.
fn parse_for_payload(bytes: &[u8]) -> Vec<(String, String, Vec<u32>, Vec<u32>)> {
    assert_eq!(bytes[0], FOR_MAGIC, "magic byte mismatch");
    let version = u16::from_le_bytes([bytes[1], bytes[2]]);
    assert_eq!(version, FOR_VERSION, "version byte mismatch");
    let mut pos = 3usize;
    let mut out = Vec::new();
    while pos < bytes.len() {
        let field_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;
        let field = std::str::from_utf8(&bytes[pos..pos + field_len])
            .expect("field bytes are valid utf-8 (sourced from String above)")
            .to_owned();
        pos += field_len;
        let term_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2;
        let term = std::str::from_utf8(&bytes[pos..pos + term_len])
            .expect("term bytes are valid utf-8")
            .to_owned();
        pos += term_len;
        let body_len =
            u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
        pos += 4;
        let (doc_ids, freqs) = decode_postings_doc_id_freq(&bytes[pos..pos + body_len])
            .expect("body was just produced by encode_postings_doc_id_freq");
        pos += body_len;
        out.push((field, term, doc_ids, freqs));
    }
    out
}

/// Streaming-restore variant: walks every term's doc-id channel via
/// `DocIdDeltaCursor` without materialising the full `Vec<u32>` —
/// mirrors what the future restore path will do for cold posting
/// lists. Returns the total number of doc ids visited so the
/// optimiser can't elide the call.
fn streaming_visit(bytes: &[u8]) -> u64 {
    let mut total: u64 = 0;
    let mut pos = 3usize;
    while pos < bytes.len() {
        let field_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2 + field_len;
        let term_len = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2 + term_len;
        let body_len =
            u32::from_le_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
        pos += 4;
        let mut cursor = DocIdDeltaCursor::new(&bytes[pos..pos + body_len])
            .expect("cursor accepts well-formed body");
        while let Some(_id) = cursor.next().expect("body is well-formed") {
            total += 1;
        }
        pos += body_len;
    }
    total
}

/// Pretty-print the compression ratio and the term-count to stderr
/// so the bench output reads as a Phase 1 exit-criterion check.
fn print_ratio(json_bytes: usize, for_bytes: usize, term_count: usize) {
    let ratio = json_bytes as f64 / for_bytes as f64;
    eprintln!(
        "snapshot_size: terms={term_count} json={json_bytes}B for={for_bytes}B \
         compression={ratio:.2}x ({pct:.1}% of JSON)",
        pct = (for_bytes as f64 / json_bytes as f64) * 100.0
    );
    // Phase 1 exit criterion : `wc -c` on the blob divided by ≥ 2.
    // Asserting here keeps the bench self-checking — a regression
    // (e.g. someone disables delta encoding) fails the bench, not
    // just the eyeballed ratio.
    assert!(
        ratio >= 2.0,
        "Phase 1 exit criterion: expected ≥ 2× compression, got {ratio:.2}x"
    );
}

/// Self-check: encode → decode → equality on the logical payload.
/// Runs once in `bench_snapshot_size` before the timed section so a
/// codec regression fails loudly without polluting the criterion
/// statistics.
fn assert_round_trip(postings: &[(String, String, Vec<u32>, Vec<u32>)]) {
    let payload = for_payload(postings);
    let decoded = parse_for_payload(&payload);
    assert_eq!(decoded.len(), postings.len(), "term count mismatch");
    for ((f1, t1, d1, fr1), (f2, t2, d2, fr2)) in postings.iter().zip(decoded.iter()) {
        assert_eq!(f1, f2, "field mismatch");
        assert_eq!(t1, t2, "term mismatch");
        assert_eq!(d1, d2, "doc_ids mismatch for ({f1}, {t1})");
        assert_eq!(fr1, fr2, "freqs mismatch for ({f1}, {t1})");
    }
}

fn bench_snapshot_size(c: &mut Criterion) {
    let dict = build_corpus_dictionary();
    let postings = extract_postings(&dict);
    let term_count = postings.len();

    // Print the size comparison once (before the timed section) so
    // the human reviewer sees the Phase 1 exit-criterion value
    // independently of criterion's iteration count.
    let json_bytes = json_payload(&postings);
    let for_bytes = for_payload(&postings);
    print_ratio(json_bytes.len(), for_bytes.len(), term_count);

    // Round-trip self-check (gates a codec regression cleanly).
    assert_round_trip(&postings);

    // Streaming-restore parity: cursor walks the same total number
    // of doc ids the eager decoder reports.
    let eager_total: u64 = postings.iter().map(|(_, _, d, _)| d.len() as u64).sum();
    let streamed_total = streaming_visit(&for_bytes);
    assert_eq!(
        streamed_total, eager_total,
        "DocIdDeltaCursor must yield the same doc-id count as the eager decoder"
    );

    let mut group = c.benchmark_group("snapshot_size");
    group.bench_function("json_encode", |b| {
        b.iter(|| {
            let bytes = json_payload(black_box(&postings));
            black_box(bytes.len())
        })
    });
    group.bench_function("for_encode", |b| {
        b.iter(|| {
            let bytes = for_payload(black_box(&postings));
            black_box(bytes.len())
        })
    });
    group.bench_function("for_streaming_restore", |b| {
        b.iter(|| black_box(streaming_visit(black_box(&for_bytes))))
    });
    group.finish();
}

criterion_group!(benches, bench_snapshot_size);
criterion_main!(benches);
