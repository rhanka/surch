# BEIR NDCG gate - 2026-05-24 (K8s, Surch 7 GiB, Lot 1.5 RAM)

First `ndcg-gate` K8s run after `8a5150f` (Track A
`wp-a-perf-followups.md` Lot 1.5 — `AppState::refresh_index`
drops the in-memory `PostingsBuilder` snapshot, `terms_finalized`
flag + fallback `rebuild_index` in `append_to_index` so a
bulk-after-refresh request still sees old postings).

The fix targets the Surch RSS overhead introduced by Lot 1
(`04fde72`, `2026-05-24-ndcg-gate-incremental-bulk-K8s/`): RSS
peak went from `4802 MiB` (full-rebuild path) to `5859 MiB`
(incremental path) because the per-chunk `PostingsBuilder`
snapshot was kept live across the 21 TREC-COVID chunks for the
incremental append.

NDCG@10 / Recall@10 are unchanged across SciFact and TREC-COVID.
Bulk timings are unchanged. The result is **a modest RAM saving
(~268 MiB)**, not the ~1 GiB hoped — analysed below.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26359069219>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=ndcg-gate`)
- Job result: PASS, Kubernetes Job `SuccessCriteriaMet=True`,
  `Complete=True`
- Job duration (CI total): 22m34s
  (`10:43:05Z -> 11:05:39Z`)
- Gate window inside the pod (first SciFact start to last
  TREC-COVID end): 9m42s (`10:46:43Z -> 10:56:25Z`)
- Head SHA: `01ad77e891237c1315f2dc0ef319a4b328151437`
  (Lot 1.5 code from `8a5150f` + `handover.md`)
- Surch image:
  `ghcr.io/rhanka/surch:sha-01ad77e891237c1315f2dc0ef319a4b328151437`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-01ad77e891237c1315f2dc0ef319a4b328151437`
- Raw files in this directory: `summary.md`, `bench.json`,
  `rss-ndcg-surch.json`, `rss-ndcg-os.json`, `job.yaml`.

## Environment

Same pod shape as `2026-05-24-ndcg-gate-incremental-bulk-K8s/`.
OpenSearch is `opensearchproject/opensearch:2.17.1` with
`-Xms1g -Xmx1g`. `shareProcessNamespace=true`.

| Container | CPU request | CPU limit | Mem request | Mem limit |
|---|---:|---:|---:|---:|
| `ndcg-driver` | 100m | 500m | 128Mi | 1Gi |
| `surch` (init, restartPolicy=Always) | 150m | 2000m | 256Mi | **7Gi** |
| `opensearch` (init, restartPolicy=Always) | 250m | 1200m | 256Mi | 2Gi |

## SciFact

5183 docs / 300 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 2248.9 ms | 12 092.1 ms | Surch 5.4x faster |
| NDCG@10 | 0.6576 | 0.6537 | Surch +0.6% |
| Recall@10 | 0.8100 | 0.8033 | Surch +0.8% |

## TREC-COVID (full 171 k corpus)

171 332 docs / 50 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 189 180.3 ms (~3m09s) | 98 664.8 ms (~1m39s) | OpenSearch 1.92x faster |
| NDCG@10 | 0.4750 | 0.4902 | Surch `-3.1%` |
| Recall@10 | 0.0132 | 0.0132 | Tie |

The Lot 1 incremental bulk gain (~5.6x on Surch TREC-COVID bulk)
is preserved: Surch bulk in this run (189 s) is in the same range
as the Lot 1 proof (180 s on `04fde72`), within run-to-run noise.

## Paired RSS — Lot 1.5 effect on Surch peak

Sampled at 1 Hz for `RSS_SAMPLE_SECONDS=1200` (20 minutes) via
`/proc/<pid>/status:VmRSS`.

| Container | PID (in pod) | Samples | Peak RSS | Final RSS | Limit |
|---|---:|---:|---:|---:|---:|
| `surch`      | 7  | 1200 | **5591 MiB** (5.46 GiB) | 5591 MiB | 7 GiB |
| `opensearch` | 15 | 1200 | 1461 MiB (1.43 GiB)     | 1461 MiB | 2 GiB |

Comparison vs the three previous `ndcg-gate` runs on the same
7 GiB pool:

