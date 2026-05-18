# Surch — K8s burst CI/CD

Date: 2026-05-18
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
- The burst pool (DEV1-L, 0..1 autoscaled, Scaleway label
  `k8s.scaleway.com/pool-name=burst`) scales `0 -> 1` for the Job
  duration and back to `0` after the TTL.
- All Jobs carry `nodeSelector: k8s.scaleway.com/pool-name=burst`.
  They also keep a `pool=burst` toleration for clusters that taint burst
  nodes explicitly.
- Reports and diagnostics are written under `target/bench-reports/k8s/`
  and uploaded with `actions/upload-artifact@v4` even when the Job fails.
  The workflow also publishes a GitHub Actions step summary and stores
  the same Markdown recap as `<job>.gha-summary.md` inside the artifact.
- Before `kubectl apply`, `ci-k8s` now verifies that the expected GHCR
  tags already exist. A missing runtime
  `ghcr.io/rhanka/surch:sha-<full_sha>` image, or a missing benchmark
  driver `ghcr.io/rhanka/surch:bench-sha-<full_sha>` image for
  `ndcg-gate` / `insee-bench`, fails immediately with the matching
  `docker-build.yml` command instead of burning the full 30 min Job
  budget.
- The workflow renders manifests with `envsubst '${SURCH_SHA}'` only, so
  shell variables inside the Job scripts stay intact. Job manifests must
  use `ghcr.io/rhanka/surch:sha-${SURCH_SHA}` for the runtime and
  `ghcr.io/rhanka/surch:bench-sha-${SURCH_SHA}` for benchmark drivers,
  matching the `docker-build.yml` long-SHA tag contract.
- PVC dependencies declared by `claimName:` are checked before `kubectl
  apply`, so a missing corpus volume fails in seconds instead of waiting
  for the Job deadline.
- Job status is fail-closed: `Complete=True` passes; `Failed=True`,
  `FailureTarget=True`, pod phase `Failed`, terminal pod startup errors
  (`ErrImagePull` / `ImagePullBackOff` / container config errors /
  `StartError`), non-zero container exits, or a 30 min timeout fail the
  workflow.

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
- the Surch image tag `ghcr.io/rhanka/surch:sha-<full_sha>` exists and
  is pullable by the cluster;
- for `ndcg-gate` and `insee-bench`, the bench driver image tag
  `ghcr.io/rhanka/surch:bench-sha-<full_sha>` also exists and is
  pullable. `ci-k8s` does not build or publish these images; it only
  verifies the tags before dispatching the K8s Job.

`make bench-k8s` prints the exact image tag expected for `K8S_REF` and
the remediation command before it dispatches `ci-k8s`. It also prints
the dispatched run id and run URL before it starts watching, so the
heavy run can be resumed later from the exact GitHub Actions page.

`00-init-corpora` declares the three PVCs it needs:
`surch-corpus-beir` (5 Gi), `surch-corpus-insee` (2 Gi), and
`surch-scratch` (5 Gi). Run it before `ndcg-gate` or `insee-bench` so
those recurring Jobs can mount the corpus PVCs read-only. The INSEE PVC
is hydrated from matchID's stable examples fixtures:
`clients_test.csv`, `deaths_test.csv`, and the generated
`artillery_names.txt` adapter consumed by Surch's Rust artillery harness.
It intentionally avoids the live INSEE public endpoints, which are not a
reliable CI dependency.

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

- `<job>.gha-summary.md` with the run URL, K8s Job name, conditions, and
  the copied benchmark summary when one exists;
- `<job>.job.describe.txt` and `<job>.job.yaml`;
- `<job>.job.conditions.txt`;
- `<job>.pods.txt` and `<job>.pods.describe.txt`;
- `<job>.events.txt` and `<job>.job.events.txt`;
- `<job>.job.log`;
- per-pod/per-container logs, including `*.previous.log` when present;
- job-specific `/reports` files copied from the report container:
  - `ndcg-gate.bench.json`, `ndcg-gate.scifact-surch.out`,
    `ndcg-gate.scifact-os.out`
  - `insee-bench.summary.md`, `insee-bench.art-surch.json`,
    `insee-bench.art-os.json`

## Latest diagnostics

`ci-k8s` run `26058595173` proved the image handoff is now correct for
`ghcr.io/rhanka/surch:sha-236980c600a60c40a8f28e2c433558c59ec5d5f7`,
then failed inside `ndcg-gate` before the benchmark driver could run.
The uploaded artifact is
`k8s-bench-ndcg-gate-236980c600a60c40a8f28e2c433558c59ec5d5f7`.

The relevant root causes in the artifact are:

- `ndcg-driver` used the Surch distroless runtime image with
  `command: ["/bin/sh", "-c"]`; the image has no `/bin/sh`, so the
  container hit a `StartError`.
- The reference engine sidecar exited with code `126` because the
  pod-level non-root security context forced an incompatible user for
  its entrypoint.
- The previous wait loop did not surface those pod states early enough;
  it now prints per-container waiting / terminated / last-terminated
  reasons and fails on terminal states before the Job deadline.

Current repo response:

- `docker-build.yml` publishes `bench-sha-<full_sha>` from the
  `bench-driver` Dockerfile stage.
- `ndcg-gate` and `insee-bench` use that driver image for shell and
  benchmark tooling.
- The reference engine sidecar overrides the pod default with a
  `1000:1000` security context.
- `docker-build` run `26063701483` kept the runtime image publication
  green but failed the first bench-driver push because `.dockerignore`
  still excluded `scripts/bench/scifact-ndcg.sh`; the Docker context now
  re-includes only that script from the ignored scripts tree.

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
  that prints the run id / URL before optional `gh run watch`

## Known limits

- `00-init-corpora` is the first smoke target to run. It uses a Python
  image so download, zip extraction, and matchID CSV validation do not
  depend on optional shell tools.
- `ndcg-gate` and `insee-bench` use the dedicated
  `bench-sha-<full_sha>` driver image for `/bin/sh`, `wget`,
  `scifact-ndcg.sh`, `artillery_bench`, and `bench_report`; the
  distroless runtime image remains reserved for `surch-api`.
- The reference engine sidecar now declares its own `1000:1000`
  security context instead of inheriting the Surch runtime user's
  `65532:65532` pod default.
- A missing GHCR tag is treated as a workflow precondition failure, not
  as a 30 min benchmark timeout. When that happens, inspect the image
  publication workflow before re-running `ci-k8s`.
- `workflow_dispatch` is intentional. Add a PR trigger only after a few
  manual runs have reproduced the budget and image contract.
