# BEIR NDCG gate - 2026-05-20 (K8s)

First quota-unblocked `ndcg-gate` run on the Scaleway burst pool after
the Surch tenant quota was raised in `poc-k8s`.

This report is deliberately mixed: SciFact stays green and comparable to
OpenSearch 2.17.1, while TREC-COVID exposes a quality blocker for
Surch. Do not use this run as a TREC-COVID quality win.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26157480132>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=ndcg-gate`)
- Job result: PASS, Kubernetes Job `Complete=True`
- Job duration: 9m57s
- Head SHA: `69240116599e8e86f629f13f3d7611d73ff1a07d`
- Surch image:
  `ghcr.io/rhanka/surch:sha-69240116599e8e86f629f13f3d7611d73ff1a07d`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-69240116599e8e86f629f13f3d7611d73ff1a07d`
- Artifact:
  `k8s-bench-ndcg-gate-69240116599e8e86f629f13f3d7611d73ff1a07d`
- Raw files in this directory: `summary.md`, `bench.json`, `job.yaml`.

## Environment

The run used the live `surch` namespace quota from `poc-k8s` HEAD
`980d58d`:

- `requests.cpu=1500m`
- `requests.memory=1Gi`
- `limits.cpu=4500m`
- `limits.memory=6Gi`
- `persistentvolumeclaims=3`
- `pods=5`

The pod ran one driver container plus Surch and OpenSearch sidecars on
the `burst` node pool. OpenSearch was `opensearchproject/opensearch:2.17.1`.

## SciFact

5183 docs / 300 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 4097.5 ms | 12088.0 ms | Surch 3.0x faster |
| NDCG@10 | 0.6576 | 0.6537 | Surch +0.6% |
| Recall@10 | 0.8100 | 0.8033 | Surch +0.8% |

SciFact keeps the existing quality guardrail: Surch remains above the
`NDCG@10 >= 0.65` floor and slightly ahead of OpenSearch on this run.

## TREC-COVID

50 unique test queries.

| Metric | Surch | OpenSearch 2.17.1 | Delta |
|---|---:|---:|---:|
| Bulk index | 5116.1 ms | 28710.8 ms | Surch 5.6x faster |
| NDCG@10 | 0.0000 | 0.1141 | Surch fails quality |
| Recall@10 | 0.0000 | 0.0026 | Surch fails quality |

The infrastructure gate passed, but the retrieval result is not
acceptable for TREC-COVID. The next Track B/D action is to diagnose the
TREC-COVID query/index mismatch before turning this corpus into a
quality gate.

## Notes

The driver log emitted repeated `curl` HTTP errors during the run:

- `413` while loading or querying larger TREC-COVID payloads.
- `400` for several request shapes after that.

The scripts still processed all 50 unique TREC-COVID qids and exited 0.
That makes the run useful as a diagnostic artifact, but not as a
quality acceptance report.
