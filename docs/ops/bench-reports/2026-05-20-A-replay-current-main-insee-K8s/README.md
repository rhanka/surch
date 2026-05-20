# Track A replay kickoff - current main INSEE K8s

First promoted proof created by the cumulative Track A replay branch.
This is not an isolated historical algorithm replay; it is the
current-main anchor that proves the replay pipeline and records the
latest INSEE 10k latency envelope before older A-replay lots are
enabled.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26193166785>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=insee-bench`)
- Artifact:
  `k8s-bench-insee-bench-466693f55e1a3cd8b007e058be07584251986ecb`
- Artifact id: `7123081611`
- Artifact digest:
  `sha256:28a23e8f5f72a0078d62b691da7af43f98d7db01b64850dff0b8da0766a0c6b9`
- Ref: `main`
- Measured Surch SHA: `466693f55e1a3cd8b007e058be07584251986ecb`
- Surch image:
  `ghcr.io/rhanka/surch:sha-466693f55e1a3cd8b007e058be07584251986ecb`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-466693f55e1a3cd8b007e058be07584251986ecb`
- Reference engine image: `opensearchproject/opensearch:2.17.1`
- Kubernetes job manifest: `job.yaml`
- Human summary: `summary.md`

`main` later advanced to `6710490` with documentation-only Track D
promotion. No search code changed between the measured SHA and that
push.

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
| Surch | 2.0 | 3.7 | 5.4 | 36.8 | 13 170 | 0 |
| OpenSearch 2.17.1 | 4.5 | 9.7 | 18.2 | 362.8 | 13 170 | 0 |

Surch speedup on this capture:

- p50: 2.3x faster.
- p95: 2.6x faster.
- p99: 3.4x faster.
- max: 9.9x faster.

## SLO verdict

All emitted SLO checks passed:

- Surch p95 <= 200 ms: PASS, observed 3.7 ms.
- Surch max <= 500 ms: PASS, observed 36.8 ms.
- Surch error rate <= 1%: PASS, observed 0.000%.
- Reference p95 <= 200 ms: PASS, observed 9.7 ms.
- Reference max <= 500 ms: PASS, observed 362.8 ms.
- Reference error rate <= 1%: PASS, observed 0.000%.

## Missing proof

- RSS peak/final was not captured by this K8s job.
- This run is current-main only, not a historical before/after replay
  for a single algorithm.
- Quality guardrails are covered by `ndcg-gate`, not by `insee-bench`.

## Operator verdict

The replay pipeline is live and produces a human-promoted Track A
artifact. Current main keeps the matchID-style INSEE latency SLO with
wide headroom and no request errors. The next replay work remains the
historical A-replay-1 through A-replay-3 enablement described in
`plan/perf-replay-wp-a-algo-ledger.md`.
