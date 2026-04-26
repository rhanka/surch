# Branch: BR-07 - API Search Compat And Integration

## Objective
- Deliver `_search` compatibility for the MVP DSL and integrate API behavior with search-core semantics.

## Scope / Guardrails
- Search API only
- Use BR-05 search semantics rather than reinterpreting query rules in the API layer
- Keep analytics out of scope

## Spec Sources
- Required:
  - `spec/SPEC_OS_SEARCH_AND_QUERY_DSL.md`
  - `spec/SPEC_SECURITY_AND_TESTING_BASELINE.md`

## Allowed Paths
- `surch-api/src/**`
- `surch-core/src/common/**`
- `tests/integration/api_compat/search_api.rs`
- `plan/07_BRANCH_api-search-compat-and-integration.md`

## Forbidden Paths
- `rules/**`

## Conditional Paths
- `surch-core/src/search/**` only if API wiring reveals a contract mismatch and it is recorded

## Dependency Gates
- [ ] BR-02 reviewed
- [ ] BR-05 and BR-06 merged or otherwise available in isolated integration branch

## Environment Mapping
- Worktree: `tmp/br-07-api-search`
- Data path: `data/test/br-07/`
- API port: `9207`
- Mode: local Rust + isolated data

## Plan / Lots / Todo

- [ ] **Lot 0 - Search request contract read**
  - [ ] Confirm request body grammar and response shape from spec
  - [ ] Confirm in-scope query families for MVP

- [ ] **Lot 1 - Request parsing and execution wiring**
  - [ ] Normalize `_search` request parsing
  - [ ] Route parsed queries to search-core contract
  - [ ] Lot gate:
    - [ ] `cargo test --workspace search`

- [ ] **Lot 2 - Response compatibility**
  - [ ] Normalize `hits.total`, `hits.hits`, `_score`, and `_source`
  - [ ] Add support for `from`, `size`, and basic `sort`
  - [ ] Lot gate:
    - [ ] `cargo test --workspace search_api`

- [ ] **Lot 3 - Integration tests**
  - [ ] Add `tests/integration/api_compat/search_api.rs`
  - [ ] Cover valid and invalid query cases
  - [ ] Final gate:
    - [ ] `cargo fmt --all`
    - [ ] `cargo clippy --workspace --all-targets --all-features`
    - [ ] `cargo test --workspace`

## Feedback Loop
- Raise `spec-mismatch` if API response shape and search-core output drift apart

## Tests Required
- Integration: `_search` request/response compatibility, pagination, sorting, fuzzy requests

## Security Checks
- [ ] Query depth and expensive-pattern limits reviewed at API boundary

## Merge Checklist
- [ ] `_search` aligns with spec for MVP clauses
- [ ] Integration tests pass
- [ ] Search API remains analytics-free
