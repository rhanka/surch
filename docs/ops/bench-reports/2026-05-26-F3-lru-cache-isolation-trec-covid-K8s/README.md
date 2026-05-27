# Objective F — F3: isolated search-result cache (LRU) contribution (large corpus)

Second F3 isolation result, via the **measurement-toggle on an isolated
branch** strategy (same approach as the WAND isolation,
`2026-05-26-F3-wand-isolation-trec-covid-K8s`). The throwaway
`perf-isolation` branch (NEVER merged to main) adds `SURCH_DISABLE_SEARCH_CACHE`
(read once via `OnceLock`, default off → production path on main unchanged).
Running the `trec-covid-latency` harness with the toggle ON disables the
per-query search-result LRU entirely, so **every** artillery request recomputes
from the postings — isolating the cache's contribution against the published
cache-on median.

- Cache OFF run: GHA `26482991052` @ `perf-isolation` `54f06b7`
  (`SURCH_DISABLE_SEARCH_CACHE=1`, MaxScore still ON).
- Cache ON baseline: 3-rep median, `2026-05-25-F4-trec-covid-latency-3rep-K8s`.
- Same harness, same 171k TREC-COVID corpus, same 13 170 queries, 0 errors.

## Result — the LRU result cache carries the bulk of the headline advantage

| trec-covid 171k (Surch) | p50 | p95 | p99 | max |
|-------------------------|----:|----:|----:|----:|
| Cache **ON** (median) | 0.5 ms | 1.3 ms | 5.3 ms | 308 ms |
| Cache **OFF** (recompute every query) | 309.1 ms | 532.3 ms | 623.8 ms | 913.8 ms |
| Cache contribution | **−618x** | **−409x** | **−118x** | **−3.0x** |

With the result cache disabled, Surch's median query latency goes from
`0.5 ms` to `309 ms` — i.e. the cache is responsible for **essentially all**
of the sub-millisecond steady-state latency, and therefore for the bulk of the
~354x advantage over OpenSearch reported in the F4 latency study.

## Honest caveat — this is partly a benchmark-cardinality artifact

The artillery workload replays a **fixed set of 50 distinct queries** 13 170
times. After warmup the LRU hit rate is ~99.6%, so cache-on latency is
dominated by cache *hits* (`0.5 ms`), not by retrieval work. A production
workload with high query cardinality (low repeat rate) would see a far lower
hit rate and an effective latency much closer to the cache-OFF numbers. The
`0.5 ms` headline is the **best case** (hot, low-cardinality), not the typical
case.

## Raw engine vs OpenSearch (cache OFF, same run, same conditions)

With the cache out of the way, the cache-OFF numbers are the closest available
estimate of Surch's **raw per-query compute** (MaxScore still on). Against the
OpenSearch arm of the *same* run (which has no comparable result cache active
on this workload):

| trec-covid 171k (global) | p50 | p95 | p99 | max |
|--------------------------|----:|----:|----:|----:|
| Surch (cache OFF) | 309.1 ms | 532.3 ms | 623.8 ms | 913.8 ms |
| OpenSearch | 169.2 ms | 414.6 ms | 597.5 ms | 1094.3 ms |
| Surch vs OS | **1.83x slower** | 1.28x slower | ~parity | **1.20x faster** |

So **with the result cache disabled, Surch's core retrieval engine is roughly
on par with OpenSearch and somewhat slower at the median** (p50 ~1.8x) on this
171k workload — it is only *faster* at the extreme tail (`max`). Surch's
decisive advantage in the published F4 study comes from the result cache, not
from a faster core scorer. This is the directly-measured, honest decomposition
of the headline number.

## Retrieval equivalence held (cache-OFF run)

The pre-artillery hits probe (`surch.bench.trec_hits.v1`) on this run:
Surch matched `7 507 757` docs vs OpenSearch `7 510 550` (ratio `0.9996`,
`-0.04 %`), 50/50 queries non-zero on both engines — same retrieval work as
the cache-on baseline, so the latency decomposition is like-for-like.

## Method note

The cache-OFF Job exits non-zero because Surch `max = 913.8 ms` (and the
elevated p50/p95) exceed the artillery latency SLO calibrated for the
cache-on engine — that failure IS the signal. The latency numbers are read
from the run's artillery report regardless of the gate verdict (0 errors
across all 13 170 requests on both engines).

The `SURCH_DISABLE_SEARCH_CACHE` toggle and the `trec-covid-latency` env that
sets it live **only on the `perf-isolation` branch** and are never merged to
`main` — production carries no measurement flag (the no-flags-in-prod
constraint).

## Sources

- Cache-OFF: GHA `26482991052` (ci-k8s `trec-covid-latency`,
  `perf-isolation` `54f06b7`, image `sha-54f06b7…`).
- Cache-ON median: `2026-05-25-F4-trec-covid-latency-3rep-K8s/`.
- Toggle: `perf-isolation` branch, `crates/surch-api/src/state.rs`
  (`search_cache_enabled()`), `deploy/k8s/jobs/trec-covid-latency.yaml`.
