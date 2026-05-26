# Objective F — F4: 3-rep large-corpus search latency (TREC-COVID 171k)

Multi-repetition of the `trec-covid-latency` K8s harness — the
large-corpus search-latency axis (the regime INSEE 10k could not
exercise). Promotes the single-run F4 landing
(`2026-05-25-F4-trec-covid-latency-K8s/`) to a median + range verdict,
addressing that report's single-run caveat. Surch and OpenSearch
2.17.1 run as sibling containers in one Pod; both index the full 171k
TREC-COVID corpus, then `artillery_bench` replays the real TREC-COVID
test queries (`multi_match` over `title`/`text`) over 6 RPS phases
(13 170 queries, 8 workers).

- 3 successful repetitions, all 5 SLO checks PASS each. GHA runs
  `26422565840` (rep1, @ `9f53ba2`), `26423474877` (rep2),
  `26424070888` (rep3) — reps 2–3 @ `b614d60`. The Surch binary and
  the Job are identical across all three (only docs changed between
  the SHAs).
- 0 errors, both engines, all reps.

## Surch search latency (global, 0 errors)

| Metric | rep1 | rep2 | rep3 | median | min–max |
|--------|-----:|-----:|-----:|-------:|--------:|
| p50 | 0.5 ms | 0.5 ms | 0.5 ms | **0.5 ms** | 0.5–0.5 ms |
| p95 | 1.3 ms | 1.3 ms | 1.3 ms | **1.3 ms** | 1.3–1.3 ms |
| p99 | 6.0 ms | 5.0 ms | 5.3 ms | **5.3 ms** | 5.0–6.0 ms |
| max | 395.1 ms | 301.9 ms | 308.4 ms | **308.4 ms** | 301.9–395.1 ms |

Surch p50/p95 are **zero-variance** across the 3 reps (`0.5 / 1.3 ms`);
only the tail (p99/max) shows the expected cold-start noise.

## OpenSearch 2.17.1 (same runs, 0 errors)

| Metric | rep1 | rep2 | rep3 | median |
|--------|-----:|-----:|-----:|-------:|
| p50 | 183.8 ms | 174.1 ms | 176.9 ms | **176.9 ms** |
| p95 | 487.8 ms | 468.6 ms | 481.4 ms | **481.4 ms** |
| p99 | 677.7 ms | 604.3 ms | 673.1 ms | **673.1 ms** |
| max | 1389.1 ms | 1223.0 ms | 1282.4 ms | **1282.4 ms** |

## Verdict (medians)

| Metric | Surch | OpenSearch | Surch advantage |
|--------|------:|-----------:|----------------:|
| p50 | 0.5 ms | 176.9 ms | **~354x faster** |
| p95 | 1.3 ms | 481.4 ms | **~370x faster** |
| p99 | 5.3 ms | 673.1 ms | **~127x faster** |
| max | 308.4 ms | 1282.4 ms | **~4.2x faster** |
| errors | 0 / 13170 | 0 / 13170 | parity |

On the full 171k corpus with real multi-term queries, Surch is two-to-
three orders of magnitude faster than OpenSearch at the typical and
percentile latencies, reproducibly across 3 reps. OpenSearch also
*degrades* under load within each run (p50 climbs toward ~190 ms at
50 RPS) while Surch stays flat — see the per-phase tables in the
single-run report.

## Memory (RSS peak, sampled)

| Engine | rep1 | rep2 | rep3 | median |
|--------|-----:|-----:|-----:|-------:|
| Surch | 2135 MB | 2123 MB | 2104 MB | **2123 MB** |
| OpenSearch | 1415 MB | 1411 MB | 1421 MB | **1415 MB** |

Surch RSS peak on the 171k corpus is `~2123 MB` median (range
2104–2135, `±0.7%`), `~1.5x` the OpenSearch JVM peak — the expected
footprint at this corpus size (matches the F2 ndcg-gate `~2168 MB`),
well within the `2560 MB` job budget.

## Work-equivalence probe (in-artifact)

A subsequent run (`26424807778`, same Job + an untimed hits-equivalence
probe before the artillery phases — `surch.bench.trec_hits.v1`) issues
each of the 50 TREC queries once to both engines and records
`hits.total`:

| Metric | Value |
|--------|------:|
| queries | 50 |
| both engines non-empty | **50 / 50** |
| Surch empty result sets | 0 |
| OpenSearch empty result sets | 0 |
| Surch total matched docs | 7 507 757 |
| OpenSearch total matched docs | 7 510 550 |
| Surch / OpenSearch matched-doc ratio | **0.9996** (`-0.04 %`) |

Every query returns a non-empty set on **both** engines, and the total
matched-document volume agrees to within `0.04 %`. Combined with the
NDCG@10 parity (Surch `0.4750` vs OpenSearch `0.4902`,
`2026-05-25-F2-ndcg-3rep-K8s/`), this is direct in-artifact evidence
that Surch does the *same* retrieval work as OpenSearch — the ~360x
latency advantage is real work done faster (WAND/MaxScore + skip-list
early termination on long posting lists, no JVM overhead), not a
degenerate near-empty Surch result set. The probe is untimed (runs
before the phases) and that run's latency is consistent with the
3-rep medians (Surch p50 `0.5 ms` / p95 `1.2 ms`).

## Sources

- GHA runs `26422565840`, `26423474877`, `26424070888` (ci-k8s
  `trec-covid-latency`).
- Single-run landing + per-phase tables + SLO-fix story:
  `2026-05-25-F4-trec-covid-latency-K8s/`.
- NDCG / RSS cross-check: `2026-05-25-F2-ndcg-3rep-K8s/`.
