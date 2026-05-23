# BEIR NDCG gate - 2026-05-22 (K8s, Surch 7 GiB)

First `ndcg-gate` run that ingests the full 171 k TREC-COVID corpus
end-to-end without OOM, on the Scaleway burst pool after the node pool
upgrade + Surch container memory cap bump (`4Gi -> 7Gi`) in `d9cac15`.

SciFact stays comparable to OpenSearch 2.17.1 with Surch slightly
ahead on both NDCG@10 and Recall@10. TREC-COVID is now a real
cross-engine measurement: Surch trails OpenSearch by `-0.0152` NDCG@10
(`-3.1%` relative) on the full corpus, with identical Recall@10.

This run does not promote a memory win: the RSS sampler wiring landed
in `b9faefe`, so `surch.bench.rss.v1` envelopes are not present in
this artifact set. The Track A performance ledger still records
`RSS: not captured by current harness` for the rows that cite this
report. Surch memory peak is reported here as `kubectl top` evidence
only.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26304471549>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=ndcg-gate`)
- Job result: PASS, Kubernetes Job `SuccessCriteriaMet=True`,
  `Complete=True`
- Job duration (CI total): 30m11s (`18:14:26Z -> 18:44:37Z`)
- Gate window inside the pod (first SciFact start to last TREC-COVID
  end): 21m38s (`18:22:00Z -> 18:43:38Z`)
- Head SHA: `d9cac15b3b62f405c4bc52c30764f6b1db357a73`
- Surch image:
  `ghcr.io/rhanka/surch:sha-d9cac15b3b62f405c4bc52c30764f6b1db357a73`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-d9cac15b3b62f405c4bc52c30764f6b1db357a73`
- Artifact:
  `k8s-bench-ndcg-gate-d9cac15b3b62f405c4bc52c30764f6b1db357a73`
  (id `7167929039`, 34 KiB)
- Raw files in this directory: `summary.md`, `bench.json`, `job.yaml`.

## Environment

The pod ran one driver container plus Surch and OpenSearch sidecars on
the `burst` node pool. OpenSearch was
`opensearchproject/opensearch:2.17.1` with `-Xms1g -Xmx1g`.

| Container | CPU request | CPU limit | Mem request | Mem limit |
|---|---:|---:|---:|---:|
| `ndcg-driver` | 100m | 500m | 128Mi | 1Gi |
| `surch` (initContainer, restartPolicy=Always) | 150m | 2000m | 256Mi | **7Gi** |
| `opensearch` (initContainer, restartPolicy=Always) | 250m | 1200m | 256Mi | 2Gi |

`activeDeadlineSeconds=3600` covers the full TREC-COVID ingest (~17
min) plus SciFact and the four query phases.

## SciFact

5183 docs / 300 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 3661.5 ms | 7843.4 ms | Surch 2.1x faster |
| NDCG@10 | 0.6576 | 0.6537 | Surch +0.6% |
| Recall@10 | 0.8100 | 0.8033 | Surch +0.8% |

SciFact keeps the `NDCG@10 >= 0.65` floor and reproduces the
historical paired baseline (`2026-05-16-vs-os-2.17.1`).

## TREC-COVID (full 171 k corpus)

171332 docs / 50 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 1 001 949.9 ms (~16m42s) | 72 273.2 ms (~1m12s) | OpenSearch 13.9x faster |
| NDCG@10 | 0.4750 | 0.4902 | Surch `-0.0152` (`-3.1%` relative) |
| Recall@10 | 0.0132 | 0.0132 | Tie |
| Lucene/Anserini baseline NDCG@10 | 0.595 | 0.595 | Both engines below reference |

Quality verdict: Surch is a real cross-engine BEIR result on
TREC-COVID for the first time. The previous false-green
(`2026-05-20-ndcg-gate-K8s`, `NDCG@10 = 0.0000`) is fully retired.
Surch lags OpenSearch by `0.0152` NDCG@10 on the same corpus, the
same queries, and the same qrels.

Ingest verdict: bulk indexing is now the dominant cost on Surch for
TREC-COVID. Surch took `~16m42s` vs OpenSearch `~1m12s` on the same
8 MiB pair-aware chunks, a `13.9x` slowdown that does not appear on
SciFact (Surch is 2.1x faster there). This points at a Surch-side
scaling issue specific to TREC-COVID corpus shape (long text/title
fields, ~33x more docs than SciFact), not at the chunker.

## Memory shape (kubectl top, peak observed)

Live `kubectl top pod` samples were captured every ~13 s for 28 min.

| Container | Peak | Limit | Notes |
|---|---:|---:|---|
| `surch` | 5274 MiB (~5.15 GiB) | 7 GiB | Steady plateau in the post-ingest phase; +200 MiB/chunk growth observed at 4 GiB is now bounded under the new cap. |
| `opensearch` | 1475 MiB (~1.44 GiB) | 2 GiB | Steady. |
| `ndcg-driver` | 15 MiB | 1 GiB | Negligible. |

This is `kubectl top` evidence, not a sampled-RSS measurement. The
`surch.bench.rss.v1` paired RSS envelopes from `rss-sample.sh` will
land in the next `ndcg-gate` run on `b9faefe` or later.

## Verdict

| Axis | Verdict |
|---|---|
| SciFact quality | PASS, Surch slightly ahead of OpenSearch (`+0.6%` NDCG@10, `+0.8%` Recall@10). |
| TREC-COVID quality | First real cross-engine result. Surch is within `-3.1%` of OpenSearch on NDCG@10; Recall@10 is tied. Not a win, not a blocker; this is the new TREC-COVID baseline. |
| Bulk indexing | SciFact: Surch wins `2.1x`. TREC-COVID: OpenSearch wins `13.9x`. The TREC-COVID bulk gap is the next Surch ingest-perf target. |
| Memory | Surch full-corpus footprint is `5.15 GiB / 7 GiB` cap. RSS peak/final remains `not captured by current harness` in the Track A ledger; promotion of a paired-RSS run on `b9faefe` is the next step. |
| SLO / errors | Job condition `Complete=True`, `SuccessCriteriaMet=True`, no driver-side `curl` failures, no Surch OOM, no OpenSearch error. |

## Follow-ups

- Promote a successor `ndcg-gate` run on `b9faefe` (RSS sampler
  wired) so the Track A ledger can drop `RSS: not captured by current
  harness` for the SciFact / TREC-COVID rows.
- File a Surch bulk-ingest scaling task for TREC-COVID-shaped corpora
  (long text fields, 171 k docs) before claiming an indexing win
  there. SciFact bulk parity is not at risk.
