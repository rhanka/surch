# Branch: BR-05 - Search Query Execution And Fuzzy

## Objective
- Deliver executable MVP query semantics, including exact handling for the selected core Query DSL and fuzzy behavior up to distance 2.

## Scope / Guardrails
- Search semantics only
- Do not expand API surface here
- Keep scoring simple but deterministic

## Spec Sources
- Required:
  - `spec/SPEC_OS_SEARCH_AND_QUERY_DSL.md`
  - `spec/SPEC_SECURITY_AND_TESTING_BASELINE.md`

## Allowed Paths
- `surch-core/src/search/**`
- `surch-core/src/common/**`
- `surch-core/tests/**`
- `plan/05_BRANCH_search-query-execution-and-fuzzy.md`

## Forbidden Paths
- `surch-api/**`
- `rules/**`

## Conditional Paths
- `surch-core/src/indexer/**` only if branch contract requires a token or mapping interface change and it is recorded
- `surch-core/src/storage/**` only if a reader contract mismatch is discovered and recorded

## Dependency Gates
- [x] BR-02 reviewed
- [x] BR-03 and BR-04 contracts reviewed

## Environment Mapping
- Worktree: `tmp/br-05-search`
- Data path: `data/test/br-05/`
- Mode: local Rust + isolated data

## Plan / Lots / Todo

- [x] **Lot 0 - Query contract read**
  - [x] Review current query types and fuzzy code
  - [x] Confirm MVP clause list and defaults from spec

- [x] **Lot 1 - Core query execution**
  - [x] Normalize `term`, `terms`, `range`, `exists`, `bool`
  - [x] Add clause-level unit tests
  - [x] Lot gate:
    - [x] `cargo test -p surch-core search::query`

- [ ] **Lot 2 - Full-text and fuzzy**
  - [ ] Normalize `match`, `match_phrase`, `multi_match`, `prefix`, `wildcard`, `regexp`, `fuzzy`
  - [ ] Tighten fuzzy behavior up to distance 2
  - [ ] Add unit tests for fuzzy and invalid expensive-query cases
  - [ ] Lot gate:
    - [ ] `cargo test -p surch-core search::fuzzy`

- [ ] **Lot 3 - Integration semantics**
  - [ ] Add `tests/integration/search/query_dsl_core.rs`
  - [ ] Add `tests/integration/search/fuzzy_compat.rs`
  - [ ] Final gate:
    - [ ] `cargo fmt --all`
    - [ ] `cargo clippy --workspace --all-targets --all-features`
    - [ ] `cargo test --workspace`

## Feedback Loop
- Raise `security-alert` for any unbounded wildcard or regex behavior
- attention: branch-local verification is green on `surch-core`; workspace-level clippy remains pending because the crate still contains pre-existing warnings outside the BR-05 slice.
- decision: `regexp` remains unsupported in MVP and is not part of Lot 1 delivery.

## Tests Required
- Unit: each clause family and fuzzy behavior
- Integration: end-to-end query semantics on indexed documents

## Security Checks
- [x] Expensive query bounds reviewed
- [x] Deep nesting and invalid payload behavior considered at contract level for Lot 1 clauses

## Merge Checklist
- [x] Clause semantics align with spec for `term`, `terms`, `range`, `exists`, and `bool`
- [ ] Fuzzy behavior verified up to distance 2
- [ ] Search integration tests pass
