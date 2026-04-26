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
- [ ] Governance spec reviewed
- [ ] Search and Query DSL reference set verified

## Environment Mapping
- Worktree: `tmp/br-02-spec-harvest`
- Mode: local docs only

## Plan / Lots / Todo

- [ ] **Lot 0 - Source capture**
  - [ ] Confirm search API source docs
  - [ ] Confirm clause-level docs for MVP DSL

- [ ] **Lot 1 - DSL contract**
  - [ ] Normalize JSON grammar for MVP clauses
  - [ ] Record required fields, defaults, and validation rules

- [ ] **Lot 2 - Fuzzy focus**
  - [ ] Capture fuzziness, transpositions, prefix length, and max expansions rules
  - [ ] Mark what Surch must support exactly in MVP

- [ ] **Lot 3 - Test inventory**
  - [ ] List valid and invalid query cases needed for implementation and compatibility tests

## Feedback Loop
- Record any syntax ambiguity as `clarification` or `spec-mismatch`

## Tests Required
- Compatibility scenarios listed in spec, no code tests in this branch

## Security Checks
- [ ] Expensive query controls called out in the spec

## Merge Checklist
- [ ] DSL grammar complete
- [ ] Fuzzy semantics complete
- [ ] Validation cases captured
