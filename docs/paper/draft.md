# Surch: matching and beating a JVM search engine with a pure-Rust, OpenSearch-compatible core

> **Draft** (Objective F F5, 2026-05-31). Engineering-experience
> report on the Surch performance optimisation programme. Methodology
> in `docs/paper/methodology.md`; every figure cites a promoted K8s
> report under `docs/ops/bench-reports/`. Scope: the *recent* lot
> sequence (Lot 1 → Lot 3 + jemalloc), which is cleanly isolated and
> multi-rep. Isolating the historical optimisation family (F3) is now
> planned (backlog, after the Track D priority); until then this draft
> cites those as delivered-but-not-yet-individually-isolated.
> Plot-ready data for the headline curves (bulk-by-lot, RSS-by-lot,
> latency-by-corpus) is in `docs/paper/figures/` (CSV + provenance +
> SVG renders).

## Abstract

We report the optimisation of Surch, an OpenSearch-compatible search
engine written in pure Rust, to the point where it matches and
exceeds OpenSearch 2.17.1 on bulk indexing, small-corpus search
latency, and memory footprint — without any retrieval-quality
regression — on BEIR (SciFact, TREC-COVID) and a real matchID (INSEE)
workload. The headline result: full 171k-document TREC-COVID bulk
ingestion goes from `13.9x slower` than OpenSearch to `1.55x faster`
(a `~17.8x` Surch speedup, `1002 s → 71 s` median) across a four-step
sequence, while NDCG@10 stays bit-stable and resident memory later
drops to `907 MiB` (`0.62x` the OpenSearch peak) after removing unused
in-memory posting positions. The large-corpus cache-on latency result
is reported with its LRU caveat, and raw cache-off TREC-COVID p50
remains an explicit open front. All measurements run on the same
Kubernetes Pod (Surch and OpenSearch as sibling containers), with ≥3
repetitions for the final OpenSearch claims.

## Results at a glance — four axes, Surch vs OpenSearch / Elasticsearch

| Axis | vs OpenSearch 2.17.1 (BEIR / INSEE) | vs Elasticsearch 8.6.1 (matchID / deces) |
|------|-------------------------------------|------------------------------------------|
| **1. Bulk indexing** | **1.55x faster** on TREC-COVID 171k (`70.96 s` vs `109.73 s`, 3-rep median) and **6.7x faster** on SciFact (`2.09 s` vs `13.97 s`). | **Parity / slight lead** on deces 1.36M (`104.2 s` vs `115.9 s`, 3-rep median); honest nuance: ES best run `91.5 s` still beats Surch best `100.8 s`. |
| **2. Search latency** | **2.7-3.1x faster** on INSEE 10k (`p50 1.5/4.0 ms`, `p95 4.1/12.2 ms`, `p99 8.4/26.3 ms`); **1.83x slower** on raw large-corpus TREC-COVID cache-OFF p50 (`309 ms` vs `169 ms`). The cache-ON `~354x` result is LRU-masked, not a raw-engine claim. | **2.45x faster p50** on deces 1.36M (`2.0 ms` vs `4.9 ms` under the concurrent probe; `~1.6x` at equal `WORKERS=2`). Tail still trails (`p95/p99` Surch `14/21 ms` vs ES `11/15 ms`) until the structural compact/SIMD postings work. |
| **3. Memory (RSS)** | **0.62x, Surch lighter** on TREC-COVID 171k after the position drop (`907 MiB` vs `1465 MiB`; prior 3-rep Surch median was `2168 MiB`). | 28M-scale measurement pending. |
| **4. Quality / parity** | **Parity across 4 BEIR datasets**: SciFact `0.6576/0.6537`, TREC-COVID `0.4750/0.4902`, NFCorpus `0.3033/0.3034`, FiQA `0.2294/0.2389` (Surch / OpenSearch NDCG@10). | **Bit-exact parity** on matchID oracles: B1 `30/30`, B2 `8/8`, `0` divergence. |

