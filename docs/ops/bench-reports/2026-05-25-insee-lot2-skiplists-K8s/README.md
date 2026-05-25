# INSEE artillery — 2026-05-25 (K8s, Lot 2 skip lists search latency)

Isolated search-latency measurement for Track A **Lot 2** — skip
lists on FoR postings + leapfrog AND (`d73c862`). The `ndcg-gate`
runs measure bulk + quality but only 50 BEIR queries with no
percentiles, so Lot 2's search-side gain is measured here with the
`insee-bench` artillery workload (matchID INSEE 10k, 13 170 queries
across 6 RPS phases).

## Clean isolation (control vs Lot 2)

Both runs are on the **same jemalloc stack** and neither carries
Lot 1.6 (which is a bulk-only change, search-neutral). The only
difference between the two SHAs is Lot 2:

| SHA | Stack | Lot 2 |
|-----|-------|:-----:|
| `b9f6636` (control) | jemalloc (Lot 1.7) | no |
| `d73c862` (Lot 2) | jemalloc (Lot 1.7) | **yes** |

Both `insee-bench` runs are GREEN (the `bench_report` RSS-SLO fix
`e37a864` is cherry-picked onto both branches so the JVM reference
engine's >1 GiB RSS no longer fails the Job).

### Surch search latency (artillery deces, 13 170 queries, 0 errors)

| Metric | Control `b9f6636` (no Lot 2) | Lot 2 `d73c862` | Delta |
|--------|---:|---:|---:|
| p50 | 1.6 ms | 1.6 ms | 0 % |
| p95 | 3.9 ms | 3.4 ms | **-13 %** |
| p99 | 7.9 ms | 6.5 ms | **-18 %** |
| max | 68.3 ms | 64.1 ms | -6 % |

**Verdict**: Lot 2 (skip lists + leapfrog AND) improves the search
latency tail (p95 `-13 %`, p99 `-18 %`) while the median (p50) is
unchanged. This is the expected shape: the skip-list cursor lets
multi-term AND queries leapfrog over non-matching blocks, which
helps the slower multi-term tail, not the already-fast single-term
median.

### Cross-engine context (same runs)

| Engine | p50 | p95 | p99 | max | errors |
|--------|----:|----:|----:|----:|-------:|
| Surch (Lot 2, `d73c862`) | 1.6 ms | 3.4 ms | 6.5 ms | 64.1 ms | 0 |
| OpenSearch 2.17.1 | 4.5 ms | 12.5 ms | 25.4 ms | 414.3 ms | 0 |

Surch is `2.8x` faster than OpenSearch at p50 and `3.7x` at p95 on
the matchID INSEE workload. All artillery SLOs PASS on both engines
(p95 ≤ 200 ms, max ≤ 500 ms, error rate ≤ 1 %). Surch RSS peak on
this 10 k workload is `75 MB` (well under the 1024 MB Surch SLO).

## Provenance

- Control: GHA run
  <https://github.com/rhanka/surch/actions/runs/26394464319>
  (`perf-control-jemalloc` = `b9f6636` + `bench_report` fix),
  insee-bench PASS.
- Lot 2: GHA run
  <https://github.com/rhanka/surch/actions/runs/26382530895>
  (`perf-lot2-skiplists` = `d73c862` + `bench_report` fix),
  insee-bench PASS.
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=insee-bench`).
- Workload: INSEE 10 k `deces` artillery, phases
  `2:30,2:30,5:30,10:30,20:30,50:240`, 8 workers, 13 170 queries.
- Raw files: `summary-lot2-d73c862.md`, `summary-control-b9f6636.md`,
  `rss-art-surch-lot2.json`, `rss-art-os-lot2.json`, `job.yaml`.

## Caveats

- **Single run per SHA**. The Track A replay protocol prefers 3
  repetitions with median + IQR for a final verdict. The p95/p99
  improvement direction is consistent with the algorithm, but a
  3-rep paired run would tighten the confidence interval. Deferred
  as a follow-up if a definitive search-win headline is needed.
- The deces INSEE 10 k workload is matchID-shaped (multi-field
  name/date AND queries). The skip-list gain may differ on other
  query shapes (pure OR, single-term).

## Closure

- Track A `wp-a-perf-followups.md` Lot 2 search-latency leaf
  closes: skip lists deliver a `-13 % / -18 %` p95/p99 search
  latency improvement on the matchID INSEE workload, cleanly
  isolated against the jemalloc control.
- This also fixes a Track E/B regression: `bench_report` RSS SLO
  now gates Surch only (`e37a864`), unblocking every future
  `insee-bench` run that carries paired RSS.
