# B1 oracle gate - Elasticsearch 8.6.1

Canonical Track D proof for the `deces_v1.json` matchID replay against
Surch and Elasticsearch 8.6.1 on the Scaleway burst-pool K8s runner.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26192816780>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=b1-oracle-gate`)
- Artifact:
  `k8s-bench-b1-oracle-gate-466693f55e1a3cd8b007e058be07584251986ecb`
- Artifact id: `7122750629`
- Artifact digest:
  `sha256:d4c8ca410d9ef58ef80c57651e8a3067a14357f1d633c709e39917caee7fcd89`
- Ref: `main`
- Surch SHA: `466693f55e1a3cd8b007e058be07584251986ecb`
- Surch image:
  `ghcr.io/rhanka/surch:sha-466693f55e1a3cd8b007e058be07584251986ecb`
- Bench driver image:
  `ghcr.io/rhanka/surch:bench-sha-466693f55e1a3cd8b007e058be07584251986ecb`
- Oracle image:
  `docker.elastic.co/elasticsearch/elasticsearch:8.6.1`
- Kubernetes job manifest: `job.yaml`

The uploaded artifact contains 17 files. It did not include a standalone
`/reports/b1-oracle.json`; the promoted `b1-oracle.json` in this
directory is the envelope emitted by the driver log between
`BEGIN_SURCH_K8S_B1_ORACLE` and `END_SURCH_K8S_B1_ORACLE`.

## Verdict

- Gate result: PASS.
- Kubernetes conditions: `SuccessCriteriaMet=True`, `Complete=True`.
- Runtime: 3m21s in GitHub Actions; the K8s job completed in the
  30-minute cap.
- Total replay requests: 30.
- Skipped requests: 0.
- Unexpected divergences: 0.
- Oracle exit code: 0.

Captured envelope:

```json
{
  "divergence_count": 0,
  "divergences": [],
  "es_url": "http://127.0.0.1:9200",
  "generated_at": "2026-05-20T22:10:41Z",
  "schema": "surch.bench.b1_oracle.v1",
  "skipped_count": 0,
  "surch_url": "http://127.0.0.1:7700",
  "total_requests": 30
}
```

## Secondary confirmation

A second manual dispatch on the same `main` SHA also passed:

- GHA run: <https://github.com/rhanka/surch/actions/runs/26193038471>
- Job: `b1-oracle-gate`
- Duration: 2m43s.
- Purpose: duplicate confirmation only; `26192816780` remains the
  canonical promoted proof for Track D.

## Operator verdict

The B1 matchID oracle phase is closed for Elasticsearch 8.6.1 on the
30-request `deces_v1` replay: Surch and the oracle returned matching
status, total-hit, top-id, and critical response-shape checks for every
request.

Phase 4 widening is still a separate follow-up: broader matchID
fixtures, multi-field/date/geo/edge-ngram coverage, and `deces_v2`
INSEE replay should be scoped in a new plan instead of reopening this
B1 proof.
