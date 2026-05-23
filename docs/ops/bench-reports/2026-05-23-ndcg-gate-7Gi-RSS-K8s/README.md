# BEIR NDCG gate - 2026-05-23 (K8s, Surch 7 GiB, paired RSS)

First `ndcg-gate` K8s run that carries a paired `surch.bench.rss.v1`
RSS sampling envelope for Surch and OpenSearch alongside the full
SciFact + TREC-COVID NDCG/Recall measurements. Supersedes
`2026-05-22-ndcg-gate-7Gi-K8s/` for the Track A ledger memory row
(that report had `kubectl top` evidence only, no paired RSS).

Run shape, image tags, node pool, container limits, SciFact and
TREC-COVID NDCG@10 / Recall@10 are reproducible vs `26304471549` on
`d9cac15`. The only new evidence here is the paired RSS sampling
plus the small bulk variance described below.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26340177506>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=ndcg-gate`)
- Job result: PASS, Kubernetes Job `SuccessCriteriaMet=True`,
  `Complete=True`
- Job duration (CI total): 28m41s (`18:23:36Z -> 18:52:17Z`)
- Gate window inside the pod (first SciFact start to last
  TREC-COVID end): 24m33s (`18:27:08Z -> 18:51:41Z`)
- Head SHA: `137b352…` (after rss sampler argv[0]-basename fix +
  driver-log marker reconstruction)
- Surch image:
  `ghcr.io/rhanka/surch:sha-137b352…`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-137b352…`
- Raw files in this directory: `summary.md`, `bench.json`,
  `rss-ndcg-surch.json`, `rss-ndcg-os.json`, `job.yaml`.

## Environment

Same pod shape as `2026-05-22-ndcg-gate-7Gi-K8s/`:

| Container | CPU request | CPU limit | Mem request | Mem limit |
|---|---:|---:|---:|---:|
| `ndcg-driver` | 100m | 500m | 128Mi | 1Gi |
| `surch` (init, restartPolicy=Always) | 150m | 2000m | 256Mi | **7Gi** |
| `opensearch` (init, restartPolicy=Always) | 250m | 1200m | 256Mi | 2Gi |

`activeDeadlineSeconds=3600`. `shareProcessNamespace=true`.
OpenSearch is `opensearchproject/opensearch:2.17.1` with
`-Xms1g -Xmx1g`.

## SciFact

5183 docs / 300 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 3290.3 ms | 11 376.1 ms | Surch 3.5x faster |
| NDCG@10 | 0.6576 | 0.6537 | Surch +0.6% |
| Recall@10 | 0.8100 | 0.8033 | Surch +0.8% |

SciFact reproduces the historical paired baseline.

## TREC-COVID (full 171 k corpus)

171332 docs / 50 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 1 112 516.0 ms (~18m33s) | 93 796.4 ms (~1m34s) | OpenSearch 11.9x faster |
| NDCG@10 | 0.4750 | 0.4902 | Surch `-0.0152` (`-3.1%` relative) |
| Recall@10 | 0.0132 | 0.0132 | Tie |

NDCG@10 / Recall@10 are identical to `2026-05-22-ndcg-gate-7Gi-K8s/`
(same algorithm, same corpus, same qrels). Bulk timings vary
slightly across runs: Surch `1 112 516 ms` here vs `1 001 950 ms` on
`d9cac15` (+11.0%); OpenSearch `93 796 ms` here vs `72 273 ms` on
`d9cac15` (+29.8%). The 11.9x Surch/OS bulk gap on TREC-COVID
remains within the same order of magnitude as the 13.9x reported on
`d9cac15`; the gap is the Track A perf follow-up triggered by these
two runs (`plan/wp-a-perf-followups.md` Lot 1).

## Paired RSS (`surch.bench.rss.v1`)

Sampled at 1 Hz for `RSS_SAMPLE_SECONDS=1200` (20 minutes) from
`rss-sample.sh` reading `/proc/<pid>/status:VmRSS`. PID resolution
uses argv[0] basename match (Surch `surch-api`, OpenSearch `java`
with required `org.opensearch.bootstrap` content substring) so the
driver shell process is not mistakenly captured.

| Container | PID (in pod) | Samples | Peak RSS | Final RSS | Container limit |
|---|---:|---:|---:|---:|---:|
| `surch`      | 7  | 1200 | **4802 MiB** | 4802 MiB | 7 GiB |
| `opensearch` | 15 | 1200 | **1395 MiB** | 1395 MiB | 2 GiB |

`peak_kb == final_kb` for both engines: the 20 min sampling window
ends with TREC-COVID still in flight, so the post-ingest decay is
not captured here. The Surch peak (4802 MiB, 70 % of the 7 GiB cap)
is consistent with the `kubectl top` plateau of `5274 MiB` observed
on the `d9cac15` run; the difference is the 1 Hz `/proc` reader vs
the kubelet metrics aggregator window.

For OpenSearch the RSS peak matches the configured `Xmx=1g` plus
JVM overhead and Lucene off-heap caches.

## Verdict

| Axis | Verdict |
|---|---|
| SciFact quality | PASS, Surch `+0.6%` NDCG@10, `+0.8%` Recall@10 vs OpenSearch. |
| TREC-COVID quality | Surch within `-3.1%` of OpenSearch on NDCG@10; Recall@10 tied. Same baseline as `2026-05-22-ndcg-gate-7Gi-K8s/`. |
| Bulk indexing | SciFact: Surch wins `3.5x`. TREC-COVID: OpenSearch wins `11.9x`. Tracked as the next Surch ingest scaling target. |
| Memory (paired RSS) | Surch peak `4802 MiB / 7 GiB` (70 %). OpenSearch peak `1395 MiB / 2 GiB` (70 %). Both fit comfortably; Surch full-corpus footprint is `~3.4x` OpenSearch peak. |
| SLO / errors | Job `Complete=True`, `SuccessCriteriaMet=True`, no driver `curl` failure, no Surch OOM, no OpenSearch error. |

## Closure effects

- Track B last leaf (paired RSS reporting) closes: this report carries
  the first `surch.bench.rss.v1` envelopes alongside `bench.json` and
  `summary.md`.
- Track E last leaf (`ci-k8s` as the standard heavy-run target)
  closes: the ndcg-gate K8s harness now produces summary, bench JSON,
  paired RSS, Job conditions, pod describe, live metrics samples,
  and cluster events in a single artifact reconstructed from driver
  log markers after Job completion.
- Track A performance ledger: the `Memory / RSS` and `Bulk indexing`
  rows updated to cite this report; the
  `RSS: not captured by current harness` line is dropped for the
  SciFact / TREC-COVID rows in the same commit.
