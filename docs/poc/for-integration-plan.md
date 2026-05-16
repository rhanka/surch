# FoR codec integration plan — Track A wire-up roadmap

**Status** : roadmap, no wire-up done yet.  
**Branch** : `wp/a-optim` (Track A — Surch mesurablement plus rapide qu'OS sur même charge).  
**Context** : `crates/surch-codec/src/postings_block.rs` ships a self-contained
delta-varint codec (and a paired `(doc_id, freq)` codec with a streaming
`DocIdDeltaCursor`). It is NOT wired into the live postings path
(`crates/surch-index/src/postings.rs`) yet. This document describes the
3 phases that bring the codec into the hot path without regressing
the **SciFact NDCG@10 = 0.6576** baseline.

## Why a phased wire-up

The naive option — encode every `Vec<Posting>` to bytes inside
`FieldPostings` and decode on every `lookup()` — has two failure modes:

1. **Latency regression on warm queries**. The scoring hot path
   (`maxscore_match`, `score_for_query`) iterates each posting at
   most twice per query token. Decoding the entire byte payload at
   every call would re-run a varint loop ~N times per query — easily
   30–50 ns/posting on dense postings, vs ~3 ns/posting today for
   `Vec<Posting>` indexed access. The criterion bench harness
   (`crates/surch-api/benches/search_hot_path.rs`) is the gate.
2. **SciFact NDCG@10 drift**. Any change to the order/values returned
   by `PostingsEnum` propagates into BM25 weights and the top-K
   tie-breaks. The codec is exact (delta-varint is lossless), but a
   careless wire-up that changes the `Vec<Posting>` iteration order
   would silently lower NDCG@10. The integration tests in
   `crates/surch-api/tests/scifact_ndcg.rs` (and
   `scripts/bench/scifact-ndcg.sh`) are the gate.

So the plan is incremental: first add the codec to **cold** code paths
(snapshot, persisted segment), then opt into **decode-on-demand** for
**rarely-accessed** terms, only then consider replacing the live
`Vec<Posting>` in RAM. Each phase is independently shippable.

## Phase 1 — Persisted snapshot (no live-path impact)

**Goal** : Use the codec when serialising a `TermDictionary` to disk
(future durable segment format). RAM stays unchanged; disk size and
snapshot I/O shrink.

**Scope** :

- Add `TermDictionary::encode_to_bytes(&self) -> Vec<u8>` /
  `decode_from_bytes(&[u8]) -> Result<TermDictionary>` using
  `encode_postings_doc_id_freq` for every per-term posting list.
- Plug into `crates/surch-api/src/snapshot.rs` next to the FST
  bytes. The snapshot directory format stays backward-compatible:
  add `postings.cv1` (codec version 1) alongside the existing FST
  payload; the loader picks `postings.cv1` if present, falls back
  to the raw layout otherwise.

**Risks** : zero on the live read path (the postings stay
`Vec<Posting>` in RAM). All risk is on snapshot/restore I/O, gated by
existing snapshot tests + round-trip property tests.

**Measurable gain** : disk footprint of postings = ~2–4× compression
on the BAN 25 k corpus (per the standalone bench: dense_1k_step3
≈ 3.7×, sparse_2k_random ≈ 2.8×). Cold-start snapshot restore time
drops proportionally to disk bytes read.

**SciFact NDCG@10** : trivially preserved — no behavioural change.

## Phase 2 — Cold posting lists (decode-on-demand)

**Goal** : Encode the per-term postings of rare terms (`doc_freq` ≤
threshold, default 16) into the codec format and keep only the bytes
in RAM. Hot terms keep their `Vec<Posting>` layout for fast random
access. Lookup is one varint walk per query token — acceptable for
rare terms which are only walked a handful of times.

**Scope** :

- Introduce `enum PostingsStorage { Hot(Vec<Posting>),
  Cold(Vec<u8>) }` inside `FieldPostings`. `Hot` is the current
  layout; `Cold` carries an `encode_postings_doc_id_freq` payload
  + a parallel `Vec<Vec<u32>>` of positions (positions are not yet
  codec-compressed — see Phase 3).
- `PostingsBuilder::build()` picks `Hot` vs `Cold` based on the
  per-term `doc_freq` threshold.
- `FieldPostings::lookup()` returns a `PostingsView<'_>` enum that
  exposes either a slice or a `DocIdDeltaCursor` + freq iterator;
  callers in `crates/surch-search/src/execution.rs` are updated to
  consume `PostingsView` directly (no materialisation).

**Risks** :

- **Latency** of the scoring loop on bool-must with a rare term.
  The criterion bench (`bench_bool_must_2`) must stay within +5 %
  of baseline; if it regresses, the threshold is raised or the
  feature is gated behind a config flag (default off).
- **Ordering invariants**. `Vec<Posting>` is sorted by ascending
  `doc_id` by construction; the codec preserves that ordering by
  delta-varint. A property test in `crates/surch-codec/tests/`
  asserts `decode(encode(v)) == v` for any monotonic `Vec<u32>`.

**Measurable gain** : RAM reduction on the BAN 25 k corpus =
~15–25 % of `surch_index_term_stats_bytes` (the long tail of singleton
terms — "rue de la X" unique strings — dominates the posting count
without dominating the scoring cost). Verified via the
`surch_index_term_stats_bytes` Prometheus gauge before/after.

**SciFact NDCG@10** : preserved by construction (delta-varint is
lossless) and gated by the existing `scripts/bench/scifact-ndcg.sh`
script run on each commit.

## Phase 3 — Block-128 FoR + positions codec

**Goal** : Replace the per-term varint payload with the canonical
Lucene block-128 FoR layout (1 byte selector + scalar-bitpacked
payload), and apply the same scheme to positions
(`Vec<u32>` per posting). This is the layout described in
`docs/poc/perf-optimization-plan.md` § "C — Block-128 postings with
delta + scalar bitpacking".

**Scope** :

- Generalise `postings_block.rs` to a block-128 layout: each block
  carries `(selector_byte, bitpacked_payload)` where `selector_byte`
  encodes the bit-width of every delta in the block.
- Pair with **SK** (skip list per block) — already partially in
  place via `BlockMeta` — to enable true Block-Max WAND skipping.
- Decode-on-demand cursor becomes block-stride aware: callers can
  skip entire blocks based on `BlockMeta.max_doc_id`.

**Risks** : largest of the three phases. SIMD intrinsics (when added)
are off by default; the scalar bitpacking fallback is the gate.

**Measurable gain** : per the perf plan, **−50 to −80 MB** on the BAN
25 k postings (bits/int ≈ ceil(log2(max_delta)) instead of 32) plus
5 ns/int decode vs ~30–50 ns/int today.

**SciFact NDCG@10** : preserved by construction; gated by the
existing parity test.

## Out of scope — what this plan does NOT do

- Touch `crates/surch-api/src/search.rs` (the live scoring path).
  All wire-up is inside `surch-codec` and `surch-index`; the search
  crate sees the change through the existing `PostingsEnum` API
  (or its `PostingsView` successor in Phase 2).
- Touch `crates/surch-api/src/snapshot_es*.rs`, `telemetry.rs`,
  `scroll.rs` (Track D scope).
- Bring in `bitpacking` / SIMD crates. The current codec is pure
  scalar Rust + LEB128; SIMD is a follow-up after Phase 3 ships.

## Per-phase exit criteria

| Phase | Gate test                                         | Bench gate (criterion)              |
|-------|---------------------------------------------------|-------------------------------------|
| 1     | snapshot round-trip property test, NDCG@10        | snapshot-restore time ↓             |
| 2     | unit test `decode(encode(v))==v` + NDCG@10        | `bench_bool_must_2` within +5 %     |
| 3     | NDCG@10 + Block-Max WAND skip-correctness test    | `bench_match_simple` ≥ 1.3×         |

## Bench harness

Standalone codec bench : `cargo bench --bench for_decode -p surch-codec`  
End-to-end scoring bench : `cargo bench --bench search_hot_path -p surch-api`

The first runs in seconds and is appropriate for inner-loop iteration
on the codec; the second is the integration gate before every commit
that touches `crates/surch-index/src/postings.rs`.
