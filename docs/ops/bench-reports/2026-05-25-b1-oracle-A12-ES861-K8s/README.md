# Track D — matchID B1 oracle parity after A12 (vs Elasticsearch 8.6.1)

Re-run of the matchID B1 oracle gate after Track D step A12 (the
write-time sub-field projection from A10 now also feeds sort and
aggregation on the read path). Confirms A12 did not perturb matchID
parity against the Elasticsearch 8.6.1 reference.

- GHA run `26423292686` on `main` @ `9640169` — **PASS**.
- Surch and Elasticsearch 8.6.1 run as sibling containers in one Pod;
  the oracle driver replays the 30-request matchID B1 suite against
  both and diffs the responses.

## Result

| Metric | Value |
|--------|------:|
| total requests | 30 |
| divergences | **0** |
| skipped | 0 |

`30/30` identical, **0 divergence** — same as the A10 baseline
(`2026-05-25-b1-oracle-A10-ES861-K8s/`). Track D's sub-field work
(A10 write-time fan-out + A12 sort/agg projection) is parity-neutral
against Elasticsearch 8.6.1.

## Sources

- GHA run `26423292686` (ci-k8s `b1-oracle-gate`), image
  `sha-9640169…` / `bench-sha-9640169…`.
- `b1-oracle.json` (`surch.bench.b1_oracle.v1`, this dir).
- `job.yaml` (the dispatched K8s Job, this dir).
