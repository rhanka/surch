# Track A replay - current main INSEE K8s repetition 1

This report is a promoted repetition for the cumulative Track A replay
line. It counts as repetition `1/3` for the current-main INSEE K8s
run group. It is not a final performance verdict by itself; the final
verdict still requires at least three successful repetitions of the same
workload, ref, runtime image, and bench-driver image.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26202012197>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=insee-bench`)
- Workflow verdict: PASS
- Artifact:
  `k8s-bench-insee-bench-ac558e6d08c7566f8cbc0b96c56a5b943eb1ae79`
- Artifact id: `7126271947`
- Artifact digest:
  `sha256:274ed630818f02fa12cfdc85c76112d2dc6db472d1fe947b11cf8edfdeb75994`
- Ref: `main`
- Measured Surch SHA: `ac558e6d08c7566f8cbc0b96c56a5b943eb1ae79`
- Surch image:
  `ghcr.io/rhanka/surch:sha-ac558e6d08c7566f8cbc0b96c56a5b943eb1ae79`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-ac558e6d08c7566f8cbc0b96c56a5b943eb1ae79`
- Reference engine image: `opensearchproject/opensearch:2.17.1`
- Kubernetes job manifest: `job.yaml`
- Human summary: `summary.md`
- Pod metrics samples: `pods.top.samples.txt`
- Node metrics samples / RBAC diagnostic: `nodes.top.samples.txt`

## K8s configuration

- Namespace: `surch`
- Node pool selector: `k8s.scaleway.com/pool-name=burst`
- Node observed: `scw-poc-burst-c1f70bea719b428797bdd6485188061c`
- Toleration: `pool=burst:NoSchedule`
- Active deadline: `1800s`
- Job completion: `SuccessCriteriaMet=True`, `Complete=True`
- Live quota recorded in plan: requests `1500m CPU / 1Gi memory`,
  limits `4500m CPU / 6Gi memory`
- PVCs: `surch-corpus-insee` read-only, `surch-scratch` read/write,
  `reports` EmptyDir `128Mi`

Container resource shape:

| Container | Requests | Limits |
| --- | --- | --- |
| `surch` | `150m`, `128Mi` | `800m`, `512Mi` |
| `opensearch` | `250m`, `256Mi` | `1200m`, `1536Mi` |
| `artillery-runner` | `100m`, `128Mi` | `500m`, `512Mi` |

## Workload

- Fixture: INSEE 10k `deces` slice from
  `tests/matchid_compat/deces/slice-10000.ndjson.gz`.
- Scenario: matchID-style artillery workload with 8 workers.
- Phases: 2, 2, 5, 10, 20, then 50 RPS for 240 seconds.
- Issued requests: 13 170 per engine.
- Errors: 0 on both engines.

## Latency

| Engine | p50 ms | p95 ms | p99 ms | max ms | issued | errors |
|---|---:|---:|---:|---:|---:|---:|
| Surch | 1.9 | 3.5 | 5.0 | 25.0 | 13 170 | 0 |
| OpenSearch 2.17.1 | 4.5 | 9.3 | 16.3 | 354.1 | 13 170 | 0 |

Surch speedup on this repetition:

- p50: 2.4x faster.
- p95: 2.7x faster.
- p99: 3.3x faster.
- max: 14.2x faster.

## Monitoring

`pods.top.samples.txt` captured live `kubectl top pods --containers`
samples during the run. Peak observed samples:

| Container | Samples | max CPU | max memory |
| --- | ---: | ---: | ---: |
| `surch` | 61 | 91m | 77Mi |
| `opensearch` | 59 | 1200m | 1476Mi |
| `artillery-runner` | 59 | 113m | 4Mi |

`nodes.top.samples.txt` preserves the cluster RBAC diagnostic:
`nodes.metrics.k8s.io is forbidden` for the workflow service account.
That means node-level `kubectl top nodes` is not available from this
workflow yet, but the missing node metric is recorded instead of silently
dropping monitoring evidence.

## SLO verdict

All emitted SLO checks passed:

- Surch p95 <= 200 ms: PASS, observed 3.5 ms.
- Surch max <= 500 ms: PASS, observed 25.0 ms.
- Surch error rate <= 1%: PASS, observed 0.000%.
- Reference p95 <= 200 ms: PASS, observed 9.3 ms.
- Reference max <= 500 ms: PASS, observed 354.1 ms.
- Reference error rate <= 1%: PASS, observed 0.000%.

## Repeatability state

- Accepted repetitions for this current-main replay group: `1/3`.
- Invalid prior attempts: `26200481514` lacked live pod metrics;
  `26201223312` captured live pod metrics but the workflow false-failed
  before the sidecar-completion wait-loop fix.
- Required next step: run the same `insee-bench` workload on the same
  SHA and image tags two more times, then aggregate p50 / p95 / p99 /
  max as median and IQR, or min / median / max if the artifact shape
  cannot support IQR.

## Missing proof

- RSS peak/final is still not captured by the current harness. The pod
  memory samples above are useful cluster monitoring, not a paired
  process RSS report.
- This is current-main only, not an isolated historical before/after
  replay for a single algorithm.
- Quality guardrails are covered by `ndcg-gate`, not by `insee-bench`.
