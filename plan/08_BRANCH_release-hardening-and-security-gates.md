# Branch: BR-08 - Release Hardening And Security Gates

## Objective
- Stabilize the MVP release candidate, close compatibility gaps, and verify security and test gates before merge to `main`.

## Scope / Guardrails
- No new product scope
- Fixes only, based on evidence from previous branches
- Release branch is the only path to `main`

## Spec Sources
- Required:
  - `spec/SPEC_OS_INDEX_AND_DOCUMENT_APIS.md`
  - `spec/SPEC_OS_SEARCH_AND_QUERY_DSL.md`
  - `spec/SPEC_SECURITY_AND_TESTING_BASELINE.md`

## Allowed Paths
- `surch-core/**`
- `surch-api/**`
- `tests/integration/**`
- `CHANGELOG.md`
- `plan/08_BRANCH_release-hardening-and-security-gates.md`

## Forbidden Paths
- `rules/**`
- unrelated planning branches

## Conditional Paths
- `spec/**` only if release verification uncovers a real documentation error

## Dependency Gates
- [x] BR-03 complete
- [x] BR-04 complete
- [x] BR-05 complete
- [x] BR-06 complete
- [x] BR-07 complete
- [ ] BR-09 complete
- [ ] BR-10 complete

## Environment Mapping
- Worktree: `tmp/br-08-release`
- Data path: `data/test/br-08/`
- API port: `9208`
- Mode: isolated validation, containerized e2e allowed if needed

## Plan / Lots / Todo

- [x] **Lot 0 - Candidate assembly**
  - [x] Confirm merged feature set
  - [x] Confirm release checklist inputs

- [ ] **Lot 1 - Full verification**
  - [x] `cargo fmt --all`
  - [x] `cargo clippy --workspace --all-targets --all-features`
  - [x] `cargo test --workspace`
  - [x] Run compatibility smoke against documented MVP endpoints via handler-level API tests and search/storage integration coverage

- [ ] **Lot 2 - Security review**
  - [x] Review input validation coverage
  - [x] Review wildcard and regex limits
  - [x] Run `cargo audit` if available
  - [x] Record unresolved risk items explicitly

- [ ] **Lot 3 - Release closeout**
  - [x] Update `CHANGELOG.md`
  - [x] Confirm rollback path
  - [ ] Confirm release branch ready for merge

## Feedback Loop
- Use `security-alert` for any release-blocking gap
- Use `attention` for any deferred non-MVP issue
- attention: release verification is green for fmt, clippy, and workspace tests; remaining open work is final security review, changelog sync, and explicit release sign-off.
- attention: `cargo audit` is now installed and clean on the current dependency graph.
- decision: rollback path is commit-based. Before release approval, tag the release candidate SHA. If post-release rollback is needed, create a rollback fix branch from `main` or release branch, revert the release commits in reverse order, rerun workspace verification, and publish a hotfix release.
- blocked: final release sign-off still requires BR-09 zero-gap evidence and BR-10 performance parity evidence in the MatchID Elastic context.

## Tests Required
- Full workspace tests
- Compatibility smoke tests
- Release-critical regression tests

## Security Checks
- [x] No unresolved high-risk MVP issue without documented decision

## Merge Checklist
- [x] Full verification complete
- [x] Security gate complete
- [x] Changelog updated
- [ ] Release candidate approved
