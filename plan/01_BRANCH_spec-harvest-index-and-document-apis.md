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
- [x] Governance spec reviewed
- [x] OpenSearch source links verified

## Environment Mapping
- Worktree: `tmp/br-01-spec-harvest`
- Mode: local docs only

## Plan / Lots / Todo

- [x] **Lot 0 - Source capture**
  - [x] Confirm source docs for create index, delete index, get index, get mapping
  - [x] Confirm source docs for index document, get document, delete document, bulk, refresh, flush

- [x] **Lot 1 - Compatibility matrix**
  - [x] Normalize endpoints, methods, required params, body shapes, and response fields
  - [x] Record error codes and compatibility traps

- [x] **Lot 2 - Test inventory**
  - [x] Write Given/When/Then compatibility cases for success and error paths
  - [x] Mark MVP `MUST`, `SHOULD`, or `LATER`

- [x] **Lot 3 - Final validation**
  - [x] Spec reviewed for ambiguity and contradictions

## Feedback Loop
- `attention`: `Flush` remains explicitly `LATER` for MVP and must not block BR-03, BR-04, or BR-06.
- `attention`: unsupported-but-known syntax is normalized to explicit `400` rejection rather than silent acceptance.
- `attention`: baseline `cargo test` in this worktree is still blocked by missing system OpenSSL for `openssl-sys`; environment issue only, outside branch scope.
- Record any undocumented OpenSearch divergence as `spec-mismatch`.

## Tests Required
- Compatibility scenarios listed in spec, no code tests in this branch

## Security Checks
- [x] Body size and malformed NDJSON concerns captured for bulk

## Merge Checklist
- [x] Syntax matrix complete
- [x] Error shape notes complete
- [x] MVP endpoint priorities marked
