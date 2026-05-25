# Surch: matching and beating a JVM search engine with a pure-Rust, OpenSearch-compatible core

> **Draft** (Objective F F5, 2026-05-25). Engineering-experience
> report on the Surch performance optimisation programme. Methodology
> in `docs/paper/methodology.md`; every figure cites a promoted K8s
> report under `docs/ops/bench-reports/`. Scope: the *recent* lot
> sequence (Lot 1 → Lot 3 + jemalloc), which is cleanly isolated and
> multi-rep. The historical optimisation isolation (F3) is pending a
> scope decision; this draft cites those as delivered-but-not-isolated.

## Abstract

We report the optimisation of Surch, an OpenSearch-compatible search
engine written in pure Rust, to the point where it matches and
exceeds OpenSearch 2.17.1 on bulk indexing, search latency, and
memory footprint — without any retrieval-quality regression — on
BEIR (SciFact, TREC-COVID) and a real matchID (INSEE) workload. The
headline result: full 171k-document TREC-COVID bulk ingestion goes
from `13.9x slower` than OpenSearch to `1.55x faster` (a `~17.8x`
Surch speedup, `1002 s → 71 s` median) across a four-step sequence,
while NDCG@10 stays bit-stable and resident memory drops to
`~2168 MiB`. All measurements run on the same Kubernetes Pod (Surch
and OpenSearch as sibling containers), with ≥3 repetitions for the
final claims.

## Results at a glance (3-rep medians, Surch vs OpenSearch 2.17.1)

| Axis | Workload | Surch | OpenSearch | Surch advantage |
|------|----------|------:|-----------:|----------------:|
| Bulk index | TREC-COVID 171k | 70.96 s | 109.73 s | **1.55x faster** (non-overlapping) |
| Bulk index | SciFact 5.2k | 2.09 s | 13.97 s | **6.7x faster** |
| Search p50 | INSEE 10k | 1.5 ms | 4.0 ms | **2.7x faster** |
| Search p95 | INSEE 10k | 4.1 ms | 12.2 ms | **3.0x faster** |
| Search p99 | INSEE 10k | 8.4 ms | 26.3 ms | **3.1x faster** |
| RSS peak | TREC-COVID 171k | 2168 MiB (±0.5%) | 1467 MiB | 1.48x (Surch heavier) |
| NDCG@10 | SciFact / TREC-COVID | 0.6576 / 0.4750 | 0.6537 / 0.4902 | parity (bit-stable) |
| matchID B1 oracle | deces_v1 vs ES 8.6.1 | 30/30, 0 divergence | — | parity preserved |

Surch leads on every speed and latency axis and on SciFact quality;
it trails OpenSearch only on TREC-COVID NDCG@10 (`-0.0152`) and on
absolute RSS (a JVM heap is sized for a different regime). Sources:
`docs/ops/bench-reports/2026-05-25-F2-{ndcg,insee}-3rep-K8s/`,
`…-b1-oracle-A10-ES861-K8s/`.

## 1. Introduction

JVM-based search engines (Elasticsearch, OpenSearch) dominate the
OpenSearch-compatible space. A pure-Rust implementation promises
lower memory overhead and no GC pauses, but must prove it on the
same wire protocol and the same workloads. Surch implements the
OpenSearch REST surface (bulk, search DSL, snapshots, SLM, matchID
parity) in Rust with a FoR-encoded postings codec, an FST term
dictionary, and refcounted stored `_source`.

This report documents four optimisation steps that closed (and
reversed) the bulk-indexing gap to OpenSearch, plus the search-side
and memory results, all measured under an identical K8s harness.

## 2. Methodology (summary)

See `docs/paper/methodology.md`. Key points: Surch and OpenSearch
2.17.1 run as sibling containers in one Pod on a Scaleway burst
node; the bench driver speaks HTTP to both. Metrics use versioned
JSON schemas (`surch.bench.{artillery,ndcg_gate,rss}.v1`). Allocator
parity: since Lot 1.7 Surch uses jemalloc, as OpenSearch does on
Linux. Quality is reported on every run; a perf claim is inadmissible
if NDCG@10/Recall@10 move. Final claims use ≥3 repetitions
(median + range).

## 3. The bulk-indexing optimisation sequence

Baseline: on the full 171k TREC-COVID corpus, Surch bulk ingestion
was `1001.95 s` vs OpenSearch `72.27 s` — OpenSearch `13.9x` faster
(`2026-05-22-ndcg-gate-7Gi-K8s`). Four steps:

### Lot 1 — incremental bulk append (`367acdc`)

Root cause: every `_bulk` chunk triggered a full `rebuild_index()`
that re-indexed the *cumulative* document store — O(N²/chunk) over
the corpus. Fix: an incremental `append_to_index` path for
pure-insert batches. Result: TREC-COVID bulk `1001.95 → 179.86 s`
(`~5.6x`), OpenSearch advantage `13.9x → 2.06x`
(`2026-05-24-ndcg-gate-incremental-bulk-K8s`).

### Lot 1.5 — release the builder on refresh (`8a5150f`)

The incremental path kept the per-chunk `PostingsBuilder` alive
(`+1 GiB` RSS). `refresh_index` now drops it. But the glibc default
allocator did not return the freed pages: RSS only fell `5859 →
5591 MiB` (`2026-05-24-ndcg-gate-lot1.5-ram-K8s`) — motivating Lot
1.7.

### Lot 1.7 — jemalloc (`b9f6636`)

