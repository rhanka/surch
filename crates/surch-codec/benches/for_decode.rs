//! Criterion bench: standalone Frame-of-Reference / delta-varint codec.
//!
//! Goal — measure encode, decode, and decode-on-demand cursor speed
//! plus the compression ratio of `postings_block::*` against the naive
//! `Vec<u32>` layout that `crates/surch-index/src/postings.rs` keeps
//! in RAM today. This bench is **standalone**: it does NOT wire the
//! codec into the live postings path; that integration ships across
//! the 3 phases described in `docs/poc/for-integration-plan.md`.
//!
//! Three corpus shapes mirror the BAN/INSEE workload:
//!
//! - `dense_1k_step3` : 1 024 doc ids spaced by 3 (mean delta ≈ 3 —
//!   frequent short tokens like "rue" / "DUPONT")
//! - `sparse_2k_random` : 2 000 random doc ids ∈ [0, 10 M] (mid-rare
//!   tokens; same xorshift32 seed as the lib test)
//! - `seq_4k_dense` : 4 096 contiguous doc ids (very dense head token —
//!   worst case for varint, best case for the future block-128 layout)
//!
//! Run: `cargo bench --bench for_decode -p surch-codec`.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use surch_codec::postings_block::{
    decode_doc_ids_delta_varint, decode_postings_doc_id_freq, encode_doc_ids_delta_varint,
    encode_postings_doc_id_freq, DocIdDeltaCursor,
};

/// Deterministic xorshift32 (same constants as the in-crate test) so
/// the bench corpus is reproducible without pulling `rand`.
fn xorshift32_corpus(count: usize, modulus: u32, seed: u32) -> Vec<u32> {
    let mut state = seed;
    let mut next = || -> u32 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut out: Vec<u32> = (0..count).map(|_| next() % modulus).collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn dense_1k_step3() -> Vec<u32> {
    (0..1024).map(|i| i * 3 + 7).collect()
}

fn sparse_2k_random() -> Vec<u32> {
    xorshift32_corpus(2000, 10_000_000, 0x1234_5678)
}

fn seq_4k_dense() -> Vec<u32> {
    (0..4096).collect()
}

fn freqs_for(doc_ids: &[u32]) -> Vec<u32> {
    // Same distribution as the lib test: term_freq ∈ [1..=16].
    doc_ids.iter().map(|d| (d % 16) + 1).collect()
}

/// Pretty-print the compression ratio of the codec against the naive
/// `Vec<u32>` layout. Printed once per corpus shape during the bench
/// `setup` phase so the human reviewer sees the RAM win side-by-side
/// with the latency numbers.
fn print_ratio(label: &str, doc_ids: &[u32], encoded_len: usize) {
    let naive_bytes = std::mem::size_of_val(doc_ids);
    let ratio = naive_bytes as f64 / encoded_len as f64;
    eprintln!(
        "for_decode/{label}: n={n} encoded={encoded_len}B naive={naive_bytes}B \
         compression={ratio:.2}x ({pct:.1}% of naive)",
        n = doc_ids.len(),
        pct = (encoded_len as f64 / naive_bytes as f64) * 100.0
    );
}

fn bench_encode_doc_ids(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_doc_ids");
    for (label, corpus) in [
        ("dense_1k_step3", dense_1k_step3()),
        ("sparse_2k_random", sparse_2k_random()),
        ("seq_4k_dense", seq_4k_dense()),
    ] {
        group.throughput(Throughput::Elements(corpus.len() as u64));
        // Side-print ratio once per shape.
        let one_shot = encode_doc_ids_delta_varint(&corpus).expect("encode");
        print_ratio(&format!("encode/{label}"), &corpus, one_shot.len());
        group.bench_with_input(BenchmarkId::from_parameter(label), &corpus, |b, ids| {
            b.iter(|| {
                let encoded = encode_doc_ids_delta_varint(black_box(ids)).expect("encode");
                black_box(encoded);
            });
        });
    }
    group.finish();
}

fn bench_decode_doc_ids(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_doc_ids");
    for (label, corpus) in [
        ("dense_1k_step3", dense_1k_step3()),
        ("sparse_2k_random", sparse_2k_random()),
        ("seq_4k_dense", seq_4k_dense()),
    ] {
        let encoded = encode_doc_ids_delta_varint(&corpus).expect("encode");
        group.throughput(Throughput::Elements(corpus.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &encoded, |b, bytes| {
            b.iter(|| {
                let decoded = decode_doc_ids_delta_varint(black_box(bytes)).expect("decode");
                black_box(decoded);
            });
        });
    }
    group.finish();
}

fn bench_cursor_walk(c: &mut Criterion) {
    let mut group = c.benchmark_group("cursor_walk");
    for (label, corpus) in [
        ("dense_1k_step3", dense_1k_step3()),
        ("sparse_2k_random", sparse_2k_random()),
        ("seq_4k_dense", seq_4k_dense()),
    ] {
        let freqs = freqs_for(&corpus);
        let encoded = encode_postings_doc_id_freq(&corpus, &freqs).expect("encode");
        group.throughput(Throughput::Elements(corpus.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &encoded, |b, bytes| {
            b.iter(|| {
                let mut cursor = DocIdDeltaCursor::new(black_box(bytes)).expect("cursor");
                let mut sum: u64 = 0;
                while let Some(v) = cursor.next().expect("walk") {
                    sum = sum.wrapping_add(v as u64);
                }
                black_box(sum);
            });
        });
    }
    group.finish();
}

fn bench_encode_postings(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_postings");
    for (label, corpus) in [
        ("dense_1k_step3", dense_1k_step3()),
        ("sparse_2k_random", sparse_2k_random()),
        ("seq_4k_dense", seq_4k_dense()),
    ] {
        let freqs = freqs_for(&corpus);
        let one_shot = encode_postings_doc_id_freq(&corpus, &freqs).expect("encode");
        // For postings the "naive" layout is `Vec<Posting>` with at
        // least doc_id (u32) + freq (u32) = 8 B/posting; positions
        // are not included here (the codec doesn't store them yet —
        // see Phase 2 in the integration plan).
        let naive_bytes = corpus.len() * 8;
        eprintln!(
            "for_decode/encode_postings/{label}: n={n} encoded={enc}B \
             naive_doc_id_freq={naive_bytes}B compression={ratio:.2}x",
            n = corpus.len(),
            enc = one_shot.len(),
            ratio = naive_bytes as f64 / one_shot.len() as f64
        );
        group.throughput(Throughput::Elements(corpus.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(label),
            &(corpus, freqs),
            |b, (ids, fr)| {
                b.iter(|| {
                    let encoded = encode_postings_doc_id_freq(black_box(ids), black_box(fr))
                        .expect("encode");
                    black_box(encoded);
                });
            },
        );
    }
    group.finish();
}

fn bench_decode_postings(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_postings");
    for (label, corpus) in [
        ("dense_1k_step3", dense_1k_step3()),
        ("sparse_2k_random", sparse_2k_random()),
        ("seq_4k_dense", seq_4k_dense()),
    ] {
        let freqs = freqs_for(&corpus);
        let encoded = encode_postings_doc_id_freq(&corpus, &freqs).expect("encode");
        group.throughput(Throughput::Elements(corpus.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(label), &encoded, |b, bytes| {
            b.iter(|| {
                let (ids, fr) = decode_postings_doc_id_freq(black_box(bytes)).expect("decode");
                black_box((ids, fr));
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_encode_doc_ids,
    bench_decode_doc_ids,
    bench_cursor_walk,
    bench_encode_postings,
    bench_decode_postings,
);
criterion_main!(benches);