This is the canonical performance framing: four product axes, with
OpenSearch and Elasticsearch separated instead of mixing cache-on,
cache-off, BEIR, and matchID results in one flat scorecard. Surch wins
bulk, small-corpus search latency, memory, and quality/parity on the
measured fronts. The open fronts are also explicit: raw large-corpus
search p50 vs OpenSearch, deces p95/p99 tail vs Elasticsearch, and
28M-scale RSS/indexation. Sources:
`docs/ops/bench-reports/2026-05-25-F2-{ndcg,insee}-3rep-K8s/`,
`docs/ops/bench-reports/2026-05-26-F3-lru-cache-isolation-trec-covid-K8s/`,
`docs/ops/bench-reports/2026-05-25-b2-oracle-deces-v2-ES861-K8s/`,
and `docs/paper/beat-elasticsearch-campaign.md`.

## A+F5 final readout

The Track A / F5 package is now the current publishable readout for the
performance programme: methodology, per-axis scorecard, optimisation
trajectory, explicit limitations, CSV figure data, and rendered SVG figures.
The headline numbers to carry forward are:

- Bulk: TREC-COVID 171k `1001.95 s -> 70.96 s` median for Surch; final
  head-to-head `70.96 s` vs OpenSearch `109.73 s` (`1.55x` faster).
- Bulk on rich matchID `deces` mapping: Surch `104.2 s` vs Elasticsearch
  `115.9 s` median on 1.36M docs; parity / slight lead, with ES retaining
  the best single run (`91.5 s`).
- Search latency: INSEE 10k Surch `p50/p95/p99 = 1.5/4.1/8.4 ms` vs
  OpenSearch `4.0/12.2/26.3 ms`.
- Large-corpus search: TREC-COVID cache-on is `~354x` p50 for Surch, but this
  is explicitly LRU-masked; cache-off raw p50 remains behind OpenSearch
  (`309 ms` vs `169 ms`).
- Memory: post-position-drop Surch RSS `907 MiB` vs OpenSearch `1465 MiB`
  on TREC-COVID 171k (`0.62x`, Surch lighter).
- Quality/parity: no optimisation regressed BEIR quality; matchID B1/B2
  oracles remain `0` divergence against Elasticsearch 8.6.1.

The next measurement should be the full `deces` indexation run, not another
local micro-optimisation. The 1.36M path is now strong enough to justify a
28M-scale ES/Surch indexation proof: run Elasticsearch first as the production
baseline, then Surch on the same dataprep corpus and report bulk duration,
throughput, RSS, final doc count, and any failure mode.

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

