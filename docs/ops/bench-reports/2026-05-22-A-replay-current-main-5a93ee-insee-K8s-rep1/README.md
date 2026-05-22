# Track A replay — current-main 5a93ee INSEE K8s rep 1

Single-repetition K8s diagnostic that extends the current-main replay
trace past the closed stable group on `61a13f871f810c98379375f2c94a10bbc696ac6e`
(`docs/ops/bench-reports/2026-05-21-A-replay-current-main-61a13f-insee-K8s/`).
The Surch hot path on `5a93ee331e3e779d851681c136d191c6d89c63a3` adds
the snapshot `_verify` / `_status` REST surface; the replay confirms
that the new endpoints did not regress the matchID artillery workload.

This is a **single repetition**, not a final repeated verdict. The
protocol in `plan/perf-replay-wp-a-algo-ledger.md` requires three
successful K8s repetitions on a stable ref before publishing a final
verdict. The next two repetitions on the same `5a93ee3` ref need to be
dispatched before any current-main claim is updated.

## Verdict

Valid single K8s diagnostic for search latency and SLO on the INSEE 10k
matchID-style workload, with no regression versus the closed `61a13f8`
repeated group.

- Repetitions accepted: `1/3`.
- Surch p50/p95/p99/max: `2.1 / 3.6 / 5.4 / 18.3 ms`.
- OpenSearch 2.17.1 p50/p95/p99/max: `4.3 / 9.1 / 17.6 / 317.8 ms`.
- Surch speedup vs OpenSearch: `2.0x / 2.5x / 3.3x / 17.4x` on
  p50/p95/p99/max.
- Errors: `0 / 13170` on both engines.
- SLO verdict: PASS for both engines (artillery p95 ≤ 200 ms, max
  ≤ 500 ms, error rate ≤ 1 %).
- RSS: not captured by current harness; container memory samples in
  `artifacts/insee-bench.pods.top.samples.txt` are Kubernetes container
  metrics, not paired process RSS.

## Provenance

| Rep | GHA run | Ref | Artifact |
| --- | --- | --- | --- |
| 1 | `26266511714` | `main` at `5a93ee331e3e779d851681c136d191c6d89c63a3` | `k8s-bench-insee-bench-5a93ee331e3e779d851681c136d191c6d89c63a3` |

Image tags used by the run:

- Surch runtime: `ghcr.io/rhanka/surch:sha-5a93ee331e3e779d851681c136d191c6d89c63a3`
- Bench driver: `ghcr.io/rhanka/surch:bench-sha-5a93ee331e3e779d851681c136d191c6d89c63a3`
- Reference engine: `opensearchproject/opensearch:2.17.1` (the benchmark
  summary row is labelled `elasticsearch` by the generic bench schema).

## K8s configuration

- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=insee-bench`).
- Namespace: `surch`.
- Kubernetes Job: `insee-bench`.
- Node pool selector: `k8s.scaleway.com/pool-name=burst`.
- Toleration: `pool=burst:NoSchedule`.
- Active deadline: `1800s`.
- Job completion: `SuccessCriteriaMet=True`, `Complete=True`.
- Live Surch tenant quota: requests `1500m CPU / 1Gi memory`, limits
  `4500m CPU / 6Gi memory`.
- PVCs: `surch-corpus-insee` read-only, `surch-scratch` read/write,
  `reports` EmptyDir.

Container resource shape (from `deploy/k8s/jobs/insee-bench.yaml`):

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

## Per-phase latency

Source: `artifacts/insee-bench-v2ggq.artillery-runner.log`.

| Phase | Engine | rps | duration s | issued | errors | p50 ms | p95 ms | p99 ms | max ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | surch | 2 | 30 | 60 | 0 | 2.1 | 3.4 | 6.8 | 6.8 |
| 2 | surch | 2 | 30 | 60 | 0 | 2.2 | 3.1 | 4.4 | 4.4 |
| 3 | surch | 5 | 30 | 150 | 0 | 2.0 | 4.1 | 5.4 | 9.2 |
| 4 | surch | 10 | 30 | 300 | 0 | 2.1 | 3.5 | 4.5 | 5.6 |
| 5 | surch | 20 | 30 | 600 | 0 | 1.9 | 3.4 | 4.1 | 5.1 |
| 6 | surch | 50 | 240 | 12000 | 0 | 2.1 | 3.7 | 5.4 | 18.3 |
| 1 | opensearch | 2 | 30 | 60 | 0 | 12.7 | 27.2 | 189.5 | 189.5 |
| 2 | opensearch | 2 | 30 | 60 | 0 | 11.7 | 19.8 | 27.0 | 27.0 |
| 3 | opensearch | 5 | 30 | 150 | 0 | 9.9 | 14.3 | 18.5 | 19.8 |
| 4 | opensearch | 10 | 30 | 300 | 0 | 7.8 | 11.8 | 15.7 | 61.1 |
| 5 | opensearch | 20 | 30 | 600 | 0 | 7.4 | 11.2 | 16.6 | 29.1 |
| 6 | opensearch | 50 | 240 | 12000 | 0 | 4.1 | 7.4 | 14.9 | 317.8 |

## Comparison vs the closed 61a13f triplet

Median of the closed `61a13f8` triplet vs this single 5a93ee3 run:

| Metric | 61a13f triplet median | 5a93ee3 rep 1 | Delta |
| --- | --- | --- | --- |
| Surch p50 ms | 2.1 | 2.1 | 0.0 |
| Surch p95 ms | 3.6 | 3.6 | 0.0 |
| Surch p99 ms | 5.0 | 5.4 | +0.4 |
| Surch max ms | 22.0 | 18.3 | -3.7 |
| OS p50 ms | 3.9 | 4.3 | +0.4 |
| OS p95 ms | 9.3 | 9.1 | -0.2 |
| OS p99 ms | 16.7 | 17.6 | +0.9 |
| OS max ms | 225.6 | 317.8 | +92.2 |

Surch p50/p95 are byte-for-byte identical to the closed triplet median,
p99 is +0.4 ms (within Surch's own min/median/max envelope on the
triplet, `4.5 / 5.0 / 9.6 ms`), and max is -3.7 ms. The OpenSearch max
spike (317.8 ms vs 225.6 ms median) is a single-run tail outlier, not
attributable to the Surch change.

No quality artifact for this run — the change touched only snapshot
REST surface, not the search ranker. The matchID 30-request replay
(`tests/matchid_compat/replays/deces_v1.json`) and the B1 ES 8.6.1
oracle gate (`ci-k8s` run `26192816780`) remain the active quality
witnesses for the hot path.

## Open follow-up

- [ ] Dispatch repetitions 2/3 and 3/3 of `insee-bench` on the
  `5a93ee3` ref to close a new stable repeated current-main group.
- [ ] Update the Track A performance ledger row with the closed
  triplet aggregation when reps 2 and 3 land.
- [ ] Track B paired RSS reporting remains the prerequisite before any
  memory-win claim on this trace.
