# main Infra Plan

Track principal: E - infra K8s / poc-k8s
Branch: `main` until a dedicated infra branch is created
Owner: conductor / infra owner
Status: active infra lane; merge state below is the source of truth

## Finality

- [ ] Make `ci-k8s` a reliable heavy-benchmark target with preserved
  diagnostics.

## Scope

- [x] Delivered scope:
  `.github/workflows/ci-k8s.yml`, `docs/ops/k8s-ci.md`.
- [ ] Next scope: prove restartable sidecar completion on GitHub
  Actions and promote `ci-k8s` as the heavy-run target.
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
- [x] Docker builder toolchain moved from `rust:1.88` to
  `rust:1.91.1` after `docker-build` run `26057290880` failed on AWS
  dependency MSRV.
- [x] K8s Job manifests consume the `sha-<full commit SHA>` GHCR tag
  that `docker-build.yml` actually publishes.
- [x] `ci-k8s` run `26058595173` proved the GHCR image handoff for
  `sha-236980c600a60c40a8f28e2c433558c59ec5d5f7` and preserved the
  failure artifact.
- [x] Wait loop now fails early on pod phase `Failed`, terminal
  waiting / terminated reasons, and non-zero container exits instead of
  relying only on Job conditions.
- [x] Docker build publishes a shell-capable benchmark driver image as
  `ghcr.io/rhanka/surch:bench-sha-<full commit SHA>` while keeping the
  default runtime image distroless.
- [x] `docker-build` run `26063701483` kept the runtime image green but
  failed the bench-driver stage because `.dockerignore` excluded
  `scripts/bench/scifact-ndcg.sh`.
- [x] `.dockerignore` now re-includes only the K8s SciFact gate script
  needed by the bench-driver image.
- [x] `docker-build` run `26064128510` published both runtime and
  bench-driver images for `6a493e0`.
- [x] `ci-k8s` run `26064198159` proved both GHCR images are pullable
  by K8s, the Pod reaches Running, and `ndcg-driver` reaches benchmark
  execution then exits `0` after reaching the report-write path.
- [x] The remaining run `26064198159` blocker is Job completion
  semantics: regular engine sidecars kept the Pod running after the
  driver finished, so the Job timed out at 30 min.
- [x] `scifact-ndcg.sh` now writes generated bulk NDJSON to a temporary
  file when the BEIR corpus mount is read-only and fails closed on
  shell, `jq`, or `curl` errors.
- [x] `ndcg-gate` and `insee-bench` use the bench driver image for
  shell/scripts/tools; the Surch sidecar keeps the runtime image.
- [x] `ndcg-gate` and `insee-bench` now declare Surch and the reference
  engine as restartable init-container sidecars, so the Job can complete
  when the benchmark driver exits.
- [x] The reference engine sidecar now has a per-container `1000:1000`
  security context.
- [x] `make bench-k8s` prints the runtime and bench driver tags before
  dispatch.
- [x] Verification run recorded: `ci-k8s` `26038117579` failed in 16s
  on missing image, replacing 30m timeout pattern.
- [x] `main` CI after latest integration was green: `26038398172`.
- [ ] `ci-k8s` heavy run reports Job `Complete=True`.
  - [x] Diagnose image contract mismatch between GHCR preflight and
    rendered Job manifests.
  - [x] Diagnose next runtime blockers: Surch distroless driver image
    has no `/bin/sh`; reference engine entrypoint exits under the
    pod-level `65532:65532` security context.
  - [x] Prove `ndcg-driver` benchmark execution with run `26064198159`.
  - [x] Diagnose sidecar completion blocker from run `26064198159`.
  - [ ] Prove restartable sidecar manifests on GitHub Actions.

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
  - [x] Align rendered Job images with the same `sha-<full commit SHA>`
    tag convention.
  - [x] Make `make bench-k8s` print the exact image tag it expects.
  - [x] If the tag is missing, print the exact remediation command:
    trigger `docker-build.yml` for the same ref, then rerun `ci-k8s`.
  - [x] Keep the missing-image preflight fail-fast.
  - [x] Commit main: `5c25463`.
  - [x] Gate 1: missing image fails fast with actionable message.
  - [x] Gate 2a: existing image reaches pod startup instead of image
    pull failure; run `26058595173`.
  - [x] Gate 2b: existing image reaches benchmark execution after the
    driver/security-context runtime fixes.
  - [ ] Gate 2c: restartable sidecar manifests allow Job
    `Complete=True` after driver exit.

- [ ] Lot 3 - Heavy-run standardisation
  - [x] Keep Docker builder MSRV aligned with locked dependencies before
    dispatching `ci-k8s`.
  - [x] Preserve diagnostics and artefacts on failure; run
    `26058595173` uploaded
    `k8s-bench-ndcg-gate-236980c600a60c40a8f28e2c433558c59ec5d5f7`.
  - [x] Make the wait loop fail early on terminal pod/container states.
  - [x] Provide a shell-capable benchmark driver image/stage for
    `ndcg-gate` and `insee-bench`.
  - [x] Fix the Docker build context so the bench-driver stage can copy
    `scripts/bench/scifact-ndcg.sh`; first failing run:
    `26063701483`.
  - [x] Move the reference engine sidecar to a compatible per-container
    security context.
  - [x] Move engine sidecars to restartable init containers so Jobs can
    complete after the benchmark driver exits.
  - [x] Make the SciFact driver compatible with read-only BEIR PVCs and
    fail closed on HTTP/script errors.
  - [x] Make `bench-k8s` print the runtime and bench driver image
    contracts before dispatch.
  - [ ] Make `ci-k8s` the standard burst / large-corpus path.
  - [x] Turn `make bench-k8s` into a real entry point.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Record run ids and artefact paths.
