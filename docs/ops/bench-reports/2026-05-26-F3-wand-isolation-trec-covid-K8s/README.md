# Objective F — F3: isolated WAND / MaxScore contribution (large corpus)

First F3 isolation result, via the **measurement-toggle on an isolated
branch** strategy (the historical-SHA replay was blocked — the old code
lacks the modern bench binaries; see `plan/wp-f-perf-paper.md`). A throwaway
`perf-isolation` branch (NEVER merged to main) adds `SURCH_DISABLE_MAXSCORE`
(read once, default off → production path on main unchanged). Running the
`trec-covid-latency` harness with the toggle ON forces the exhaustive scorer,
isolating the Block-Max WAND / MaxScore family's contribution against the
published MaxScore-on median.

- MaxScore OFF run: GHA `26481737037` @ `perf-isolation` `6e1846d`
  (`SURCH_DISABLE_MAXSCORE=1`).
- MaxScore ON baseline: 3-rep median, `2026-05-25-F4-trec-covid-latency-3rep-K8s`.
- Same harness, same 171k TREC-COVID corpus, same 13 170 queries, 0 errors.

## Result — WAND/MaxScore is a large-corpus TAIL optimisation

| trec-covid 171k (Surch) | p50 | p95 | p99 | max |
|-------------------------|----:|----:|----:|----:|
| MaxScore **ON** (median) | 0.5 ms | 1.3 ms | **5.3 ms** | **308 ms** |
| MaxScore **OFF** (no-WAND) | 0.6 ms | 1.4 ms | **51.4 ms** | **3915 ms** |
| WAND contribution | ~flat | ~flat | **−90 %** | **−92 %** |

- **p50 / p95 unchanged**: the median and typical-percentile queries are
  cheap with or without WAND — most TREC-COVID queries touch short enough
  posting lists that exhaustive scoring is already fast.
- **p99 / max collapse without WAND**: the worst-case queries (high-frequency
  terms → long posting lists) are **10x (p99) to 13x (max)** slower when the
  Block-Max WAND / MaxScore skip is disabled — `max` blows out from `308 ms`
  to `3.9 s`. WAND's value is precisely to cap the tail on long lists.

This is the directly-measured complement to the earlier finding that MaxScore
was **latency-neutral on INSEE 10k** (`2026-05-25-lot3-bmw-skiplist-K8s`):
short 10k posting lists give nothing to skip, but on the 171k corpus the same
optimisation cuts tail latency by an order of magnitude. The optimisation's
benefit regime — long posting lists / large corpora — is now quantified.

## Method note

The MaxScore-OFF Job exits non-zero because Surch `max = 3915 ms` exceeds the
artillery `max ≤ 500 ms` SLO — that SLO is calibrated for the MaxScore-on
engine, and the failure IS the signal (the tail blows out without WAND). The
latency numbers are read from the run's artillery report regardless of the
gate verdict.

The `SURCH_DISABLE_MAXSCORE` toggle and the `trec-covid-latency` env that
sets it live **only on the `perf-isolation` branch** and are never merged to
`main` — production carries no measurement flag (the no-flags-in-prod
constraint that scoped out the earlier in-place Lot 3 isolation).

## Sources

- MaxScore-OFF: GHA `26481737037` (ci-k8s `trec-covid-latency`,
  `perf-isolation` `6e1846d`, image `sha-6e1846d…`).
- MaxScore-ON median: `2026-05-25-F4-trec-covid-latency-3rep-K8s/`.
- Toggle: `perf-isolation` branch, `crates/surch-api/src/search.rs`
  (`maxscore_enabled()`), `deploy/k8s/jobs/trec-covid-latency.yaml`.
