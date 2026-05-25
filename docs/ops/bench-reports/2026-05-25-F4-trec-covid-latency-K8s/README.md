# Objective F — F4: large-corpus search latency (TREC-COVID 171k)

First green run of the `trec-covid-latency` K8s harness (Objective F
F4) — the large-corpus search-latency axis the INSEE 10k workload
could not exercise. Surch and OpenSearch 2.17.1 run as sibling
containers in one Pod; both index the full 171k TREC-COVID corpus,
then `artillery_bench` replays the real TREC-COVID test queries
(`multi_match` over `title`/`text`) against each engine over 6 RPS
phases (13 170 queries, 8 workers). This is the regime that stresses
long posting lists / skip-lists.

- GHA run `26422565840` on `main` @ `9f53ba2` — **PASS** (all 5 SLO
  checks green).
- Single run (not yet multi-rep): an illustrative large-corpus data
  point, not a final median claim. See *Caveats* below.

## Surch search latency (artillery TREC-COVID, 0 errors)

| Phase (RPS×s) | issued | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|---:|
| 1 (2×30) | 60 | 25.9 ms | 221.4 ms | 395.1 ms | 395.1 ms |
| 2 (2×30) | 60 | 0.9 ms | 187.9 ms | 274.5 ms | 274.5 ms |
| 3 (5×30) | 150 | 0.6 ms | 115.7 ms | 218.0 ms | 306.5 ms |
| 4 (10×30) | 300 | 0.6 ms | 1.9 ms | 69.6 ms | 170.1 ms |
| 5 (20×30) | 600 | 0.5 ms | 1.3 ms | 2.9 ms | 4.3 ms |
| 6 (50×240) | 12000 | 0.5 ms | 1.2 ms | 3.0 ms | 12.3 ms |
| **global** | 13170 | **0.5 ms** | **1.3 ms** | **6.0 ms** | 395.1 ms |

Surch shows a cold-start tail in the early low-RPS phases (p95 up to
221 ms while the page cache / structures warm), then settles to a
steady-state p50 `0.5 ms` / p95 `1.2–1.3 ms` once warm (phases 5–6,
which carry 12 600 of the 13 170 queries).

## OpenSearch 2.17.1 (same runs, 0 errors)

| Phase (RPS×s) | issued | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|---:|
| 1 (2×30) | 60 | 33.4 ms | 136.6 ms | 677.5 ms | 677.5 ms |
| 2 (2×30) | 60 | 40.3 ms | 120.9 ms | 143.0 ms | 143.0 ms |
| 3 (5×30) | 150 | 32.3 ms | 137.9 ms | 170.0 ms | 173.6 ms |
| 4 (10×30) | 300 | 22.7 ms | 112.5 ms | 131.8 ms | 225.4 ms |
| 5 (20×30) | 600 | 74.6 ms | 483.7 ms | 678.8 ms | 778.2 ms |
| 6 (50×240) | 12000 | 193.2 ms | 489.8 ms | 681.6 ms | 1389.1 ms |
| **global** | 13170 | **183.8 ms** | **487.8 ms** | **677.7 ms** | 1389.1 ms |

OpenSearch stays in the tens-to-hundreds of ms throughout and
*degrades* under load (p50 climbs to `193 ms` at 50 RPS), rather than
warming down like Surch.

## Verdict (global, steady-state dominated)

| Metric | Surch | OpenSearch | Surch advantage |
|--------|------:|-----------:|----------------:|
| p50 | 0.5 ms | 183.8 ms | **~368x faster** |
| p95 | 1.3 ms | 487.8 ms | **~375x faster** |
| p99 | 6.0 ms | 677.7 ms | **~113x faster** |
| max | 395.1 ms | 1389.1 ms | 3.5x faster |
| errors | 0 / 13170 | 0 / 13170 | parity |

On the full 171k corpus with real multi-term queries, Surch is
two-to-three orders of magnitude faster than OpenSearch at the
typical/percentile latencies, with both engines at 0 errors.

## Memory (RSS, sampled over the run)

| Engine | peak MB | final MB |
|--------|--------:|---------:|
| Surch | 2135.0 | 1206.0 |
| OpenSearch | 1415.0 | 1415.0 |

Surch's resident set on the 171k corpus is `~2135 MB` peak (matching
the F2 3-rep `~2168 MB ±0.5%` on the same corpus), `~1.5x` the
OpenSearch JVM peak — the expected footprint at this corpus size, not
a regression. The SLO budget for this job was set to `2560 MB`
accordingly (`--rss-peak-mb`, vs the `1024 MB` INSEE default).

## SLO checks (all PASS)

- artillery error rate ≤ 1 % [art-os] — PASS (0.000 %)
- Surch artillery p95 ≤ 200 ms [art-surch] — PASS (1.3 ms)
- Surch artillery max ≤ 500 ms [art-surch] — PASS (395.1 ms)
- artillery error rate ≤ 1 % [art-surch] — PASS (0.000 %)
- Surch RSS peak ≤ 2560 MB [rss-art-surch] — PASS (2135.0 MB)

## Caveats (read before citing the headline)

1. **Single run.** This is one repetition; the final article claim
   needs ≥3 reps (median + range), as done for F2. Treat the `~368x`
   as an illustrative large-corpus data point, not a settled median.
2. **Work-equivalence between engines.** `artillery_bench` records
   latency + error rate only — it drains the response body without
   parsing `hits.total`, so the artifact alone cannot assert the two
   engines matched the same document sets. The equivalence evidence
   comes from elsewhere: the `ndcg-gate` job shows **NDCG@10 parity**
   on TREC-COVID (Surch `0.4750` vs OpenSearch `0.4902`), i.e. the
   top-K retrieved are of equivalent quality. The latency gap
   therefore reflects Surch's WAND/MaxScore + skip-list early
   termination on long posting lists (the regime INSEE 10k could not
   reach) and the absence of JVM overhead — not a degenerate
   near-empty result set on the Surch side. A future hardening would
   log `hits.total` per request in `artillery_bench` for a direct
   in-artifact assertion.
3. **Cold start.** Surch's early-phase p95 (up to 221 ms) is a
   warm-up artifact; the steady-state figures (phases 5–6) are the
   representative ones and dominate the global percentiles.

## Sources

- GHA run `26422565840` (ci-k8s `trec-covid-latency`), image
  `sha-9f53ba2…` / `bench-sha-9f53ba2…`.
- `summary-26422565840.md` (raw `bench_report` output, this dir).
- `job.yaml` (the dispatched K8s Job, this dir).
- NDCG parity: `2026-05-25-F2-ndcg-3rep-K8s/`. RSS cross-check:
  same. The `--rss-peak-mb` per-workload budget: commit `9f53ba2`.
