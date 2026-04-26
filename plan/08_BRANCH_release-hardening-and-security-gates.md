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
- [ ] BR-03 complete
- [ ] BR-04 complete
- [ ] BR-05 complete
- [ ] BR-06 complete
- [ ] BR-07 complete

## Environment Mapping
- Worktree: `tmp/br-08-release`
- Data path: `data/test/br-08/`
- API port: `9208`
- Mode: isolated validation, containerized e2e allowed if needed

## Plan / Lots / Todo

- [ ] **Lot 0 - Candidate assembly**
  - [ ] Confirm merged feature set
  - [ ] Confirm release checklist inputs

- [ ] **Lot 1 - Full verification**
  - [ ] `cargo fmt --all`
  - [ ] `cargo clippy --workspace --all-targets --all-features`
  - [ ] `cargo test --workspace`
  - [ ] Run compatibility smoke against documented MVP endpoints

- [ ] **Lot 2 - Security review**
  - [ ] Review input validation coverage
  - [ ] Review wildcard and regex limits
  - [ ] Run `cargo audit` if available
  - [ ] Record unresolved risk items explicitly

- [ ] **Lot 3 - Release closeout**
  - [ ] Update `CHANGELOG.md`
  - [ ] Confirm rollback path
  - [ ] Confirm release branch ready for merge

## Feedback Loop
- Use `security-alert` for any release-blocking gap
- Use `attention` for any deferred non-MVP issue

## Tests Required
- Full workspace tests
- Compatibility smoke tests
- Release-critical regression tests

## Security Checks
- [ ] No unresolved high-risk MVP issue without documented decision

## Merge Checklist
- [ ] Full verification complete
- [ ] Security gate complete
- [ ] Changelog updated
- [ ] Release candidate approved
