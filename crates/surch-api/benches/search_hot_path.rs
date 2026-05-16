//! Criterion bench: in-process scoring hot path.
//!
//! Goal: isolate the cost of `surch_api::search::run_search` (query
//! parse + match + score + top-K) from HTTP / serde overhead. The
//! existing end-to-end harness (`target/bench-mon/bench.sh`) reports
//! `took=0` on most queries because the HTTP round-trip dominates the
//! wall clock — useless to compare scoring-side optimisations.
//!
//! Each bench group indexes a single 5 000-doc corpus once (during
//! Criterion's `iter_batched` setup phase) and then measures the cost
//! of a single `run_search` call on that frozen state. Setup cost is
//! NOT folded into the measurement.
//!
//! Corpus: BEIR SciFact when available
//! (`$SURCH_BEIR_SCIFACT_PATH` → falls back to the in-repo default
//! `target/beir/scifact/corpus.jsonl`); synthetic 5 000-doc fallback
//! when the file is missing, so the bench still runs in CI / on
//! contributors' machines without the BEIR fixtures.
//!
//! Run: `cargo bench --bench search_hot_path -p surch-api`.
//! See `docs/ops/perf-bench.md` for baseline / regression workflow.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use serde_json::{json, Value};

use surch_api::search::{parse_search_request, run_search, SearchRequest};
use surch_api::state::{AppState, DocumentWriteOperation};

const INDEX: &str = "bench_scifact";
const CORPUS_TARGET_SIZE: usize = 5_000;
/// Deterministic seed for the synthetic-corpus fallback. Lifting this
/// value here keeps the bench reproducible across runs.
const SYNTHETIC_SEED: u64 = 0x5C1F_AC75_BEE5_0000;

/// Indexed bench state, built once and shared across all groups via a
/// `OnceLock`. Cloning `AppState` is cheap (it is an `Arc`-wrapper) so
/// the per-iter clone in `iter_batched` does not contaminate the
/// measurement.
fn shared_state() -> &'static AppState {
    static STATE: OnceLock<AppState> = OnceLock::new();
    STATE.get_or_init(build_state)
}

fn build_state() -> AppState {
    let state = AppState::default();
    let docs = load_corpus();
    let ops: Vec<DocumentWriteOperation> = docs
        .into_iter()
        .enumerate()
        .map(|(i, source)| DocumentWriteOperation::Index {
            index: INDEX.to_owned(),
            id: format!("doc-{i:06}"),
            source,
            status: 201,
        })
        .collect();
    state.apply_document_writes(ops);
    state
}

/// Returns the 5 000-doc corpus the benches score against. Reads BEIR
/// SciFact when present; otherwise generates a deterministic synthetic
/// corpus with the same `{title, text}` shape.
fn load_corpus() -> Vec<Value> {
    if let Some(path) = scifact_path() {
        if let Ok(docs) = read_scifact(&path) {
            if !docs.is_empty() {
                return docs;
            }
        }
        eprintln!(
            "search_hot_path: SciFact corpus at {} unreadable or empty; \
             falling back to synthetic corpus",
            path.display()
        );
    }
    synthetic_corpus(CORPUS_TARGET_SIZE)
}

fn scifact_path() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("SURCH_BEIR_SCIFACT_PATH") {
        let path = PathBuf::from(value);
        if path.exists() {
            return Some(path);
        }
    }
    // Default path used by `scripts/bench/scifact-ndcg.sh`.
    let default = PathBuf::from("/home/antoinefa/src/surch/target/beir/scifact/corpus.jsonl");
    if default.exists() {
        return Some(default);
    }
    None
}

fn read_scifact(path: &Path) -> std::io::Result<Vec<Value>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut docs = Vec::with_capacity(CORPUS_TARGET_SIZE);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let raw: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let title = raw
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let text = raw
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        docs.push(json!({ "title": title, "text": text }));
        if docs.len() >= CORPUS_TARGET_SIZE {
            break;
        }
    }
    Ok(docs)
}

/// Deterministic synthetic corpus. We use a SplitMix64 PRNG so the
/// shape is independent of the host crypto / `rand` versions. Each doc
/// carries a `title` (~6 tokens) and a `text` (~80 tokens) drawn from a
/// 200-word vocabulary seeded with English stems frequent enough to
/// guarantee the bench queries (`"rust"`, `"data"`, …) hit a sizeable
/// number of documents.
fn synthetic_corpus(size: usize) -> Vec<Value> {
    let vocab: Vec<&'static str> = SYNTHETIC_VOCAB.to_vec();
    let mut rng = SplitMix64::new(SYNTHETIC_SEED);
    let mut docs = Vec::with_capacity(size);
    for _ in 0..size {
        let title = synthesise_text(&mut rng, &vocab, 4, 8);
        let text = synthesise_text(&mut rng, &vocab, 60, 100);
        docs.push(json!({ "title": title, "text": text }));
    }
    docs
}

fn synthesise_text(rng: &mut SplitMix64, vocab: &[&str], lo: usize, hi: usize) -> String {
    let span = (hi - lo) as u64;
    let count = lo + (rng.next() % span.max(1)) as usize;
    let mut out = String::with_capacity(count * 6);
    for i in 0..count {
        if i > 0 {
            out.push(' ');
        }
        let idx = (rng.next() as usize) % vocab.len();
        out.push_str(vocab[idx]);
    }
    out
}