The F2 3-rep Surch RSS peak on the full TREC-COVID corpus was
`2168 MiB` (range 2159-2180, `±0.5%`), `~1.48x` the OpenSearch peak
(`~1467 MiB`) (`2026-05-25-F2-ndcg-3rep-K8s`). The later position-drop
optimisation (#9, `3ccdbc6`) removed the unused per-posting `Vec<u32>`
from the in-memory index and flipped the current memory comparison:
Surch RSS peak `907 MiB` vs OpenSearch `1465 MiB` on the same
TREC-COVID workload (`0.62x`, Surch lighter). This post-#9 point is a
single K8s run, but the margin is far larger than the previously
observed RSS variance; the 28M-scale memory measurement is still open.

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
  correctness-neutral (ranking bit-stable)
  (`2026-05-25-lot3-bmw-skiplist-K8s`).

### Large-corpus search latency (F4)

On the full 171k TREC-COVID corpus with real multi-term queries
(`2026-05-25-F4-trec-covid-latency-3rep-K8s`, 3 reps, 13 170 queries
each, 0 errors both engines), Surch median latency is
`p50 0.5 ms / p95 1.3 ms` (both zero-variance across reps) vs
OpenSearch `p50 176.9 ms / p95 481.4 ms` — two-to-three orders of
magnitude faster (`~354x` p50, `~370x` p95), and OpenSearch
*degrades* under load (p50 climbs to ~190 ms at 50 RPS) while Surch
stays flat. This is the long posting-list regime that INSEE 10k could
not reach. Retrieval equivalence is established two ways: NDCG@10
parity on the same corpus (Surch `0.4750` vs OpenSearch `0.4902`), and
an in-artifact hits-equivalence probe (`surch.bench.trec_hits.v1`)
showing all 50 queries return non-empty sets on both engines with
total matched-doc volume agreeing to `0.04 %` (Surch `7 507 757` vs
OpenSearch `7 510 550`). The result set is therefore not degenerate;
the measured `~354x` gap is still a cache-on workload result dominated
by Surch's result LRU, not proof that the raw scorer performs the same
work faster.

**Honest caveat — the `354x` is the cache, not the engine (F3 isolation, §9).**
The steady-state median is dominated by the per-query result LRU: the harness
replays 50 distinct queries → ~99.6% hit rate. With the cache **disabled**
(`2026-05-26-F3-lru-cache-isolation-trec-covid-K8s`), Surch's raw-engine latency
is `p50 309 / p95 532 / p99 624 / max 914 ms` against OpenSearch `169 / 415 /
598 / 1094 ms` on the *same* run — i.e. **Surch's raw scorer is `1.83x` SLOWER
than OpenSearch at the median** on TREC-COVID 171k (1.28x slower p95, ~parity
p99, 1.2x faster max). So large-corpus search latency is **not** a Surch win on
the raw engine — the LRU is what produces the headline. This is the campaign's
key honesty: the small-corpus INSEE 10k latency win (`2.7–3.1x`) is real and
cache-independent, but the large-corpus raw-engine latency is the **front still
to be won** (see the beat-Elasticsearch campaign, §11, and the read-path
optimisations it targets). Surch RSS peak on this corpus is `~907 MB` after the
posting-positions drop (optimisation #9, §11) — `0.62x` the OpenSearch peak,
down from `~2123 MB` (`1.45x`, heavier) before.

## 6. Quality (non-regression)

Across the entire sequence and all repetitions, SciFact NDCG@10 =
`0.6576` / Recall@10 = `0.8100` and TREC-COVID NDCG@10 = `0.4750` /
Recall@10 = `0.0132` are bit-stable; OpenSearch is `0.6537/0.8033`
and `0.4902/0.0132`. Surch leads on SciFact and trails OpenSearch by
`0.0152` NDCG@10 on TREC-COVID. No optimisation perturbed retrieval.

The quality result generalises beyond those two corpora: on two further
BEIR datasets (`2026-05-26-F4-beir-nfcorpus-fiqa-K8s`), Surch is
bit-identical to OpenSearch on NFCorpus (NDCG@10 `0.3033` vs `0.3034`,
Recall@10 identical) and within ~4 % on FiQA (`0.2294` vs `0.2389`). Across
all four BEIR datasets Surch tracks OpenSearch 2.17.1 — ahead on SciFact,
identical on NFCorpus, and a few percent behind on TREC-COVID / FiQA — so
the BM25 + analysis pipeline is competitive across domains, not tuned to one.

## 7. matchID parity

Surch passes the 30-request matchID B1 oracle against Elasticsearch
8.6.1 with 0 divergence, including after the A10 write-time
sub-field fan-out (`2026-05-25-b1-oracle-A10-ES861-K8s`) and after
A12, which feeds that projection into sort/aggregation on the read
path — still `30/30`, 0 divergence
(`2026-05-25-b1-oracle-A12-ES861-K8s`).

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
  FoR/FST, shared sources) is delivered and measured cumulatively;
  individual isolation (F3) is now in progress via measurement toggles
  on a throwaway `perf-isolation` branch (the historical SHAs predate the
  modern bench binaries, so replaying old commits directly does not
  build). Isolated results so far:
  **(a) WAND/MaxScore on TREC-COVID 171k cuts tail latency p99 −90% / max
  −92%, p50/p95 neutral** (`2026-05-26-F3-wand-isolation-trec-covid-K8s`)
  — a large-corpus tail optimisation, neutral on the short-list INSEE 10k;
  **(b) the per-query result LRU carries the bulk of the headline median
  advantage** (`2026-05-26-F3-lru-cache-isolation-trec-covid-K8s`): with the
  cache disabled, Surch p50 goes `0.5 ms → 309 ms` (−618x), making the raw
  scorer ~1.8x *slower* than OpenSearch at p50 on this workload and only
  faster at the extreme tail. The `~354x` figure is therefore a hot,
  low-cardinality best case (the artillery harness replays 50 distinct
  queries → ~99.6% cache hit), not a raw-engine claim — see §9 caveat.
  **(c) the top-K shortcut (bounded heap + lazy `_source` hydration) is the
  single largest tail optimisation** (`2026-05-26-F3-topk-isolation-trec-covid-K8s`):
  disabled, the full-scan path clones every matching `_source`, so cold
  high-frequency queries explode to p50 `7.3 s` (phase 1), global p99
  `5.3 ms → 4093 ms` (−770x), max `308 ms → 15.5 s` (−50x) — and Surch becomes
  far slower than OpenSearch on the cold path. The sub-ms steady state is the
  joint product of lazy hydration + the LRU; WAND caps the residual cold tail.
  Remaining family member (FST term dictionary) follows the same
  toggle-isolation method.
- Single node; corpora limited to SciFact / TREC-COVID / INSEE.
  A large-corpus search-latency harness (`trec-covid-latency` K8s
  job, F4) has landed and produced a 3-rep median verdict
  (`2026-05-25-F4-trec-covid-latency-3rep-K8s`): Surch is two-to-three
  orders of magnitude faster than OpenSearch on the 171k corpus, with
  zero-variance p50/p95. Engine work-equivalence is confirmed both by
  NDCG@10 parity and by an in-artifact hits-equivalence probe (all 50
  queries non-empty on both engines, total matched-doc volume within
  `0.04 %`). Lot 3's block-leapfrog benefit is exercised in this regime
  but its individual same-stack contribution is not separately
  quantified (by decision): it is reported as delivered and beneficial
  on large corpora, folded into the cumulative result, rather than
  isolated against a no-Lot-3 control.

## 10. Beat-Elasticsearch campaign (matchID `deces`, vs ES 8.6.1)

Sections 3–9 benchmark Surch against **OpenSearch 2.17.1**. A second,
ongoing campaign benchmarks Surch against **Elasticsearch 8.6.1** on the
**matchID `deces` corpus** (the French civil-death registry: a rich 28-field
mapping with `norm` analyzer, `.raw` keyword sub-fields, `edge_ngram`
autocomplete, `index_prefixes`, `geo_point`). matchID is the honest proving
ground — Surch is the product, not matchID-specific tuning. Measurements run in
the matchID CI (`matchID-project/matchID`, branch `surch-eval`): two isolated
runner jobs (one engine each, no cross-engine CPU contention), Surch swapped in
for Elasticsearch behind the real stack. Each optimisation is one
paper-traceable step (hypothesis → change → before/after on the same workflow →
verdict); the full log is `docs/paper/beat-elasticsearch-campaign.md`. The
no-cheat bar: real corpus, ≥3 reps for claims, cache ON *and* OFF, all
dimensions reported (so losses show as plainly as wins).

**#1 — Parallel bulk analysis (rayon, `dd3f528`).** The bulk write held one
write-lock that serialised the CPU-heavy per-doc analysis; ES/Lucene
parallelise across cores. Moving the (pure) analysis off-lock to a `par_iter`,
merging in input order (byte-identical postings), **eliminated the indexation
deficit on the rich deces mapping**: deces 1.36M bulk Surch **104.2 s** (3-rep
median, range ±3%) vs ES **115.9 s** (±15%) — from `18x slower` to parity /
slightly ahead, and ~5× more consistent (the no-GC predictability thesis).
Honest nuance: not a clean "always faster" (ES best run 91.5 s < Surch best
100.8 s); claim = parity + markedly more predictable.

**#9 — Drop per-posting positions (`3ccdbc6`).** No production read path consumes
index positions (BM25 reads `freq`; `match_phrase` re-tokenises `_source`; the
persisted codec excludes positions); they were kept only to derive `freq`.
Shrinking `Posting` from `{doc_id, freq, Vec<u32>}` (~32 B + heap Vec) to a
`Copy` `{doc_id, freq}` (8 B) — matching Lucene's `index_options`-gated
positions — **flipped the memory dimension**: TREC-COVID 171k RSS peak
`2168 → 907 MB` (−58%), now **0.62× the OpenSearch peak** (was 1.48× heavier),
with bit-stable NDCG and no bulk regression. Memory at scale is the in-memory
engine's structural risk vs ES (disk + page cache); this directly de-risks it.

**#6 — Hoist BM25 idf out of the per-doc loop (`afa3a21`).** The WAND hot path
re-ran config validation + the `ln()` idf per scored doc/block; a per-term
`Bm25TermScorer` precomputes them once (bit-identical kernel). Parity-safe, but
**measured neutral** on TREC-COVID cache-off (Surch p50 302 vs 309 ms baseline =
noise): the ~300 ms cache-off latency is dominated by **posting-list decode +
`_source` hydration**, not BM25 arithmetic. Reported honestly as a correct
micro-opt that is not the read-path bottleneck — and as the signal that
redirected the campaign to the decode/hydration path (read-path single-reader +
zero-copy postings, in progress).

**#2 — Field-name allocation in term aggregation (`3cdd1ab`).** Keyed the per-doc
term map on the term only (field attached once per unique term), dropping
field-`String` allocations `O(tokens) → O(unique-terms)`; parity-preserving
allocation reduction folded into the indexing lead.

### Performance-by-optimization trajectory (Track A)

This draft now tracks the optimisation path explicitly in campaign order, with
axis-aware outcomes and remaining gaps.

#### OpenSearch 2.17.1 route (Track A)

| Step | Main lever | Reported effect |
|------|------------|-----------------|
| Baseline (pre-lots) | Full bulk rebuild in `append_to_index` | TREC-COVID 171k bulk `1001.95 s` vs OpenSearch `72.27 s` (`13.9x` slower). |
| Lot 1 | Incremental bulk append (`367acdc`) | TREC-COVID bulk `1001.95 → 179.86 s` (`~5.6x` speedup for Surch vs baseline), OpenSearch delta drops `13.9x → 2.06x` slower. |
| Lot 1.5 | Drop `PostingsBuilder` on refresh (`8a5150f`) | RSS `5859 → 5591 MiB` after builder removal; no major throughput gain, allocator pressure remains high. |
| Lot 1.7 | Jemalloc (`b9f6636`) | RSS peak `5591 → 3424 MiB` (`-39%`) and final `5591 → 1382 MiB` (`-75%`); bulk `189.18 → 139.05 s` (`-26%`). |
| Lot 1.6 | Deferred FST materialization (`2e4361e`) | TREC-COVID bulk `139.05 → 56.38 s`; Surch becomes `1.54x` faster than OpenSearch on this workload. |
| F2 3-rep confirmation (`2026-05-25-F2-3rep`) | Stable median across 3 runs | TREC-COVID bulk `70.96 s` vs OpenSearch `109.73 s` (`1.55x` faster, non-overlap); SciFact `2.09 s` vs `13.97 s` (`6.7x`). |
| Lot 2 | Skip lists + leapfrog AND (`d73c862`) | INSEE tail `p95` and `p99` reduce `-13%` and `-18%` respectively, ranking unchanged. |
| Lot 3 | MaxScore block-leapfrog (`e293cfc`) | INSEE 10k impact neutral (posting lists mostly short); long-tail readiness improved in design. |
| F3 WAND isolation (`2026-05-26-F3-wand-isolation-trec-covid-K8s`) | WAND/MaxScore enabled path only | TREC-COVID raw-tail `p99 51.4 → 5.3 ms` and `max 3915 → 308 ms` (`~90%+` cuts) with `p50/p95` neutral. |
| F3 top-K isolation (`2026-05-26-F3-topk-isolation-trec-covid-K8s`) | Top-K + lazy hydration enabled | Disabling it alone explodes de facto cold path (`full` `p99 5.3 → 4093 ms`, `max 308 → 15500 ms`); enabled case is the shipped baseline. |
| #9 | Drop per-posting positions (`3ccdbc6`) | TREC-COVID RSS peak `2168 → 907 MiB`; memory flips to `0.62x` OpenSearch peak. |

#### matchID deces route vs Elasticsearch 8.6.1 (track D/Track A boundary)

| Step | Main lever | Reported deces latency effect |
|------|------------|------------------------------|
| Baseline (pre-campaign) | Raw decode + docid intersection chain | p50 around `4513 ms` (`~1200x` slower than ES). |
| Campaign setup 1 (early deces cleanup) | `should`-intersection + `function_score` unwrap | p50 `4513 → 87.2 ms`; still large gap but no longer catastrophic. |
| Campaign setup 2 (`#10`, `8aae6a1`) | Dense single-token candidate intersection | Full path p50 `~70 ms`; cumulated improvement `4513 → 70 ms`. |
| Campaign setup 3 (`#11`, `a6fa7aa`) | Leapfrog conjunction + full candidate path redesign | No measurable gain on deces; neutral, kept for safety. |
| Campaign setup 4 (`#12`, `de19a9c`+`dfb6c25`) | `Vec<u64>` doc_len + postings-only candidate merge | Full/deces path p50 `~70 → 6.9 ms`; cumulative p50 to ~`6.9 ms` (now ~`10x` faster). |
| Campaign setup 5 (`#13`, `3bfec8f`+`2c59e91`) | Borrowed doc-len slices + one-pass min bound | p50 `6.9 → 2.0 ms`; ES p50 `4.9 ms`, so Surch is now `~2.45x` faster at p50 under concurrent probe. |
| Campaign setup 6 (`#14`, `2e7186b`+`97a0ca0`) | Reduced hashmap+`query_matches` overhead | Tail unaffected (`p95/p99` around `15/20.8 ms`), parity preserved; no headline delta. |
| Campaign setup 7 (`#15`, `f3ff8ca`) | Exact bool/function_score top-K path | Tail still `~15/20 ms` on bool/full; p50 remains `~1.9 ms`. |
| Campaign setup 8 (`#16`, `WORKERS=2`) | 2 workers / contention probe | Relative gap narrows under fair setting; deces bool/full still tail-heavy (`p95 ~10 ms`) while match path keeps lead (`<2 ms`). |

*Readout provenance:* baseline (`26609427689`), cleanup (`#1`, `26616206949`), `#10` (`26651526846`),
`#11` (`26668292578`), `#12` (`26696446460`), `#13` (`26697199003`), and `#16` probe (`sha-f3ff8ca`, same probe family).

### All-benchmarks status by the same four axes

| Axis | vs OpenSearch 2.17.1 | vs Elasticsearch 8.6.1 |
|------|----------------------|------------------------|
| **1. Bulk indexing** | **1.55x faster** on TREC-COVID and **6.7x faster** on SciFact. | **Parity / slight lead** on deces 1.36M: Surch `104.2 s` vs ES `115.9 s` median, with ES still owning the best individual run. |
| **2. Search latency** | **2.7-3.1x faster** on INSEE 10k; **1.83x slower** raw p50 on TREC-COVID cache-OFF. Cache-ON `~354x` is a hot LRU case, not a raw-engine claim. | **2.45x faster p50** on deces (`2.0 ms` vs `4.9 ms`), after the per-query setup-cost fixes; tail still trails (`p95/p99` Surch `14/21 ms` vs ES `11/15 ms`). |
| **3. Memory (RSS)** | **0.62x, Surch lighter** on TREC-COVID post-#9 (`907 MiB` vs `1465 MiB`). | 28M-scale RSS pending. |
| **4. Quality / parity** | **Parity** across 4 BEIR datasets. | **Bit-exact parity** on B1 `30/30` and B2 `8/8`, `0` divergence. |

**Won:** bulk (both engines), small-corpus latency vs OpenSearch,
median deces latency vs Elasticsearch, quality/parity, and current
TREC-COVID memory vs OpenSearch. **Open fronts:** raw-engine
large-corpus search p50 vs OpenSearch, deces p95/p99 tail vs
Elasticsearch, and the 28M full-corpus memory/indexation run. The
deces p50 gap was collapsed from the pre-campaign multi-second path to
`2.0 ms` via per-query setup fixes (should-intersection,
`function_score` unwrap, dense `u32` candidate intersection, dense
`doc_len`, zero-copy borrowed doc lengths, incremental `min_doc_len`,
and single-token candidate fast path). Four later hypotheses failed to
move the deces tail; the remaining lever is structural compact/SIMD
postings work, not another local scoring micro-tweak.

## 11. Conclusion

A pure-Rust OpenSearch-compatible engine can match and beat a mature
JVM engine on bulk indexing, small-corpus latency, and memory
simultaneously, with no quality regression, given a few targeted algorithmic
fixes and an allocator at parity. The honest counter-finding — large-corpus
raw-engine search latency (cache disabled) still trails OpenSearch — is
reported as plainly as the wins, and is the active front of the
beat-Elasticsearch campaign (§10). The measurement programme — versioned
schemas, sibling-container fairness, quality guardrails, multi-rep medians,
cache-on/off isolation — is itself reproducible from the promoted CI artefacts.