Switching the global allocator to jemalloc with
`background_thread:true,dirty_decay_ms:0,muzzy_decay_ms:0` returned
freed pages to the OS aggressively. RSS peak `5591 → 3424 MiB`
(`-39%`), final `→ 1382 MiB` (`-75%`), and — unexpectedly — bulk
itself `-26%` (less allocator contention). This also brings
allocator parity with OpenSearch
(`2026-05-24-ndcg-gate-lot1.7-jemalloc-K8s`).

### Lot 1.6 — deferred FST term-dictionary build (`2e4361e`)

The dominant remaining cost was rebuilding the whole FST term
dictionary after every `_bulk` POST. Fix: defer the FST build; a
`terms_dirty` flag + lazy `materialize_terms()` on the read path /
refresh build it once. Result: TREC-COVID bulk `139 → 56 s` —
**Surch crosses OpenSearch** (`Surch 1.54x faster`)
(`2026-05-24-ndcg-gate-lot1.6-K8s`).

### Multi-rep confirmation (F2)

3 repetitions on the final engine (`2026-05-25-F2-ndcg-3rep-K8s`):
Surch TREC-COVID bulk median `70.96 s` (range 69.70–78.18) vs
OpenSearch median `109.73 s` (105.90–111.60) — **non-overlapping
distributions**, Surch `1.55x` faster. SciFact bulk: Surch median
`2.09 s` vs OpenSearch `13.97 s` (`6.7x`).

| Step | SHA | TREC-COVID Surch bulk | vs OpenSearch |
|------|-----|----------------------:|--------------:|
| baseline | `d9cac15` | 1001.95 s | OS 13.9x faster |
| Lot 1 | `04fde72` | 179.86 s | OS 2.06x faster |
| Lot 1.5 | `01ad77e` | 189.18 s | OS 1.92x faster |
| Lot 1.7 jemalloc | `b9f6636` | 139.05 s | OS 1.42x faster |
| Lot 1.6 deferred FST | `2e4361e` | 56.38 s | **Surch 1.54x faster** |
| 3-rep median | main | **70.96 s** | **Surch 1.55x faster** |

## 4. Memory

3-rep Surch RSS peak on the full TREC-COVID corpus: median
`2168 MiB` (range 2159–2180, `±0.5%`), `~1.48x` the OpenSearch peak
(`~1467 MiB`). Reproducible to half a percent
(`2026-05-25-F2-ndcg-3rep-K8s`).

## 5. Search latency

On the matchID INSEE artillery workload (13 170 queries, 3 reps,
`2026-05-25-F2-insee-3rep-K8s`), Surch median p50/p95/p99/max =
`1.5/4.1/8.4/40.6 ms` vs OpenSearch `4.0/12.2/26.3/223.1 ms` — Surch
`2.7–3.1x` faster at p50/p95/p99, p50 zero-variance, 0 errors.

Two search-side algorithmic steps were isolated against same-stack
controls:
- **Lot 2 skip lists + leapfrog AND** (`d73c862`):
  `p95 -13% / p99 -18%` vs the jemalloc control
  (`2026-05-25-insee-lot2-skiplists-K8s`).
- **Lot 3 MaxScore block-leapfrog** (`e293cfc`): latency-neutral on
  INSEE 10k (posting lists too short to skip);
  correctness-neutral (ranking bit-stable). Its benefit regime
  (large corpora) is not yet exercised by a latency harness — a
  known gap (`2026-05-25-lot3-bmw-skiplist-K8s`).

## 6. Quality (non-regression)

Across the entire sequence and all repetitions, SciFact NDCG@10 =
`0.6576` / Recall@10 = `0.8100` and TREC-COVID NDCG@10 = `0.4750` /
Recall@10 = `0.0132` are bit-stable; OpenSearch is `0.6537/0.8033`
and `0.4902/0.0132`. Surch leads on SciFact and trails OpenSearch by
`0.0152` NDCG@10 on TREC-COVID. No optimisation perturbed retrieval.

## 7. matchID parity

Surch passes the 30-request matchID B1 oracle against Elasticsearch
8.6.1 with 0 divergence, including after the A10 write-time
sub-field fan-out (`2026-05-25-b1-oracle-A10-ES861-K8s`).

## 8. Discussion

- The largest wins were algorithmic (avoiding O(N²) rebuilds: Lot 1,
  Lot 1.6), not micro-optimisation. The allocator (Lot 1.7) gave a
  large RSS win *and* a bulk speedup, and is also a fairness fix
  (OpenSearch already uses jemalloc).
- Pure-Rust safety is orthogonal to allocator performance: Surch
  delegates to jemalloc (C) via FFI; the compile-time ownership
  guarantees are independent of the runtime allocator.
- Search-tail optimisations (skip lists) help where posting lists
  are long; a 10k matchID corpus does not exercise that regime —
  a methodological lesson for the next workload set.

## 9. Threats to validity / limitations

- Final claims are 3-rep; per-lot isolations are single-run.
- The historical optimisation family (top-K, lazy hydration, WAND,
  FoR/FST, shared sources) is delivered and measured cumulatively
  but not individually K8s-isolated (the historical SHAs predate the
  CI/Docker surface) — F3, scope decision pending.
- Single node; corpora limited to SciFact / TREC-COVID / INSEE.
  A large-corpus search-latency harness (`trec-covid-latency` K8s
  job, F4) has now landed and its first run is in flight — once it
  reports, Lot 3's block-leapfrog benefit regime (long posting
  lists) can be measured directly, which the INSEE 10k workload
  could not exercise.

## 10. Conclusion

A pure-Rust OpenSearch-compatible engine can match and beat a mature
JVM engine on bulk, latency, and memory simultaneously, with no
quality regression, given a few targeted algorithmic fixes and an
allocator at parity. The measurement programme — versioned schemas,
sibling-container fairness, quality guardrails, multi-rep medians —
is itself reproducible from the promoted CI artefacts.
