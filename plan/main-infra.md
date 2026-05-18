# main Infra Plan

Track principal: E - infra K8s / poc-k8s
Branch: `main` until a dedicated infra branch is created
Owner: conductor / infra owner
Status: active infra lane; latest tracked main commit `5c25463`

## Finality

- [ ] Make `ci-k8s` a reliable heavy-benchmark target with preserved
  diagnostics.

## Scope

- [x] Delivered scope:
  `.github/workflows/ci-k8s.yml`, `docs/ops/k8s-ci.md`.
- [ ] Next scope: image production / consumption handoff and
  `make bench-k8s` entry point.
- [x] Image handoff issue fixed locally for next commit:
  `ci-k8s.yml`, `docker-build.yml`, and `release.yml` use
  `ghcr.io/rhanka/surch:sha-<full commit SHA>`.
- [x] Human action expected for next infra lot: none by default; the
  command path is printed by `make bench-k8s` and by the missing-image
  preflight.
- [x] Evidence source: GitHub Actions runs, pod diagnostics, and
  workflow artefacts.

## Merge State

- [x] Fail-fast GHCR preflight merged to `main`: `23e60b8`.
- [x] Full-SHA image handoff merged to `main`: `5c25463`.
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
  - [x] Align image tag convention between `docker-build.yml`,
    `release.yml`, and `ci-k8s.yml`.
  - [x] Make `make bench-k8s` print the exact image tag it expects.
  - [x] If the tag is missing, print the exact remediation command:
    trigger `docker-build.yml` for the same ref, then rerun `ci-k8s`.
  - [x] Keep the missing-image preflight fail-fast.
  - [x] Commit main: `5c25463`.
  - [ ] Gate 1: missing image fails fast with actionable message.
  - [ ] Gate 2: existing image reaches benchmark execution.

- [ ] Lot 3 - Heavy-run standardisation
  - [ ] Make `ci-k8s` the standard burst / large-corpus path.
  - [ ] Ensure diagnostics and artefacts are preserved on failure.
  - [ ] Turn `make bench-k8s` into a real entry point.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Record run ids and artefact paths.
