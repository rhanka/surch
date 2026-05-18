# wp/c-ops Plan

Track principal: C - ops / packaging / snapshots
Branch: `wp/c-ops`
Worktree: `.worktrees/wp-c`
Owner: conductor / APIServer / StorageEngine depending on slice
Status: active branch exists; latest branch head `8d0ba97`

## Finality

- [ ] Make release and snapshot paths verifiable end to end.

## Scope

- [x] Delivered docs scope:
  `docs/ops/snapshot-plan.md`, `docs/ops/packaging-plan.md`,
  `docs/ops/workpackages.md`.
- [ ] Next functional scope: snapshot REST, S3/MinIO e2e, restore, SLM
  retention, release verification.
- [x] Evidence source: release workflow, Helm chart, snapshot/SLM API,
  `ci`, `ci-k8s`, and verification scripts.

## Merge State

- [x] Snapshot/packaging docs refreshed on `main`: `0a4ca02`.
- [x] Workpackage SHAs refreshed on `main`: `b14ca94`.
- [x] SLM policy API exists on `main`.
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

- [ ] Lot 2 - S3/MinIO e2e snapshot path
  - [ ] Select MinIO local or CI execution mode.
  - [ ] Create repository, snapshot, restore, and verify documents.
  - [ ] Record exact commands and artefacts.
  - [ ] Gate: reproducible e2e run.

- [ ] Lot 3 - Snapshot / SLM completeness
  - [ ] Finish snapshot REST coverage.
  - [ ] Finish restore coverage.
  - [ ] Finish SLM retention behavior.
  - [ ] Preserve diagnostics for failure cases.

- [ ] Lot 4 - Release verification
  - [ ] Reproduce release verification from CI artefacts.
  - [ ] Record signing/SBOM verification commands and outputs.
  - [ ] Keep failing-run inspection path documented.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Push branch/main and record SHA / run ids.