/// Vocabulary used by the synthetic-corpus fallback. Picked so the
/// bench queries below land on a non-empty match set (~hundreds of
/// docs per term over 5 k docs).
const SYNTHETIC_VOCAB: &[&str] = &[
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
    "speed",
    "throughput",
    "latency",
    "cache",
    "shard",
    "segment",
    "posting",
    "list",
    "term",
    "frequency",
    "inverse",
    "bm25",
    "tokenizer",
    "analyzer",
    "filter",
    "lower",
    "case",
    "ascii",
    "fold",
    "stem",
    "suffix",
    "prefix",
    "wildcard",
    "phrase",
    "boolean",
    "must",
    "should",
    "filter",
    "range",
    "from",
    "size",
    "track",
    "total",
    "hits",
    "stored",
    "source",
    "highlight",
    "aggregation",
    "bucket",
    "terms",
    "cardinality",
    "composite",
    "histogram",
    "date",
    "calendar",
    "interval",
    "month",
    "year",
    "day",
    "week",
    "weight",
    "function",
    "factor",
    "gauss",
    "decay",
    "linear",
    "exp",
    "log",
    "boost",
    "constant",
    "scope",
    "the",
    "of",
    "and",
    "to",
    "in",
    "for",
    "on",
    "with",
    "by",
    "from",
    "this",
    "that",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "we",
    "our",
    "their",
    "his",
    "her",
    "its",
    "rust",
    "lang",
    "rustacean",
    "tokio",
    "async",
    "future",
    "executor",
    "thread",
    "pool",
    "spawn",
    "await",
    "select",
    "stream",
    "iter",
    "result",
    "error",
    "thiserror",
    "anyhow",
    "serde",
    "json",
    "yaml",
    "toml",
    "config",
    "load",
    "save",
    "store",
    "write",
    "read",
    "snapshot",
    "restore",
    "recover",
    "compact",
    "merge",
    "rebuild",
    "warmup",
    "bench",
    "criterion",
    "harness",
    "report",
    "html",
    "regression",
    "baseline",
    "improve",
    "optimise",
    "profile",
    "flame",
    "graph",
    "callgrind",
    "perf",
    "linux",
    "kernel",
    "syscall",
    "io",
    "uring",
    "epoll",
    "select",
    "poll",
    "axum",
    "tower",
    "service",
    "layer",
    "middleware",
    "trace",
    "span",
    "event",
    "log",
    "metric",
    "counter",
    "gauge",
    "histogram",
    "exporter",
    "prometheus",
    "opentelemetry",
    "otlp",
    "grpc",
    "tonic",
    "client",
    "server",
    "tls",
    "auth",
    "token",
    "jwt",
    "cookie",
    "session",
    "user",
    "tenant",
    "rbac",
    "policy",
    "guard",
    "audit",
    "deny",
    "allow",
    "scope",
    "claim",
    "scope",
    "rolling",
    "release",
    "candidate",
    "stable",
    "beta",
    "nightly",
    "msrv",
    "edition",
    "feature",
    "flag",
    "gate",
    "branch",
    "main",
    "trunk",
    "patch",
    "minor",
    "major",
    "semver",
    "deprecated",
    "remove",
    "split",
    "rename",
    "move",
    "extract",
    "inline",
    "fold",
    "unfold",
    "spread",
];

/// Tiny SplitMix64. ~3 lines of arithmetic, deterministic, zero
/// dependencies, ample mixing for an offline bench corpus.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn parse(body: &str) -> SearchRequest {
    parse_search_request(body).expect("bench query body should parse cleanly")
}

fn bench_match_all(c: &mut Criterion) {
    let state = shared_state();
    let request = parse(r#"{"query":{"match_all":{}},"size":10}"#);
    let mut group = c.benchmark_group("match_all");
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::from_parameter("size=10"), |b| {
        b.iter(|| run_search(state, &[INDEX.to_owned()], &request));
    });
    group.finish();
}

fn bench_match_simple(c: &mut Criterion) {
    let state = shared_state();
    let request = parse(r#"{"query":{"match":{"title":"rust"}},"size":10}"#);
    let mut group = c.benchmark_group("match_simple");
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::from_parameter("title=rust,size=10"), |b| {
        b.iter(|| run_search(state, &[INDEX.to_owned()], &request));
    });
    group.finish();
}

fn bench_bool_must_2(c: &mut Criterion) {
    let state = shared_state();
    let request = parse(
        r#"{
            "query": { "bool": { "must": [
                { "match": { "title": "rust" } },
                { "match": { "text": "data" } }
            ] } },
            "size": 10
        }"#,
    );
    let mut group = c.benchmark_group("bool_must_2");
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::from_parameter("title+text,size=10"), |b| {
        b.iter(|| run_search(state, &[INDEX.to_owned()], &request));
    });
    group.finish();
}

fn bench_multi_match(c: &mut Criterion) {
    let state = shared_state();
    let request = parse(
        r#"{
            "query": { "multi_match": {
                "query": "rust data",
                "fields": ["title", "text"]
            } },
            "size": 10
        }"#,
    );
    let mut group = c.benchmark_group("multi_match");
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::from_parameter("2-fields,size=10"), |b| {
        b.iter(|| run_search(state, &[INDEX.to_owned()], &request));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_match_all,
    bench_match_simple,
    bench_bool_must_2,
    bench_multi_match,
);
criterion_main!(benches);
