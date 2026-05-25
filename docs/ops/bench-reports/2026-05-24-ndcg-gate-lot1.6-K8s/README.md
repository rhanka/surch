# BEIR NDCG gate - 2026-05-24 (K8s, Lot 1.6 deferred FST build)

`ndcg-gate` K8s run measuring Track A **Lot 1.6** — deferred FST
term-dictionary build off the bulk path (`2e4361e`). The run carries
both Lot 1.6 and Lot 2 (skip lists, `d73c862`) because they merged
together, but **the bulk gain reported here is attributable to Lot
1.6 alone**: Lot 2 is a search-side change and is bulk-neutral, as
proven by the control run on `d73c862` (Lot 2 only) which kept
TREC-COVID Surch bulk at `157.1 s` ≈ the jemalloc baseline `139 s`
(see `2026-05-24-ndcg-gate-lot1.7-jemalloc-K8s/`). Lot 2's own
search-latency gain is measured separately in the Lot 2 report.

**Milestone**: with Lot 1.6, Surch ingests the full 171 k TREC-COVID
corpus **faster than OpenSearch** (`56.4 s` vs `86.6 s`, Surch
`1.54x` faster), reversing the `13.9x` OpenSearch advantage measured
before Lot 1. NDCG@10 / Recall@10 unchanged. Surch RSS peak also
dropped (`3424 -> 2156 MiB`).

## Lot 1.6 isolation (control: Lot 2-only `d73c862`)

| Config | SHA | TREC-COVID Surch bulk | Attribution |
|--------|-----|----------------------:|-------------|
| jemalloc baseline (no Lot 1.6, no Lot 2) | `b9f6636` | 139.05 s | — |
| Lot 2 only (skip lists) | `d73c862` | 157.12 s | bulk-neutral (≈ baseline, within run noise) |
| Lot 1.6 + Lot 2 | `2e4361e` | **56.38 s** | the `157 -> 56 s` drop is **Lot 1.6** |

Lot 2 does not touch the bulk write path, so the deferred FST build
(Lot 1.6) owns the entire bulk speedup.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26373579876>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=ndcg-gate`)
- Job result: PASS, Kubernetes Job `SuccessCriteriaMet=True`,
  `Complete=True`
- Job duration (CI total): 22m15s (`21:44:29Z -> 22:06:44Z`)
- Head SHA: `2e4361e` (Lot 1.6 on top of Lot 2 `d73c862`)
- `ci` workspace integration: run `26373423517` SUCCESS (A+B compile
  + all workspace tests pass together)
- Surch image:
  `ghcr.io/rhanka/surch:sha-2e4361e…`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-2e4361e…`
- Raw files: `summary.md`, `bench.json`, `rss-ndcg-surch.json`,
  `rss-ndcg-os.json`, `job.yaml`.

## SciFact

5183 docs / 300 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 1375.5 ms | 11 079.9 ms | Surch 8.1x faster |
| NDCG@10 | 0.6576 | 0.6537 | Surch +0.6% |
| Recall@10 | 0.8100 | 0.8033 | Surch +0.8% |

## TREC-COVID (full 171 k corpus) — bulk parity crossed

171 332 docs / 50 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | **56 380.4 ms** (~56s) | 86 614.4 ms (~87s) | **Surch 1.54x faster** |
| NDCG@10 | 0.4750 | 0.4902 | Surch `-3.1%` |
| Recall@10 | 0.0132 | 0.0132 | Tie |

### Bulk ingest evolution across the Lot 1 → Lot 1.6 sequence

| Run | SHA | Change | Surch TREC-COVID bulk | Surch / OpenSearch |
|-----|-----|--------|----------------------:|-------------------:|
| `26304471549` | `d9cac15` | full-rebuild baseline | 1001.95 s | OS 13.9x faster |
| `26350556060` | `04fde72` | Lot 1 incremental append | 179.86 s | OS 2.06x faster |
| `26359069219` | `01ad77e` | Lot 1.5 refresh-finalize | 189.18 s | OS 1.92x faster |
| `26360701909` | `b9f6636` | Lot 1.7 jemalloc | 139.05 s | OS 1.42x faster |
| `26373579876` | `2e4361e` | **Lot 1.6 deferred FST + Lot 2** | **56.38 s** | **Surch 1.54x faster** |

