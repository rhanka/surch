# main Infra Plan

Track principal: E - infra K8s / poc-k8s
Branch: `main` until a dedicated infra branch is created
Owner: conductor / infra owner
Status: active infra lane; latest tracked main commit `23e60b8`

## Finality

- [ ] Make `ci-k8s` a reliable heavy-benchmark target with preserved
  diagnostics.

## Scope

- [x] Delivered scope:
  `.github/workflows/ci-k8s.yml`, `docs/ops/k8s-ci.md`.
- [ ] Next scope: image production / consumption handoff and
  `make bench-k8s` entry point.
- [x] Evidence source: GitHub Actions runs, pod diagnostics, and
  workflow artefacts.

## Merge State

- [x] Fail-fast GHCR preflight merged to `main`: `23e60b8`.
- [x] Verification run recorded: `ci-k8s` `26038117579` failed in 16s
  on missing image, replacing 30m timeout pattern.
- [x] `main` CI after latest integration was green: `26038398172`.
- [ ] `ci-k8s` heavy run reaches actual `ndcg-gate` benchmark.

## Lots

- [x] Lot 0 - Baseline and constraints
  - [x] Confirm infra surface:
    `.github/workflows/ci-k8s.yml`, `deploy/k8s/jobs/`,
    `docs/ops/k8s-ci.md`.
  - [x] Triage timeout run `26035416237`.
  - [x] Identify root cause: GHCR image missing, pod stuck in
    image-pull failure.

- [x] Lot 1 - Fail-fast missing image
  - [x] Add GHCR image preflight.
  - [x] Fail on terminal pod errors instead of waiting for timeout.
  - [x] Update docs.
  - [x] Commit main: `23e60b8`.
  - [x] Verify with run `26038117579`.

- [ ] Lot 2 - Image handoff
  - [ ] Decide whether `ci-k8s` depends on a prior image-build workflow,
    uses a tag fallback, or builds/pushes before bench.
  - [ ] Implement the chosen handoff.
  - [ ] Gate: missing image still fails fast; existing image reaches
    benchmark execution.

- [ ] Lot 3 - Heavy-run standardisation
  - [ ] Make `ci-k8s` the standard burst / large-corpus path.
  - [ ] Ensure diagnostics and artefacts are preserved on failure.
  - [ ] Turn `make bench-k8s` into a real entry point.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Record run ids and artefact paths.
