# BEIR NDCG gate - 2026-05-24 (K8s, Surch 7 GiB, incremental bulk)

First `ndcg-gate` K8s run that exercises the incremental bulk path
introduced in `367acdc` (Track A `wp-a-perf-followups.md` Lot 1
axis (c) — replace the per-chunk cumulative `rebuild_index()` with
an incremental `append_to_index` for pure-insert bulk batches).

The fix targets the Surch TREC-COVID bulk ingest scaling exposed by
`2026-05-22-ndcg-gate-7Gi-K8s/` (Surch `1001.95 s` vs OpenSearch
`72.27 s`, OpenSearch `13.9x` faster). NDCG@10 / Recall@10 are
unchanged across SciFact and TREC-COVID; SciFact bulk is also
slightly faster on Surch.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26350556060>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=ndcg-gate`)
- Job result: PASS, Kubernetes Job `SuccessCriteriaMet=True`,
  `Complete=True`
- Job duration (CI total): 22m38s (`03:15:17Z -> 03:37:55Z`)
- Gate window inside the pod (first SciFact start to last
  TREC-COVID end): 8m47s (`03:18:48Z -> 03:27:35Z`)
- Head SHA: `04fde721b3bc8cb360750153e7f329bd766c7a6c` (carries the
  `wp-e` `exit=143` wait-loop tolerance landed in the same PR so
  the workflow stops false-failing during sidecar cleanup)
- Surch image:
  `ghcr.io/rhanka/surch:sha-04fde721b3bc8cb360750153e7f329bd766c7a6c`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-04fde721b3bc8cb360750153e7f329bd766c7a6c`
- Raw files in this directory: `summary.md`, `bench.json`,
  `rss-ndcg-surch.json`, `rss-ndcg-os.json`, `job.yaml`.

## Environment

Same pod shape as `2026-05-23-ndcg-gate-7Gi-RSS-K8s/`. OpenSearch
is `opensearchproject/opensearch:2.17.1` with `-Xms1g -Xmx1g`.

| Container | CPU request | CPU limit | Mem request | Mem limit |
|---|---:|---:|---:|---:|
| `ndcg-driver` | 100m | 500m | 128Mi | 1Gi |
| `surch` (init, restartPolicy=Always) | 150m | 2000m | 256Mi | **7Gi** |
| `opensearch` (init, restartPolicy=Always) | 250m | 1200m | 256Mi | 2Gi |

`activeDeadlineSeconds=3600`. `shareProcessNamespace=true`.

## SciFact

5183 docs / 300 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 2289.1 ms | 10 891.2 ms | Surch 4.8x faster |
| NDCG@10 | 0.6576 | 0.6537 | Surch +0.6% |
| Recall@10 | 0.8100 | 0.8033 | Surch +0.8% |

## TREC-COVID (full 171 k corpus) — Lot 1 fix proof

171332 docs / 50 unique test queries.

| Metric | Surch (before fix, `d9cac15`) | Surch (this run, `04fde72`) | OpenSearch 2.17.1 | Delta vs OpenSearch (this run) |
|---|---:|---:|---:|---:|
| Bulk index | **1 001 950 ms** (~16m42s) | **179 860 ms** (~3m00s) | 87 044 ms (~1m27s) | **OpenSearch 2.06x faster** (was 13.9x) |
| NDCG@10 | 0.4750 | 0.4750 | 0.4902 | Surch `-3.1%` (unchanged) |
| Recall@10 | 0.0132 | 0.0132 | 0.0132 | Tie (unchanged) |

The incremental bulk path delivers an **~5.6x speedup** on Surch
TREC-COVID bulk ingest (1002 s -> 180 s) for an identical retrieval
result. The remaining `2.06x` gap to OpenSearch on this corpus
shape is the next perf attack surface (cumulative term dictionary
rebuild on each `_bulk` POST is now the dominant cost).

## Paired RSS (`surch.bench.rss.v1`)

Sampled at 1 Hz for `RSS_SAMPLE_SECONDS=1200` (20 minutes), reading
`/proc/<pid>/status:VmRSS`. PID resolution uses argv[0] basename
match plus optional content substring (Surch = `surch-api`,
OpenSearch = `java` + `org.opensearch.bootstrap`).

| Container | PID (in pod) | Samples | Peak RSS | Final RSS | Container limit |
|---|---:|---:|---:|---:|---:|
| `surch`      | 7  | 1200 | **5859 MiB** (5.72 GiB) | 5859 MiB | 7 GiB |
| `opensearch` | 15 | 1200 | **1459 MiB** (1.42 GiB) | 1459 MiB | 2 GiB |

Surch RSS rose from `4802 MiB` (full-rebuild path on
`2026-05-23-ndcg-gate-7Gi-RSS-K8s/`) to `5859 MiB` here: the
incremental append path keeps the per-chunk `PostingsBuilder` snapshot
live across chunks (it is the source of truth for subsequent
appends). The cumulative duplicate carries ~1 GiB on the TREC-COVID
shape — tracked as the next follow-up in
`plan/wp-a-perf-followups.md`. Surch fits comfortably under the
`7 GiB` cap (84%); OpenSearch RSS is unchanged.

## Verdict

| Axis | Verdict |
|---|---|
| SciFact quality | PASS, Surch `+0.6%` NDCG@10, `+0.8%` Recall@10 vs OpenSearch. |
| TREC-COVID quality | Reproducible cross-engine baseline: Surch `0.4750` vs OpenSearch `0.4902` NDCG@10, Recall@10 tied at `0.0132`. |
| Bulk indexing | SciFact: Surch wins `4.8x`. TREC-COVID: Surch closed the gap from `13.9x` to `2.06x` (gain `~5.6x`). |
| Memory (paired RSS) | Surch peak `5859 MiB / 7 GiB` (84%) — up from `4802 MiB` due to the live `PostingsBuilder` for incremental appends. OpenSearch peak unchanged. |
| SLO / errors | Job `Complete=True`, `SuccessCriteriaMet=True`, no driver failure, no Surch OOM, no OpenSearch error. The `exit=143` SIGTERM tolerance landed alongside this fix prevents the wait-loop from false-failing on sidecar cleanup. |

## Closure effects

- Track A `wp-a-perf-followups.md` Lot 1 closes: cumulative
  `rebuild_index()` quadratic bug fixed, paired Surch vs OpenSearch
  K8s evidence cited above, Track A performance ledger Bulk row
  updated.
- Track E remains closed: the `exit=143` wait-loop tolerance
  (`04fde72`) is the second consecutive win for `ci-k8s` as the
  standard heavy-run target after
  `2026-05-23-ndcg-gate-7Gi-RSS-K8s/`.

## Next perf attack

- Free the ~1 GiB `PostingsBuilder` snapshot once the index is
  declared read-mostly (call `finalize_postings()` from
  `refresh_index` with a re-build safety net on the next write).
  Tracked as a new follow-up in `plan/wp-a-perf-followups.md`.
- Cumulative term dictionary rebuild (`terms.build()` once per
  `_bulk`) is now the dominant Surch bulk cost (~95% of remaining
  `2.06x` gap). Candidate Lot for incremental term dictionary
  merge.
