# Surch — K8s burst CI/CD

Date: 2026-05-15
Status: MVP scaffolded (workflow dormant), waiting on poc-k8s PR #2.

## Architecture

```
+-----------------+    OIDC     +------------------+   kubeconfig   +-----------------+
| GitHub Actions  | <---------> | Scaleway IAM     | -------------> | Kapsule "poc"   |
| .github/...     |             | (short-lived JWT)|                | namespace surch |
| ci-k8s.yml      |             +------------------+                +--------+--------+
+-----------------+                                                          |
                                                                             v
                                                       +---------------------+----+
                                                       | burst pool (DEV1-L, 0..1)|
                                                       |   Job ndcg-gate         |
                                                       |   Job insee-bench       |
                                                       |   Job 00-init-corpora   |
                                                       +-------------------------+
```

- GHA acquires a Scaleway IAM token via OIDC federation (no static
  `SCW_SECRET_KEY` in repo secrets).
- `scw k8s kubeconfig get poc` writes a short-lived `~/.kube/config`.
- The burst pool (DEV1-L, 0..1 autoscaled, taint `pool=burst:NoSchedule`)
  scales `0 -> 1` for the Job duration and back to `0` after the TTL.
- All Jobs carry `nodeSelector: pool=burst` + a matching toleration.
- Reports surface either via `kubectl cp` from a Pod `emptyDir` or via
  GHA `actions/upload-artifact@v4`.

## Manual run

```bash
gh workflow run ci-k8s.yml -f job=ndcg-gate
gh workflow run ci-k8s.yml -f job=insee-bench
gh workflow run ci-k8s.yml -f job=00-init-corpora   # only after first landing
```

The workflow is gated `if: false` for the MVP. Remove that flag in
`.github/workflows/ci-k8s.yml` once poc-k8s PR #2 is merged and the
cluster owner confirms:

- the `surch` namespace exists with quotas applied,
- the `ghcr-pull` Secret is provisioned (PAT with `read:packages` on
  `ghcr.io/rhanka/surch`),
- the `burst` pool exists with the taint + nodeSelector contract,
- the three PVCs are bound (`surch-corpus-beir`, `surch-corpus-insee`,
  `surch-scratch`).

## Reading results

```bash
# Tail logs while a Job runs:
kubectl logs -n surch -l app.kubernetes.io/component=ndcg-gate -c ndcg-driver -f
kubectl logs -n surch -l app.kubernetes.io/component=insee-bench -c artillery-runner -f

# After completion, grab the JSON report:
POD=$(kubectl get pod -n surch -l app.kubernetes.io/component=ndcg-gate \
        -o jsonpath='{.items[0].metadata.name}')
kubectl cp -n surch "${POD}:/reports/bench.json" ./ndcg-gate.json -c ndcg-driver
```

GHA also uploads the report as an artifact named
`k8s-bench-<job>-<sha>` (see workflow step `Upload report`).

## Cost guardrails

| Knob                  | Value | Where enforced                                                |
| --------------------- | ----- | ------------------------------------------------------------- |
| `SCW_MAX_COST_EUR`    | 2     | trap on burst-pool tear-down (workflow `Burst pool down`)     |
| `SCW_MAX_DURATION_MIN`| 30    | `activeDeadlineSeconds: 1800` on every Surch Job              |
| `timeout-minutes`     | 35    | GHA workflow-level safety net (cap + 5 min)                   |

DEV1-L node-hour ~0.08€; a 30 min run costs ~0.04€. That leaves a 50x
margin against the 2€ ceiling. Idle quota at rest = 0 (no Deployment,
no CronJob ; only `workflow_dispatch` triggers it).

The CI gate is implemented as a single short-lived Pod with sidecars
(Surch + OpenSearch + driver). When the Job's TTL elapses, the
namespace returns to its 500m / 512Mi quota baseline.

## Files in this repo

- `.github/workflows/ci-k8s.yml` — GHA workflow (currently `if: false`)
- `deploy/k8s/jobs/00-init-corpora.yaml` — one-shot PVC pre-warm
- `deploy/k8s/jobs/ndcg-gate.yaml` — SciFact NDCG@10 parity gate
- `deploy/k8s/jobs/insee-bench.yaml` — paired bench Surch vs OS
- `Makefile` target `bench-k8s` — wrapper around `gh workflow run`

## TODO (tracked in the workflow as `# TODO`)

1. **OIDC role** — `SCW_OIDC_ROLE` secret to be set once poc-k8s issues
   the IAM role ARN. The Scaleway action name (`scaleway/action-oidc-auth`)
   is a placeholder until Scaleway publishes their official one.
2. **Kubeconfig fetch** — `scw k8s kubeconfig get` step is commented;
   re-enable after OIDC.
3. **Burst pool scale up/down** — `scw k8s pool update min-size=0 max-size=1`
   pre-run; revert post-run in `if: always()`. Needs the pool ID lookup
   wired (currently a shell pipe to `jq`).
4. **`envsubst` patch of the image SHA** — once the release workflow
   publishes `ghcr.io/rhanka/surch:sha-<short>`, the Job manifests can
   be applied verbatim. Until then the manifests carry `${SURCH_SHA}`
   placeholders.
5. **PR trigger** — currently `workflow_dispatch` only. Once the budget
   is reproduced on a few manual runs, uncomment the `pull_request`
   trigger at the top of `ci-k8s.yml`.
6. **bench_report `--out` flag** — `insee-bench.yaml` calls
   `bench_report --out`. The binary currently emits to stdout; add the
   flag in a follow-up so the kubectl cp path becomes deterministic.
7. **`make bench-k8s` body** — wrapper currently echoes a TODO; flip to
   `gh workflow run ci-k8s.yml -f job=$(JOB)` after the workflow ships.
