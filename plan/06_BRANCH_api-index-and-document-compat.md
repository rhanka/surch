# Branch: BR-06 - API Index And Document Compat

## Objective
- Deliver MVP-compatible index and document endpoints with correct payload parsing and response shape.

## Scope / Guardrails
- API surface limited to index and document operations
- Do not add search endpoint logic here
- Response field names must match harvested spec exactly where in scope

## Spec Sources
- Required:
  - `spec/SPEC_OS_INDEX_AND_DOCUMENT_APIS.md`
  - `spec/SPEC_SECURITY_AND_TESTING_BASELINE.md`

## Allowed Paths
- `surch-api/src/**`
- `surch-core/src/common/**`
- `tests/integration/api_compat/index_document_api.rs`
- `plan/06_BRANCH_api-index-and-document-compat.md`

## Forbidden Paths
- `surch-core/src/search/**`
- `rules/**`

## Conditional Paths
- `surch-core/src/storage/**` only if API integration reveals a contract mismatch and it is recorded
- `surch-core/src/indexer/**` only if payload-to-document conversion requires a shared fix and it is recorded

## Dependency Gates
- [x] BR-01 reviewed
- [x] BR-03 and BR-04 contracts reviewed

## Environment Mapping
- Worktree: `tmp/br-06-api-index-doc`
- Data path: `data/test/br-06/`
- API port: `9206`
- Mode: local Rust + isolated data

## Plan / Lots / Todo

- [x] **Lot 0 - Handler contract read**
  - [x] Review current `surch-api/src/main.rs`
  - [x] Confirm in-scope endpoints and response fields

- [x] **Lot 1 - Index management endpoints**
  - [x] Normalize create index, delete index, get index, get mapping
  - [x] Add handler-level tests where possible
  - [x] Lot gate:
    - [x] `cargo test -p surch-api create_index_returns_acknowledged_shape`
    - [x] `cargo test -p surch-api delete_index_returns_acknowledged_shape`
    - [x] `cargo test -p surch-api get_index_returns_index_keyed_settings_and_mappings`
    - [x] `cargo test -p surch-api get_mapping_returns_index_keyed_mappings`

- [x] **Lot 2 - Document endpoints**
  - [x] Normalize index document, get document, delete document
  - [x] Normalize response fields such as `_index`, `_id`, `_version`, `_seq_no`, `_primary_term`, `_shards`
  - [x] Lot gate:
    - [x] `cargo test -p surch-api index_document_returns_expected_metadata`
    - [x] `cargo test -p surch-api get_document_returns_found_false_for_missing_doc`
    - [x] `cargo test -p surch-api delete_document_returns_not_found_for_missing_doc`
    - [x] `cargo test -p surch-api delete_document_removes_existing_doc`

- [x] **Lot 3 - Bulk and maintenance endpoints**
  - [x] Implement or stub correctly scoped bulk, refresh, flush responses for MVP
  - [x] Add handler-level contract tests in `surch-api/src/main.rs`
  - [x] Final gate:
    - [x] `cargo fmt --all`
    - [x] `cargo clippy --workspace --all-targets --all-features`
    - [x] `cargo test -p surch-api`

## Feedback Loop
- Raise `spec-mismatch` for any response-shape discrepancy
- attention: baseline `cargo test -p surch-api` is now green after switching `reqwest` dev-dependency to `rustls`; OpenSSL system dependency is no longer the blocker for BR-06.
- attention: BR-06 Lot 1 verification is green with handler-level tests in `surch-api/src/main.rs`; broader API cleanup is still pending.
- attention: BR-06 used the conditional `surch-core/src/storage/**` scope to add persisted document deletion semantics required by the API contract.
- attention: branch-local verification is green on `surch-api`; workspace-level clippy is now green after release-hardening cleanup.

## Tests Required
- Integration: create/delete/get index, mapping, document CRUD, bulk shape, refresh, flush

## Security Checks
- [x] Invalid JSON and malformed bulk payload behavior reviewed
- [x] Body limits and payload validation considered for Lot 1 and Lot 2 handler paths

## Merge Checklist
- [x] Index/document endpoints align with spec for BR-06 scope
- [x] Response underscore fields are correct for current index/document endpoint scope
- [x] Compatibility tests pass
