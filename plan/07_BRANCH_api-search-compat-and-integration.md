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
- `surch-core/src/search/query/**`
- `plan/07_BRANCH_api-search-compat-and-integration.md`

## Forbidden Paths
- `rules/**`

## Conditional Paths
- `surch-core/src/search/**` only if API wiring reveals a contract mismatch and it is recorded

## Dependency Gates
- [x] BR-02 reviewed
- [x] BR-05 and BR-06 merged or otherwise available in isolated integration branch

## Environment Mapping
- Worktree: `tmp/br-07-api-search`
- Data path: `data/test/br-07/`
- API port: `9207`
- Mode: local Rust + isolated data

## Plan / Lots / Todo

- [x] **Lot 0 - Search request contract read**
  - [x] Confirm request body grammar and response shape from spec
  - [x] Confirm in-scope query families for MVP

- [x] **Lot 1 - Request parsing and execution wiring**
  - [x] Normalize `_search` request parsing
  - [x] Route parsed queries to search-core contract
  - [x] Lot gate:
    - [x] `cargo test -p surch-api search_term_query_returns_matching_hit`
    - [x] `cargo test -p surch-api search_match_phrase_respects_slop_zero`
    - [x] `cargo test -p surch-api search_fuzzy_query_matches_transposition`

- [x] **Lot 2 - Response compatibility**
  - [x] Normalize `hits.total`, `hits.hits`, `_score`, `_source`, and `_shards`
  - [x] Add support for `from`, `size`, and basic `sort`
  - [x] Lot gate:
    - [x] `cargo test -p surch-api search_applies_from_size_and_sort`
    - [x] `cargo test -p surch-api search_rejects_regexp_query_for_mvp`

- [x] **Lot 3 - Integration tests**
  - [x] Add handler-level tests in `surch-api/src/main.rs`
  - [x] Cover valid and invalid query cases for term, bool, phrase, prefix, wildcard, multi_match, fuzzy, regexp rejection, and pagination
  - [x] Final gate:
    - [x] `cargo fmt --all`
    - [ ] `cargo clippy --workspace --all-targets --all-features`
    - [x] `cargo test -p surch-api`

## Feedback Loop
- Raise `spec-mismatch` if API response shape and search-core output drift apart
- attention: branch-local verification is green on `surch-api`; workspace-level clippy remains pending because pre-existing warnings remain outside the BR-07 slice.
- decision: `regexp` is explicitly rejected with `400` in MVP.

## Tests Required
- Integration: `_search` request/response compatibility, pagination, sorting, fuzzy requests, and unsupported regexp rejection

## Security Checks
- [x] Query depth and expensive-pattern limits reviewed at contract level for MVP parser scope

## Merge Checklist
- [x] `_search` aligns with spec for MVP clauses in current scope
- [x] Integration tests pass
- [x] Search API remains analytics-free
