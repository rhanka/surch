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

- [ ] **Lot 0 - Handler contract read**
  - [x] Review current `surch-api/src/main.rs`
  - [x] Confirm in-scope endpoints and response fields

- [ ] **Lot 1 - Index management endpoints**
  - [ ] Normalize create index, delete index, get index, get mapping
  - [ ] Add handler-level tests where possible
  - [ ] Lot gate:
    - [ ] `cargo test --workspace index`

- [ ] **Lot 2 - Document endpoints**
  - [ ] Normalize index document, get document, delete document
  - [ ] Normalize response fields such as `_index`, `_id`, `_version`, `_seq_no`, `_primary_term`, `_shards`
  - [ ] Lot gate:
    - [ ] `cargo test --workspace document`

- [ ] **Lot 3 - Bulk and maintenance endpoints**
  - [ ] Implement or stub correctly scoped bulk, refresh, flush responses for MVP
  - [ ] Add `tests/integration/api_compat/index_document_api.rs`
  - [ ] Final gate:
    - [ ] `cargo fmt --all`
    - [ ] `cargo clippy --workspace --all-targets --all-features`
    - [ ] `cargo test --workspace`

## Feedback Loop
- Raise `spec-mismatch` for any response-shape discrepancy
- attention: baseline `cargo test -p surch-api` is now green after switching `reqwest` dev-dependency to `rustls`; OpenSSL system dependency is no longer the blocker for BR-06.

## Tests Required
- Integration: create/delete/get index, mapping, document CRUD, bulk shape, refresh, flush

## Security Checks
- [ ] Invalid JSON and malformed bulk payload behavior reviewed
- [ ] Body limits and payload validation considered

## Merge Checklist
- [ ] Index/document endpoints align with spec
- [ ] Response underscore fields are correct
- [ ] Compatibility tests pass
