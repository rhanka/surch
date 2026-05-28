# Objective F — F3: isolated top-K shortcut (bounded heap + lazy hydration) — large corpus

Third F3 isolation result (after WAND `2026-05-26-F3-wand-isolation-trec-covid-K8s`
and the LRU cache `2026-05-26-F3-lru-cache-isolation-trec-covid-K8s`), same
measurement-toggle-on-throwaway-branch method. The `perf-isolation` branch
(NEVER merged to main) adds `SURCH_DISABLE_TOPK` (read once via `OnceLock`,
default off → production path on main unchanged). When set, `run_topk_search`
bails immediately and the query falls back to the legacy full-scan `run_search`
path, which scores every candidate and **clones every matching document's
`_source`** before paginating. This isolates the top-K shortcut — a bounded
top-K heap plus **lazy `_source` hydration of only the K winners** — against the
published all-on median.

- top-K OFF run: GHA `26488095288` @ `perf-isolation` `b621c97`
  (`SURCH_DISABLE_TOPK=1`, MaxScore + LRU cache both ON).
- All-on baseline: 3-rep median, `2026-05-25-F4-trec-covid-latency-3rep-K8s`.
- Same harness, same 171k TREC-COVID corpus, same 13 170 queries, 0 errors.

## Result — the top-K shortcut is the single largest tail optimisation

| trec-covid 171k (Surch) | p50 | p95 | p99 | max |
|-------------------------|----:|----:|----:|----:|
| top-K **ON** (median) | 0.5 ms | 1.3 ms | 5.3 ms | 308 ms |
| top-K **OFF** (full-scan + clone-all `_source`) | 0.6 ms | 1.8 ms | **4093 ms** | **15470 ms** |
| shortcut contribution | ~flat | ~flat | **−770x** | **−50x** |

The per-phase breakdown shows where it bites — the **cold** queries (first
occurrence of each of the 50 distinct queries, before the LRU warms):

| phase (rps) | top-K OFF Surch p50 | p95 | max |
|-------------|--------------------:|----:|----:|
| 1 (2 rps, cold) | **7336.8 ms** | 13530 ms | 15470 ms |
| 2 (2 rps) | 4093.1 ms | 10386 ms | 11931 ms |
| 3 (5 rps) | 41.0 ms | 5547 ms | 12000 ms |
| 6 (50 rps, warm) | 0.6 ms | 1.3 ms | 11.6 ms |

Without the shortcut, a cold high-frequency-term query (matching a large
fraction of the 171k corpus) forces the full-scan path to clone hundreds of
thousands of `_source` values — **multi-second per query** (cold p50 `7.3 s`,
worst `15.5 s`). Once the LRU warms (phases 5–6) the queries are cache hits
again (`0.6 ms`), so the global p50/p95 stay flat and only the cold-miss tail
(p99/max) exposes the cost — the same cache-masking pattern as the WAND and
LRU isolations.

## Worse than OpenSearch without the shortcut

With top-K disabled, Surch's cold full-scan path is **far slower than
OpenSearch** on the same run (OpenSearch cold phase-1 p50 `43.3 ms`, global p50
`191.6 ms`, max `1725.6 ms`). So the bounded-heap + lazy-hydration shortcut is
not a marginal optimisation — it is what makes Surch's large-corpus tail
competitive at all. It is the directly-measured complement to the F4 headline:
the top-K shortcut (lazy hydration) and the LRU cache together produce the
sub-millisecond steady state; WAND caps the cold-miss tail; without any one of
them the engine degrades sharply on the cold path.

## Retrieval equivalence held

Pre-artillery hits probe (`surch.bench.trec_hits.v1`): Surch matched
`7 507 757` docs vs OpenSearch `7 510 550` (ratio `0.9996`), 50/50 queries
non-zero — identical retrieval work to the baseline, so the latency
decomposition is like-for-like.

## Method note

The top-K-OFF Job exits non-zero (Surch `max = 15.5 s` blows past the artillery
latency SLO calibrated for the top-K-on engine) — the failure IS the signal.
Latency is read from the run's artillery report regardless of the gate verdict
(0 errors across 13 170 requests on both engines). The `SURCH_DISABLE_TOPK`
toggle and the `trec-covid-latency` env that sets it live **only on
`perf-isolation`** and are never merged to `main`.

## Sources

- top-K OFF: GHA `26488095288` (ci-k8s `trec-covid-latency`, `perf-isolation`
  `b621c97`, image `sha-b621c97…`).
- All-on median: `2026-05-25-F4-trec-covid-latency-3rep-K8s/`.
- Toggle: `perf-isolation` branch, `crates/surch-api/src/search.rs`
  (`topk_shortcut_enabled`), `deploy/k8s/jobs/trec-covid-latency.yaml`.
