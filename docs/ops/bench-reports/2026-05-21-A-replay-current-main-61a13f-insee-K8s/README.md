# Track A replay - current main 61a13f INSEE K8s repeated group

This report closes the first repeated Track A K8s replay group for the
current hot path after `main` advanced past the earlier `ac558e6`
single repetition. The group uses one stable ref, one runtime image, one
bench-driver image, and the same `insee-bench` workload for all three
repetitions.

## Verdict

Valid repeated K8s performance proof for search latency and SLO on the
INSEE 10k matchID-style workload.

- Repetitions accepted: `3/3`.
- Surch median p50/p95/p99/max: `2.1 / 3.6 / 5.0 / 22.0 ms`.
- OpenSearch 2.17.1 median p50/p95/p99/max: `3.9 / 9.3 / 16.7 /
  225.6 ms`.
- Median speedup: `1.9x / 2.6x / 3.3x / 10.3x` on p50/p95/p99/max.
- Errors: `0/13170` on every run for both engines.
- SLO verdict: PASS on every run for both engines.
- RSS: not captured by current harness. Pod memory samples below are
  Kubernetes container metrics, not paired process RSS.

## Provenance

| Rep | GHA run | Ref | Artifact id | Artifact digest |
| --- | --- | --- | ---: | --- |
| 1 | `26202652997` | `main` at `61a13f871f810c98379375f2c94a10bbc696ac6e` | `7126549971` | `sha256:9d57735f72d6dfa46e40285fd3b37741174a50247c09ebbec2742dab16d69d6f` |
| 2 | `26203320060` | `perf-replay/current-main-61a13f` | `7126727126` | `sha256:9084648fbbda45194aa91a4e6c6e9dc749f8a38b03eda41a08f9ae0b781fd707` |
| 3 | `26204062094` | `perf-replay/current-main-61a13f` | `7126979242` | `sha256:965406cde73f0704a89e7683f5b8641683b3681626b2a7581a9f5749add82d24` |

All runs measured the same Surch commit:

- SHA: `61a13f871f810c98379375f2c94a10bbc696ac6e`
- Surch image:
  `ghcr.io/rhanka/surch:sha-61a13f871f810c98379375f2c94a10bbc696ac6e`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-61a13f871f810c98379375f2c94a10bbc696ac6e`
- Reference engine image: `opensearchproject/opensearch:2.17.1`
  (the benchmark summary row is labelled `elasticsearch` by the
  generic bench schema).

## K8s configuration

- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=insee-bench`)
- Namespace: `surch`
- Kubernetes Job: `insee-bench`
- Node pool selector: `k8s.scaleway.com/pool-name=burst`
- Observed node: `scw-poc-burst-c1f70bea719b428797bdd6485188061c`
- Toleration: `pool=burst:NoSchedule`
- Active deadline: `1800s`
- Job completion on all repetitions: `SuccessCriteriaMet=True`,
  `Complete=True`
- Live quota recorded for the Surch namespace: requests
  `1500m CPU / 1Gi memory`, limits `4500m CPU / 6Gi memory`
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
- Issued requests: 13 170 per engine per repetition.
- Errors: 0 on both engines in all repetitions.

## Per-run latency

| Rep | Engine | p50 ms | p95 ms | p99 ms | max ms | issued | errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | Surch | 2.2 | 3.6 | 5.1 | 17.1 | 13170 | 0 |
| 1 | OpenSearch 2.17.1 | 3.7 | 9.3 | 16.9 | 338.3 | 13170 | 0 |
| 2 | Surch | 2.0 | 3.5 | 4.8 | 22.0 | 13170 | 0 |
| 2 | OpenSearch 2.17.1 | 3.9 | 7.9 | 14.5 | 212.5 | 13170 | 0 |
| 3 | Surch | 2.1 | 3.6 | 5.0 | 36.8 | 13170 | 0 |
| 3 | OpenSearch 2.17.1 | 4.3 | 9.5 | 16.7 | 225.6 | 13170 | 0 |

## Cross-run summary

The artifacts publish one percentile row per engine per repetition. With
three point estimates, this report publishes min/median/max instead of
IQR to avoid over-claiming distribution shape from summary percentiles.

| Engine | Metric | Min | Median | Max |
| --- | --- | ---: | ---: | ---: |
| Surch | p50 ms | 2.0 | 2.1 | 2.2 |
| Surch | p95 ms | 3.5 | 3.6 | 3.6 |
| Surch | p99 ms | 4.8 | 5.0 | 5.1 |
| Surch | max ms | 17.1 | 22.0 | 36.8 |
| OpenSearch 2.17.1 | p50 ms | 3.7 | 3.9 | 4.3 |
| OpenSearch 2.17.1 | p95 ms | 7.9 | 9.3 | 9.5 |
| OpenSearch 2.17.1 | p99 ms | 14.5 | 16.7 | 16.9 |
| OpenSearch 2.17.1 | max ms | 212.5 | 225.6 | 338.3 |

## Monitoring

`run-*-pods.top.samples.txt` captured live
`kubectl top pods --containers` samples during each run. Peak observed
samples:

| Rep | Container | Samples | max CPU | max memory |
| --- | --- | ---: | ---: | ---: |
| 1 | `surch` | 70 | 98m | 81Mi |
| 1 | `opensearch` | 67 | 1200m | 1472Mi |
| 1 | `artillery-runner` | 67 | 114m | 5Mi |
| 2 | `surch` | 66 | 93m | 77Mi |
| 2 | `opensearch` | 65 | 1201m | 1476Mi |
| 2 | `artillery-runner` | 64 | 111m | 4Mi |
| 3 | `surch` | 60 | 95m | 77Mi |
| 3 | `opensearch` | 59 | 1201m | 1476Mi |
| 3 | `artillery-runner` | 58 | 183m | 4Mi |

`run-*-nodes.top.samples.txt` preserves the cluster RBAC diagnostic:
`nodes.metrics.k8s.io is forbidden` for the workflow service account.
Node-level top metrics are therefore not available from this workflow,
but the missing metric is captured explicitly in every repetition.

## Raw promoted files

- `run-1-gha-summary.md`, `run-2-gha-summary.md`,
  `run-3-gha-summary.md`
- `run-1-job.yaml`, `run-2-job.yaml`, `run-3-job.yaml`
- `run-1-job.conditions.txt`, `run-2-job.conditions.txt`,
  `run-3-job.conditions.txt`
- `run-1-pods.txt`, `run-2-pods.txt`, `run-3-pods.txt`
- `run-1-pods.top.samples.txt`, `run-2-pods.top.samples.txt`,
  `run-3-pods.top.samples.txt`
- `run-1-nodes.top.samples.txt`, `run-2-nodes.top.samples.txt`,
  `run-3-nodes.top.samples.txt`

Full logs remain in the GitHub artifacts listed above.

## Missing proof

- RSS peak/final is still not captured by the current K8s harness.
- This repeated group is current-main cumulative proof, not isolated
  historical before/after attribution for each Track A algorithm.
- Ranking quality is not measured by `insee-bench`; ranking-sensitive
  replay lots still need the SciFact/TREC quality gate beside latency.
