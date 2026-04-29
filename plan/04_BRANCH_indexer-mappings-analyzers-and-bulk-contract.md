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
- `surch-core/tests/**`
- `plan/04_BRANCH_indexer-mappings-analyzers-and-bulk-contract.md`

## Forbidden Paths
- `surch-core/src/search/**`
- `surch-api/**`
- `rules/**`

## Conditional Paths
- `surch-core/src/storage/**` only if BR-03 exposes a necessary contract mismatch and it is recorded

## Dependency Gates
- [x] BR-01 reviewed
- [x] BR-03 storage contract reviewed

## Environment Mapping
- Worktree: `tmp/br-04-indexer`
- Data path: `data/test/br-04/`
- Mode: local Rust + isolated data

## Plan / Lots / Todo

- [x] **Lot 0 - Contract read**
  - [x] Review current mapping and analyzer files
  - [x] Confirm MVP field types and analyzer list

- [x] **Lot 1 - Mapping validation**
  - [x] Tighten mapping rules for supported field types
  - [x] Reject obviously invalid field definitions
  - [x] Add unit tests for valid and invalid mapping cases
  - [x] Lot gate:
    - [x] `cargo test -p surch-core indexer::mapping`

- [x] **Lot 2 - Analyzer behavior**
  - [x] Normalize analyzer outputs for standard, simple, stop, keyword
  - [x] Add unit tests for token output and edge cases
  - [x] Lot gate:
    - [x] `cargo test -p surch-core indexer::analyzer`

- [x] **Lot 3 - Bulk ingestion contract**
  - [x] Model the parsing expectations required by bulk ingestion
  - [x] Add `surch-core/tests/indexer_bulk_contract.rs`
  - [x] Final gate:
    - [x] `cargo fmt --all`
    - [x] `cargo clippy --workspace --all-targets --all-features`
    - [x] `cargo test -p surch-core`

## Feedback Loop
- Raise `spec-mismatch` if mapping or bulk semantics differ from harvested spec
- attention: branch-local verification is green on `surch-core`; workspace-level clippy is now green after release-hardening cleanup.

## Tests Required
- Unit: mapping validation, analyzer tokenization
- Integration: `surch-core/tests/indexer_bulk_contract.rs`

## Security Checks
- [x] Invalid field definitions rejected cleanly
- [x] Bulk payload parsing risks documented

## Merge Checklist
- [x] Mapping validation works
- [x] Analyzer tests pass
- [x] Bulk contract is explicit
