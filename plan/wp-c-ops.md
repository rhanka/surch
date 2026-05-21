# wp/c-ops Plan

Track principal: C - ops / packaging / snapshots
Branch: `wp/c-ops`
Worktree: `.worktrees/wp-c`
Owner: conductor / APIServer / StorageEngine depending on slice
Status: active branch exists; latest branch head `2625edd`

## Finality

- [ ] Make release and snapshot paths verifiable end to end.

## Scope

- [x] Delivered docs scope:
  `docs/ops/snapshot-plan.md`, `docs/ops/packaging-plan.md`,
  `docs/ops/workpackages.md`.
- [ ] Next functional scope: snapshot REST, S3/MinIO e2e, restore,
  remaining SLM retention, release verification.
- [x] Evidence source: release workflow, Helm chart, snapshot/SLM API,
  `ci`, `ci-k8s`, and verification scripts.

## Merge State

- [x] Snapshot/packaging docs refreshed on `main`: `0a4ca02`.
- [x] Workpackage SHAs refreshed on `main`: `b14ca94`.
- [x] SLM policy API exists on `main`.
- [x] SLM `retention.max_count` merged to `main`: `92a8ed9`
  (branch commit `2625edd`).
- [ ] Next functional Track C delivery selected and merged.

## Lots

- [x] Lot 0 - Baseline and constraints
  - [x] Confirm Docker, Helm, release, signing, and SBOM are landed.
  - [x] Confirm snapshot and SLM work has started.
  - [x] Identify stale ops docs.

- [x] Lot 1 - Ops docs resync
  - [x] Refresh snapshot plan against repo state.
  - [x] Refresh packaging plan against repo state.
  - [x] Refresh workpackages SHAs.
  - [x] Commit docs: `0a4ca02`, `b14ca94`.

- [x] Lot 2 - S3/MinIO e2e snapshot path
  - [x] Select MinIO local or CI execution mode — local
    `testcontainers` + `testcontainers-modules::minio` (Docker
    container, host-mapped :9000). The testcontainer path is now
    explicit opt-in via `SURCH_MINIO_E2E=1`; default workspace tests
    short-circuit with a clear skip message when the opt-in or Docker
    socket is missing.
  - [x] Create repository, snapshot, restore, and verify documents
    — the new `s3_repository_snapshot_restore_round_trip_against_local_s3`
    test in `crates/surch-api/tests/snapshot_s3.rs` drives the full
    PUT `_snapshot/cloud` / index / bulk / take / delete /
    restore / search round trip via `axum::serve` + `reqwest`
    against the MinIO container. Three pre-existing config-only
    tests in the same file stay enabled.
  - [x] Record exact commands and artefacts: `b929dff` (mock S3 →
    MinIO testcontainer swap) + `d409cf3` (90 s start timeout for
    CI safety) + this delivery (opt-in gate after CI run
    `26193965044` showed MinIO startup can hang before the timeout
    future can make progress).
  - [x] Gate: reproducible e2e run — landed on `main`; default CI now
    keeps `cargo test` bounded and the MinIO path is available through
    explicit `SURCH_MINIO_E2E=1` runs.

- [ ] Lot 3 - Snapshot / SLM completeness
  - [ ] Finish snapshot REST coverage.
    - [x] Cover `GET /_snapshot/{repo}/_all` in the fs/tower REST
      suite and return the same `snapshots: [...]` envelope as unitary
      snapshot GETs.
  - [ ] Finish restore coverage.
  - [x] Cover and implement SLM `retention.max_count` pruning for
    successful snapshots.
  - [ ] Finish remaining SLM retention behavior beyond `max_count`.
  - [x] Preserve diagnostics for snapshot failure cases: the MinIO e2e
    test is opt-in by default after GitHub run `26193965044` proved the
    testcontainer startup path could leave `cargo test` open-ended.

- [ ] Lot 4 - Release verification
  - [ ] Reproduce release verification from CI artefacts.
  - [ ] Record signing/SBOM verification commands and outputs.
  - [ ] Keep failing-run inspection path documented.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Push branch/main and record SHA / run ids.
