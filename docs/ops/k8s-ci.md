# Surch — K8s burst CI/CD

Date: 2026-05-17
Status: manual `workflow_dispatch` gate, fail-closed reporting enabled.

## Architecture

```
+-----------------+   KUBE_CONFIG_DATA   +-----------------+
| GitHub Actions  | -------------------> | Kapsule "poc"   |
| .github/...     |                      | namespace surch |
| ci-k8s.yml      |                      +--------+--------+
+-----------------+                               |
                                                  v
                            +---------------------+----+
                            | burst pool (DEV1-L, 0..1)|
                            |   Job ndcg-gate         |
                            |   Job insee-bench       |
                            |   Job 00-init-corpora   |
                            +-------------------------+
```

- GHA reads `KUBE_CONFIG_DATA` from repository secrets. The secret may be
  raw kubeconfig YAML or base64-encoded YAML.
- The burst pool (DEV1-L, 0..1 autoscaled, taint `pool=burst:NoSchedule`)
  scales `0 -> 1` for the Job duration and back to `0` after the TTL.
- All Jobs carry `nodeSelector: pool=burst` + a matching toleration.
- Reports and diagnostics are written under `target/bench-reports/k8s/`
  and uploaded with `actions/upload-artifact@v4` even when the Job fails.
- The workflow renders manifests with `envsubst '${SURCH_SHA}'` only, so
  shell variables inside the Job scripts stay intact.
- Job status is fail-closed: `Complete=True` passes, `Failed=True` or a
  30 min timeout fails the workflow.

## Manual run

```bash
make bench-k8s K8S_JOB=00-init-corpora
make bench-k8s K8S_JOB=ndcg-gate
make bench-k8s K8S_JOB=insee-bench
```

Useful local knobs:

```bash
make bench-k8s K8S_JOB=ndcg-gate K8S_DRY_RUN=1
make bench-k8s K8S_JOB=ndcg-gate K8S_WATCH=0
make bench-k8s K8S_JOB=ndcg-gate K8S_REF=main
```

Cluster prerequisites:

- the `surch` namespace exists with quotas applied;
- the `burst` pool exists with the taint + nodeSelector contract;
- `KUBE_CONFIG_DATA` is set in GitHub secrets;
- the three PVCs are bound: `surch-corpus-beir`,
  `surch-corpus-insee`, `surch-scratch`;
- the Surch image tag `ghcr.io/rhanka/surch:sha-<short_sha>` exists and
  is pullable by the cluster.

## Reading results

```bash
# Tail logs while a Job runs:
kubectl logs -n surch -l app.kubernetes.io/component=ndcg-gate -c ndcg-driver -f
kubectl logs -n surch -l app.kubernetes.io/component=insee-bench -c artillery-runner -f

# Inspect status and events after a failed run:
kubectl describe job -n surch ndcg-gate
kubectl get events -n surch --sort-by='.metadata.creationTimestamp'
```

GHA uploads an artifact named `k8s-bench-<job>-<sha>`. It contains:

- `<job>.job.describe.txt` and `<job>.job.yaml`;
- `<job>.pods.txt` and `<job>.pods.describe.txt`;
- `<job>.events.txt` and `<job>.job.events.txt`;
- `<job>.job.log`;
- per-pod/per-container logs, including `*.previous.log` when present;
- `<job>.json` when `/reports/bench.json` can be copied from the report
  container.

## Cost guardrails

| Knob                  | Value | Where enforced                                                |
| --------------------- | ----- | ------------------------------------------------------------- |
| `SCW_MAX_COST_EUR`    | 2     | documented guardrail; pool autoscaling keeps idle cost at 0   |
| `SCW_MAX_DURATION_MIN`| 30    | `activeDeadlineSeconds: 1800` on every Surch Job              |
| `timeout-minutes`     | 35    | GHA workflow-level safety net (cap + 5 min)                   |

DEV1-L node-hour ~0.08€; a 30 min run costs ~0.04€. That leaves a 50x
margin against the 2€ ceiling. Idle quota at rest = 0 (no Deployment,
no CronJob ; only `workflow_dispatch` triggers it).

The CI gate is implemented as a single short-lived Pod with sidecars
(Surch + OpenSearch + driver). When the Job's TTL elapses, the
namespace returns to its 500m / 512Mi quota baseline.

## Files in this repo

- `.github/workflows/ci-k8s.yml` — manual GHA workflow
- `deploy/k8s/jobs/00-init-corpora.yaml` — one-shot PVC pre-warm
- `deploy/k8s/jobs/ndcg-gate.yaml` — SciFact NDCG@10 parity gate
- `deploy/k8s/jobs/insee-bench.yaml` — paired bench Surch vs OS
- `Makefile` target `bench-k8s` — wrapper around `gh workflow run`

## Known limits

- `00-init-corpora` is the first smoke target to run. It uses a Python
  image so download and zip extraction do not depend on optional shell
  tools.
- `ndcg-gate` and `insee-bench` still depend on the Surch runtime image
  shipping the driver tools they call (`/bin/sh`, `wget`,
  `scifact-ndcg.sh`, `artillery_bench`, `bench_report`). Until the image
  contract is updated, failures from those Jobs should be diagnosed from
  the uploaded describe/events/log artifacts.
- `workflow_dispatch` is intentional. Add a PR trigger only after a few
  manual runs have reproduced the budget and image contract.
