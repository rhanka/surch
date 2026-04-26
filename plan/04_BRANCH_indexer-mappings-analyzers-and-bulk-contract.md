# Branch: BR-04 - Indexer Mappings Analyzers And Bulk Contract

## Objective
- Deliver mapping validation, analyzer behavior, and the indexing-side contract needed for single and bulk document ingestion.

## Scope / Guardrails
- Indexing behavior only
- Do not implement search ranking here
- Keep field types limited to MVP list

## Spec Sources
- Required:
  - `spec/SPEC_OS_INDEX_AND_DOCUMENT_APIS.md`
  - `spec/SPEC_SECURITY_AND_TESTING_BASELINE.md`

## Allowed Paths
- `surch-core/src/indexer/**`
- `surch-core/src/common/**`
- `tests/integration/indexer/**`
- `plan/04_BRANCH_indexer-mappings-analyzers-and-bulk-contract.md`

## Forbidden Paths
- `surch-core/src/search/**`
- `surch-api/**`
- `rules/**`

## Conditional Paths
- `surch-core/src/storage/**` only if BR-03 exposes a necessary contract mismatch and it is recorded

## Dependency Gates
- [ ] BR-01 reviewed
- [ ] BR-03 storage contract reviewed

## Environment Mapping
- Worktree: `tmp/br-04-indexer`
- Data path: `data/test/br-04/`
- Mode: local Rust + isolated data

## Plan / Lots / Todo

- [ ] **Lot 0 - Contract read**
  - [ ] Review current mapping and analyzer files
  - [ ] Confirm MVP field types and analyzer list

- [ ] **Lot 1 - Mapping validation**
  - [ ] Tighten mapping rules for supported field types
  - [ ] Reject obviously invalid field definitions
  - [ ] Add unit tests for valid and invalid mapping cases
  - [ ] Lot gate:
    - [ ] `cargo test -p surch-core indexer::mapping`

- [ ] **Lot 2 - Analyzer behavior**
  - [ ] Normalize analyzer outputs for standard, simple, stop, keyword
  - [ ] Add unit tests for token output and edge cases
  - [ ] Lot gate:
    - [ ] `cargo test -p surch-core indexer::analyzer`

- [ ] **Lot 3 - Bulk ingestion contract**
  - [ ] Model the parsing expectations required by bulk ingestion
  - [ ] Add `tests/integration/indexer/bulk_contract.rs`
  - [ ] Final gate:
    - [ ] `cargo fmt --all`
    - [ ] `cargo clippy --workspace --all-targets --all-features`
    - [ ] `cargo test --workspace`

## Feedback Loop
- Raise `spec-mismatch` if mapping or bulk semantics differ from harvested spec

## Tests Required
- Unit: mapping validation, analyzer tokenization
- Integration: bulk contract and single-document ingestion shape

## Security Checks
- [ ] Invalid field definitions rejected cleanly
- [ ] Bulk payload parsing risks documented

## Merge Checklist
- [ ] Mapping validation works
- [ ] Analyzer tests pass
- [ ] Bulk contract is explicit
