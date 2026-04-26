# Branch: BR-02 - Spec Harvest Search And Query DSL

## Objective
- Confirm exact MVP Query DSL shape and search request semantics from OpenSearch references.

## Scope / Guardrails
- Planning and spec work only, no implementation
- Focus on grammar, defaults, validation, and fuzzy semantics

## Spec Sources
- Required:
  - `spec/SPEC_OS_SEARCH_AND_QUERY_DSL.md`
  - `spec/SPEC_EVOL_SURCH_GOVERNANCE_ORCHESTRATION.md`

## Allowed Paths
- `spec/SPEC_OS_SEARCH_AND_QUERY_DSL.md`
- `plan/02_BRANCH_spec-harvest-search-and-query-dsl.md`

## Forbidden Paths
- `surch-core/**`
- `surch-api/**`
- `rules/**`

## Conditional Paths
- `PLAN.md` only if conductor updates branch status

## Dependency Gates
- [x] Governance spec reviewed
- [x] Search and Query DSL reference set verified

## Environment Mapping
- Worktree: `tmp/br-02-spec-harvest`
- Mode: local docs only

## Plan / Lots / Todo

- [x] **Lot 0 - Source capture**
  - [x] Confirm search API source docs
  - [x] Confirm clause-level docs for MVP DSL

- [x] **Lot 1 - DSL contract**
  - [x] Normalize JSON grammar for MVP clauses
  - [x] Record required fields, defaults, and validation rules

- [x] **Lot 2 - Fuzzy focus**
  - [x] Capture fuzziness, transpositions, prefix length, and max expansions rules
  - [x] Mark what Surch must support exactly in MVP

- [x] **Lot 3 - Test inventory**
  - [x] List valid and invalid query cases needed for implementation and compatibility tests

## Feedback Loop
- `clarification`: exact numeric ceilings for pagination, bool nesting, wildcard cost, and total-hit accounting remain undefined in branch-local scope.
- `clarification`: `case_insensitive: true` is accepted in grammar for `term`, `prefix`, and `wildcard`, but consuming branches must either implement it or reject it explicitly.
- `decision`: `regexp` is now marked out of MVP and must fail with explicit unsupported-clause behavior.
- `attention`: baseline `cargo test` remains blocked by missing system OpenSSL for `openssl-sys`; environment only, not branch scope.

## Tests Required
- Compatibility scenarios listed in spec, no code tests in this branch

## Security Checks
- [x] Expensive query controls called out in the spec

## Merge Checklist
- [x] DSL grammar complete
- [x] Fuzzy semantics complete
- [x] Validation cases captured

## Branch Status
- Status: ready for downstream implementation branches with one governance follow-up on numeric cost ceilings
- Implementation handoff: spec now defines accepted shapes, explicit defaults, unsupported clauses, and required negative cases
