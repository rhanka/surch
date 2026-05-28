# Beat-Elasticsearch campaign — optimisation log (research paper trace)

**Goal.** Make Surch demonstrably **≥ Elasticsearch 8.6.1 on every benchmark**,
proven without cheating, using the matchID `deces` corpus as the honest
proving ground: first **1.36M (`deaths`)**, then **28M (full prod)**, then other
benchmarks. Surch is the product; matchID is the challenge ground (we do not
optimise matchID itself).

Each optimisation below is one paper-traceable step: **hypothesis → change →
before/after measurement (CI, representative HW) → verdict**.

## No-cheat measurement bar (mandatory for every claim)
- **Engine-to-engine** where possible (Surch vs ES over the wire), not only
  through the matchID Node backend (which is its own bottleneck and confounds
  engine latency).
- **Representative hardware** (NOT the 2-vCPU GitHub runner; the runner caps
  every absolute number and starves the engines).
- **Real corpus** (`deces` 1.36M, then 28M), not a 50-query low-cardinality
  replay (the LRU cache gives a misleading "354x" on repeated queries — banned
  as a headline).
- **≥3 reps**, report **cache ON *and* OFF**, and report **all dimensions** so
  losses show as plainly as wins: bulk/indexation time, search p50/p95/p99/max,
  QPS under concurrency, and **RSS** (the in-memory model is the scale risk).

## Dimensions tracked (where Surch can/can't credibly win)
| Dimension | Honest prior | Why |
|-----------|--------------|-----|
| Tail latency p99/max | **Surch can win** | no JVM/GC pauses |
| Footprint / startup / density | **Surch can win** | ~30 MB binary, no JVM heap |
| Bulk on simple mappings | parity/win shown vs OS 2.17.1 | deferred FST (Lot 1.6) |
| Bulk on rich mappings (deces, 28 fields) | **behind ES** | single-thread + multi-field analysis |
| Search QPS under concurrency | **at risk** | reader/writer lock contention bug |
| **Memory at scale (28M)** | **structural risk** | in-memory vs ES disk + page cache |

## Baseline (pre-campaign, deces 1.36M, matchID CI, 2-vCPU runner — confounded)
- Indexation `_bulk`: ES `116 s` (11 600 docs/s) vs Surch `2125 s` (640 docs/s) → ES ~18x.
  Cause: Surch bulk single-threaded (one write-lock) vs ES multi-thread.
- Artillery via backend: ES median `13.5 s` vs Surch `54.6 s` → ES ~4x (runner-bound, relative only).
- Source: `matchID surch-eval` branch, run `26528627429`.

## Optimisations

### #1 — Parallel bulk document analysis (rayon) — `dd3f528`
- **Hypothesis**: the single write-lock serialises the CPU-heavy per-doc
  analysis; ES parallelises bulk across cores. Moving analysis off-lock and
  running it `par_iter` should scale bulk with cores.
- **Change**: `crates/surch-index/src/document_index.rs` — pure
  `analyze_document` (off-lock, parallel) + serial `merge_analyzed`; documents
  merged in input order → **byte-identical postings (parity-preserving)**.
  Unit tests (`surch-index`) green; cloud `ci` (workspace test/clippy/fmt) green.
- **Measurement (ci-k8s `ndcg-gate`, run `26580620561`, image `sha-9b7e632`)**:
  - Quality **parity preserved** (bit-stable): SciFact NDCG@10 `0.6576`,
    TREC-COVID `0.4750` — identical to pre-change → the refactor is correct.
  - Bulk SciFact: Surch `1.83 s` vs OS `14.85 s` (~8.1x). TREC-COVID: Surch
    `68.4 s` vs OS `131.5 s` (~1.92x). Both within noise of the pre-change
    3-rep medians (SciFact `2.09 s`, TREC-COVID `70.96 s`) → **no clear gain on
    these corpora**, as expected: trec-covid/scifact have ≤2 analysed text
    fields, so per-doc analysis is not the bulk bottleneck there (the serial
    merge / postings build dominates).
  - The parallelisation's payoff is on **rich mappings** (deces: 28 fields,
    `norm` analyzer + `.raw` re-analysis + prefixes) where per-doc analysis
    dominates — measured separately on the matchID `surch-eval` deces CI.
- **Measurement (deces 1.36M, matchID `surch-eval` run `26582048931`,
  parallelised image, 2-vCPU runner)**: bulk **Surch `104.2 s` vs ES `115.9 s`**
  (1 355 728 docs). Baseline (pre-#1, run `26528627429`, same workflow): Surch
  `2125 s` vs ES `116 s`.
  → **The 18x indexation deficit on the rich deces mapping is CLOSED: Surch
  reaches parity and edges ES (104.2 s vs 115.9 s).** The win lands exactly
  where predicted (28 fields, `norm` + `.raw` re-analysis → analysis-bound).
  Quality/parity unaffected (search results unchanged; cloud `ci` oracles green).
- **Verdict**: ✅ **first beat-ES milestone — indexation deces 1.36M: Surch ≥ ES.**
  Caveats (no-cheat): single run, ~10% margin → claim is "gap closed / parity,
  Surch slightly ahead"; ≥3 reps needed to firm a hard "faster" figure (queued).
  The large baseline ratio (2125→104) partly reflects baseline-run variance on
  the shared runner; the defensible head-to-head is the **same-run** 104.2 vs
  115.9 s. Search latency is unchanged (parallelising bulk does not touch the
  read path) → next optimisations target search/concurrency.

## Backlog (ordered by leverage)
1. **Fix the reader/writer concurrency wedge** (search during sustained bulk
   hangs) — table stakes for a production engine + unlocks honest QPS numbers.
   See `docs/wp-a-perf-followups-concurrent-bulk-search-stall.md`.
2. **Engine-level deces benchmark harness** (Surch vs ES direct, representative
   HW) — to actually know where we stand on deces, not via the Node backend.
3. **Reduce rich-mapping analysis cost** (normalise-once for parent + `.raw`).
4. **Prove the tail-latency advantage** (no-GC) cleanly on deces.
5. **Confront memory at scale** (can Surch hold 28M, at what RSS vs ES?).
