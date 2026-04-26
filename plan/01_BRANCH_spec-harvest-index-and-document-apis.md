# Branch: BR-01 - Spec Harvest Index And Document APIs

## Objective
- Confirm exact OpenSearch-compatible syntax and response contracts for index management and document APIs used by the MVP.

## Scope / Guardrails
- Planning and spec work only, no engine implementation
- Focus on syntax, validation, error shape, and compatibility traps
- All findings must be written under `spec/`

## Spec Sources
- Required:
  - `spec/SPEC_OS_INDEX_AND_DOCUMENT_APIS.md`
  - `spec/SPEC_EVOL_SURCH_GOVERNANCE_ORCHESTRATION.md`

## Allowed Paths
- `spec/SPEC_OS_INDEX_AND_DOCUMENT_APIS.md`
- `plan/01_BRANCH_spec-harvest-index-and-document-apis.md`

## Forbidden Paths
- `surch-core/**`
- `surch-api/**`
- `rules/**`
- unrelated branch files

## Conditional Paths
- `PLAN.md` only if branch status or dependency wording must be updated by conductor

## Dependency Gates
- [ ] Governance spec reviewed
- [ ] OpenSearch source links verified

## Environment Mapping
- Worktree: `tmp/br-01-spec-harvest`
- Mode: local docs only

## Plan / Lots / Todo

- [ ] **Lot 0 - Source capture**
  - [ ] Confirm source docs for create index, delete index, get index, get mapping
  - [ ] Confirm source docs for index document, get document, delete document, bulk, refresh, flush

- [ ] **Lot 1 - Compatibility matrix**
  - [ ] Normalize endpoints, methods, required params, body shapes, and response fields
  - [ ] Record error codes and compatibility traps

- [ ] **Lot 2 - Test inventory**
  - [ ] Write Given/When/Then compatibility cases for success and error paths
  - [ ] Mark MVP `MUST`, `SHOULD`, or `LATER`

- [ ] **Lot 3 - Final validation**
  - [ ] Spec reviewed for ambiguity and contradictions

## Feedback Loop
- Record any undocumented OpenSearch divergence as `spec-mismatch`

## Tests Required
- Compatibility scenarios listed in spec, no code tests in this branch

## Security Checks
- [ ] Body size and malformed NDJSON concerns captured for bulk

## Merge Checklist
- [ ] Syntax matrix complete
- [ ] Error shape notes complete
- [ ] MVP endpoint priorities marked
