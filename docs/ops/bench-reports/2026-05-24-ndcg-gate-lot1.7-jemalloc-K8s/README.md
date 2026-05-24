# BEIR NDCG gate - 2026-05-24 (K8s, Surch 7 GiB, jemalloc)

First `ndcg-gate` K8s run after `b9f6636` (Track A
`wp-a-perf-followups.md` Lot 1.7 step B — switch the Surch global
allocator from glibc default to **jemalloc** via `tikv-jemallocator`
0.6, paired with
`MALLOC_CONF=background_thread:true,dirty_decay_ms:0,muzzy_decay_ms:0`
shipped in the runtime Dockerfile).

The fix targets the residual allocator inertia exposed by Lot 1.5:
`finalize_postings()` logically dropped the `PostingsBuilder`
snapshot but the glibc default heap kept ~700 MiB of freed pages
mapped without memory pressure.

**Three-way win**: Surch RSS peak `-2167 MiB` (`-39 %`), Surch RSS
final `-4209 MiB` (`-75 %`, post-refresh background-thread purge),
Surch TREC-COVID bulk `-26 %` (jemalloc faster malloc + reduced
contention on the bulk allocation path). NDCG@10 / Recall@10
unchanged.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26360701909>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=ndcg-gate`)
- Job result: PASS, Kubernetes Job `SuccessCriteriaMet=True`,
  `Complete=True`
- Job duration (CI total): 22m46s
  (`12:02:32Z -> 12:25:18Z`)
- Gate window inside the pod (first SciFact start to last
  TREC-COVID end): visible in `summary.md`
- Head SHA: `b9f6636be6303ff88b4f86cc1fd75f50441b2e72`
- Surch image:
  `ghcr.io/rhanka/surch:sha-b9f6636be6303ff88b4f86cc1fd75f50441b2e72`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-b9f6636be6303ff88b4f86cc1fd75f50441b2e72`
- Raw files in this directory: `summary.md`, `bench.json`,
  `rss-ndcg-surch.json`, `rss-ndcg-os.json`, `job.yaml`.

## Environment

Same pod shape as `2026-05-24-ndcg-gate-lot1.5-ram-K8s/`. The only
runtime change is `MALLOC_CONF` set in the surch container ENV via
the Dockerfile, and the binary now links jemalloc statically (~1 MB
binary increase).

| Container | CPU request | CPU limit | Mem request | Mem limit |
|---|---:|---:|---:|---:|
| `ndcg-driver` | 100m | 500m | 128Mi | 1Gi |
| `surch` (init, restartPolicy=Always) | 150m | 2000m | 256Mi | **7Gi** |
| `opensearch` (init, restartPolicy=Always) | 250m | 1200m | 256Mi | 2Gi |

OpenSearch already uses jemalloc by default on Linux since
~7.13 — Lot 1.7 levels the allocator playing field between Surch
and the reference engine.

## SciFact

5183 docs / 300 unique test queries.

| Metric | Surch (jemalloc) | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 1697.8 ms | 13 443.1 ms | Surch 7.9x faster |
| NDCG@10 | 0.6576 | 0.6537 | Surch +0.6% |
| Recall@10 | 0.8100 | 0.8033 | Surch +0.8% |

## TREC-COVID (full 171 k corpus)

171 332 docs / 50 unique test queries.

| Metric | Surch (jemalloc) | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 139 048.0 ms (~2m19s) | 97 830.6 ms (~1m38s) | OpenSearch 1.42x faster |
| NDCG@10 | 0.4750 | 0.4902 | Surch `-3.1%` |
| Recall@10 | 0.0132 | 0.0132 | Tie |

## Paired RSS — jemalloc effect

Sampled at 1 Hz for `RSS_SAMPLE_SECONDS=1200` (20 minutes) via
`/proc/<pid>/status:VmRSS`.