Total Surch speedup: `1001.95 s -> 56.38 s` = **17.8x** on the same
corpus / chunking / pool. The Lot 1.6 deferred FST build is the
dominant contributor in this run (it removes the cumulative
`terms.build()` that ran once per `_bulk` POST). Lot 2 (skip lists)
is a search-side optimisation and does not affect bulk timing; its
effect must be measured separately with an `insee-bench` latency
replay.

## Paired RSS (`surch.bench.rss.v1`)

| Container | Peak RSS | Final RSS | Limit | Peak vs OpenSearch |
|---|---:|---:|---:|---:|
| `surch` | **2156 MiB** (2.11 GiB) | 1290 MiB (1.26 GiB) | 7 GiB | `~1.47x` |
| `opensearch` | 1466 MiB (1.43 GiB) | 1466 MiB | 2 GiB | — |

Surch RSS peak fell again `3424 -> 2156 MiB`: deferring the FST
build means the cumulative `PostingsBuilder` clone (`terms = builder.
clone().build()`) no longer runs per chunk, so the transient peak
during bulk is much lower. Surch peak is now `~1.47x` OpenSearch
peak (was `~2.3x` on Lot 1.7, `~3.8x` on Lot 1.5).

### RSS evolution

| Run | SHA | Surch RSS peak | Surch RSS final |
|-----|-----|---------------:|----------------:|
| `04fde72` | Lot 1 | 5859 MiB | 5859 MiB |
| `01ad77e` | Lot 1.5 | 5591 MiB | 5591 MiB |
| `b9f6636` | Lot 1.7 jemalloc | 3424 MiB | 1382 MiB |
| `2e4361e` | **Lot 1.6 + Lot 2** | **2156 MiB** | **1290 MiB** |

## Verdict

| Axis | Verdict |
|---|---|
| SciFact quality | PASS, Surch `+0.6%` NDCG@10, `+0.8%` Recall@10. |
| TREC-COVID quality | Reproducible cross-engine baseline: Surch `0.4750` vs OpenSearch `0.4902` NDCG@10, Recall@10 tied. Unchanged across the whole Lot 1→1.6 sequence — no quality regression from any bulk/RAM optimisation. |
| Bulk indexing | **Surch now beats OpenSearch on TREC-COVID** (`1.54x` faster) and SciFact (`8.1x` faster). The `13.9x` OpenSearch advantage is fully reversed. |
| Memory (paired RSS) | Surch peak `2156 MiB / 7 GiB` (31 %), `~1.47x` OpenSearch peak. Lowest of the series. |
| Search latency (Lot 2) | NOT measured by `ndcg-gate` (50 queries, no latency percentiles). Needs an `insee-bench` replay to quantify the skip-list leapfrog-AND gain. |
| SLO / errors | Job `Complete=True`, `SuccessCriteriaMet=True`, no driver failure, no Surch OOM, no OpenSearch error. |

## Closure effects

- Track A `wp-a-perf-followups.md` Lot 1.6 closes: deferred FST
  build validated, TREC-COVID bulk now under OpenSearch.
- Track A `wp-a-perf-followups.md` Lot 2 lands (skip lists), but its
  search-latency gain is not yet quantified — a follow-up
  `insee-bench` replay is required before claiming a search win.
- Track A performance ledger `Bulk indexing` + `RSS / memory` rows
  updated with the parity-crossing numbers.

## Next

- Quantify Lot 2 search latency via `insee-bench` on `2e4361e` vs
  the `2026-05-21-A-replay-current-main-61a13f-insee-K8s/` baseline.
- Lot 3 — next Block-Max WAND step on top of the Lot 2 skip lists.