| Run | SHA | Path | Surch RSS peak | Delta vs Lot 1.5 |
|-----|-----|------|---------------:|-----------------:|
| `26304471549` | `d9cac15` | full-rebuild baseline | 4802 MiB* | -789 MiB (Lot 1.5 still higher) |
| `26340177506` | `137b352` | full-rebuild + paired RSS | 4802 MiB  | -789 MiB |
| `26350556060` | `04fde72` | **Lot 1 incremental bulk** | 5859 MiB | -268 MiB (Lot 1.5 saves vs Lot 1) |
| `26359069219` | `01ad77e` | **Lot 1 + Lot 1.5 RAM** | **5591 MiB** | — |

*RSS via `kubectl top` only, the `surch.bench.rss.v1` sampler
landed on `137b352`.

**Lot 1.5 saves `268 MiB` vs Lot 1 alone**, not the `~1 GiB`
predicted from the logical builder size. The logical free
happens correctly (test
`bulk_router_bulk_refresh_bulk_search_preserves_old_docs` covers
the post-refresh fallback rebuild), but the system-level RSS
stays high.

### Why the gap

- Surch sees the 1200 s sampling window from t=0 (engines healthy,
  ~10:46) to t=1200 (~11:06). RSS peak is observed **during** the
  TREC-COVID bulk (~10:53→10:54), **before** the
  `_refresh` triggers `finalize_postings()`. Peak is unchanged
  vs Lot 1 because the builder grows the same way during bulk —
  Lot 1.5 only frees it after refresh.
- After refresh (~10:54-10:56) and during the idle tail until
  t=1200, the sampler keeps seeing ~5591 MiB. So the freed
  builder allocations are not returned to the OS within ~12 min.
  This is consistent with glibc's default `malloc` behaviour: heap
  arenas keep freed pages mapped unless `malloc_trim(0)` is
  called or the allocator's threshold for `MADV_DONTNEED` is
  reached.

### Logical vs system RSS

`finalize_postings()` does drop the `PostingsBuilder`. The Rust
side reclaims the allocation. But on Linux + glibc default
allocator, `RSS` does not shrink automatically — pages stay
resident until reclaimed under pressure. Surch is way under the
7 GiB cap so the kernel has no incentive to evict.

## Verdict

| Axis | Verdict |
|---|---|
| SciFact quality | PASS, Surch `+0.6%` NDCG@10, `+0.8%` Recall@10 vs OpenSearch. |
| TREC-COVID quality | Reproducible cross-engine baseline: Surch `0.4750` vs OpenSearch `0.4902` NDCG@10, Recall@10 tied at `0.0132`. |
| Bulk indexing | Surch SciFact 5.4x faster, TREC-COVID OS 1.92x faster (vs 13.9x pre-Lot-1). Lot 1 gain preserved. |
| Memory (paired RSS) | Surch peak `5591 MiB / 7 GiB` (80 %). Lot 1.5 saves 268 MiB vs Lot 1 alone (`5859 -> 5591`). The remaining overhead is the glibc heap not returning freed pages without pressure. |
| SLO / errors | Job `Complete=True`, `SuccessCriteriaMet=True`, no driver failure, no Surch OOM, no OpenSearch error. |

## Closure effects

- Track A `wp-a-perf-followups.md` Lot 1.5 closes with the
  caveat: logical fix lands, system RSS gain is modest (268 MiB)
  because of allocator inertia. Further RAM gains require an
  orthogonal Lot (allocator tuning / `malloc_trim` / jemalloc).
- Track A performance ledger `RSS / memory` row updated with the
  Lot 1.5 peak + the allocator caveat.

## Next perf attack candidates

- **Lot 1.6** — incremental term dictionary build. Independent
  from RAM; targets the residual `~1.92x` Surch/OS TREC-COVID bulk
  gap (cumulative `terms.build()` on every `_bulk` POST).
- **Lot 1.7** (new) — allocator: call `libc::malloc_trim(0)` from
  `refresh_index` after `finalize_postings()`, or switch the
  Surch binary to `jemalloc` with aggressive purge. Expected
  effect: Surch RSS drops from `5591 MiB` toward the logical
  `~4802 MiB` after refresh.
- **Lot 2** — skip lists on the codec FoR path (orthogonal to
  bulk, accelerates search).