| Container | Peak RSS | Final RSS | Limit | Cap usage (peak) |
|---|---:|---:|---:|---:|
| `surch` (jemalloc) | **3424 MiB** (3.34 GiB) | **1382 MiB** (1.35 GiB) | 7 GiB | 49 % |
| `opensearch` | 1462 MiB (1.43 GiB) | 1462 MiB | 2 GiB | 73 % |

`peak_kb` vs `final_kb` divergence is the key signal: jemalloc's
background thread (`background_thread:true`) actively returns
freed dirty / muzzy pages to the OS once the bulk burst is over,
so the post-refresh steady state is far below the bulk peak.
With the glibc default (Lot 1.5), `peak == final` because pages
were never returned without memory pressure.

### Comparison vs all previous `ndcg-gate` runs on the same 7 GiB pool

| Run | SHA | Path | Surch RSS peak | Surch RSS final | TREC-COVID Surch bulk |
|-----|-----|------|---------------:|----------------:|----------------------:|
| `26304471549` | `d9cac15` | full-rebuild baseline | 4802 MiB* | 4802 MiB* | 1001.95 s |
| `26340177506` | `137b352` | full-rebuild + paired RSS | 4802 MiB | 4802 MiB | 1112.52 s |
| `26350556060` | `04fde72` | Lot 1 incremental bulk | 5859 MiB | 5859 MiB | 179.86 s |
| `26359069219` | `01ad77e` | Lot 1 + Lot 1.5 RAM | 5591 MiB | 5591 MiB | 189.18 s |
| `26360701909` | `b9f6636` | **+ Lot 1.7 jemalloc** | **3424 MiB** | **1382 MiB** | **139.05 s** |

*RSS via `kubectl top` only.

## Verdict

| Axis | Verdict |
|---|---|
| SciFact quality | PASS, Surch `+0.6%` NDCG@10, `+0.8%` Recall@10 vs OpenSearch. |
| TREC-COVID quality | Reproducible cross-engine baseline: Surch `0.4750` vs OpenSearch `0.4902` NDCG@10, Recall@10 tied at `0.0132`. |
| Bulk indexing | Surch SciFact `7.9x` faster (vs `5.4x` on Lot 1.5). TREC-COVID Surch/OS gap `1.92x -> 1.42x` (jemalloc speeds up the allocation hot path too, not just RAM). |
| Memory (paired RSS) | Surch peak `3424 MiB / 7 GiB` (49 %), down from `5591 MiB`. Surch final `1382 MiB`, down from `5591 MiB` — the background purge thread actively returns pages after the bulk burst. Surch full-corpus footprint is now `~2.3x` OpenSearch peak (vs `~3.8x` on Lot 1.5). |
| SLO / errors | Job `Complete=True`, `SuccessCriteriaMet=True`, no driver failure, no Surch OOM, no OpenSearch error. |

## Closure effects

- Track A `wp-a-perf-followups.md` Lot 1.7 closes (step B chosen
  and validated). The Lot 1.5 allocator caveat is fully resolved
  — actually exceeded the predicted `~4800 MiB` target by landing
  at `3424 MiB`.
- Track A performance ledger `RSS / memory` row updated with the
  jemalloc numbers; `Bulk indexing` row updated with the new
  TREC-COVID gap (`1.42x` vs OpenSearch).

## Next perf attack candidates

- **Lot 1.6** — incremental term dictionary build. Targets the
  residual `1.42x` Surch/OS TREC-COVID bulk gap (cumulative
  `terms.build()` on every `_bulk` POST). Now the main remaining
  Surch bulk cost.
- **Lot 2** — skip lists on the codec FoR path (orthogonal to
  bulk, accelerates search).

## Notes

- Surch binary size grew ~1 MB from the statically-linked
  jemalloc (~600 KB compressed in the GHCR layer).
- jemalloc parity with Elasticsearch / OpenSearch is now achieved:
  both engines use jemalloc on Linux. The remaining Surch/OS
  difference is purely algorithmic, not allocator.
- INSEE-side replay still needs a paired RSS rerun on the
  jemalloc build to confirm the gain transfers to the matchID
  shape.
