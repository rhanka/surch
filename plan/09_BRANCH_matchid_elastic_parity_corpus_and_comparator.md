# Branch: BR-09 - MatchID Elastic Parity Corpus And Comparator

## Objective
- Build the correctness harness that proves zero search gap between Elasticsearch and Surch in the MatchID usage context.

## Scope / Guardrails
- This branch is about evidence, not feature invention.
- Compare Elasticsearch behavior as used by MatchID, not MatchID UI or unrelated workflows.
- Freeze corpus and normalization rules before comparing outputs.

## Spec Sources
- Required:
  - `spec/SPEC_MATCHID_ELASTIC_PARITY_EXIT_CRITERIA.md`
  - `spec/SPEC_OS_SEARCH_AND_QUERY_DSL.md`
  - `spec/SPEC_OS_INDEX_AND_DOCUMENT_APIS.md`

## Allowed Paths
- `spec/**`
- `tests/**`
- `scripts/**`
- `plan/09_BRANCH_matchid_elastic_parity_corpus_and_comparator.md`

## Forbidden Paths
- unrelated API or core feature work
- `rules/**`

## Dependency Gates
- [ ] BR-08 technical hardening complete
- [ ] MatchID reference corpus source identified
- [ ] Elasticsearch baseline source identified

## Environment Mapping
- Worktree: `tmp/br-09-matchid-parity`
- Mode: validation harness and corpus tooling

## Plan / Lots / Todo

- [ ] **Lot 0 - Corpus contract**
  - [ ] Define request corpus format
  - [ ] Define normalized response comparison format
  - [ ] Freeze golden corpus source

- [ ] **Lot 1 - Comparator harness**
  - [ ] Implement Elasticsearch vs Surch replay harness
  - [ ] Record diff categories

- [ ] **Lot 2 - Zero-gap evidence**
  - [ ] Run corpus replay
  - [ ] Produce pass/fail summary

## Feedback Loop
- Block on any ambiguity around what counts as a zero-gap comparison field.

## Merge Checklist
- [ ] Golden corpus frozen
- [ ] Comparator reproducible
- [ ] Zero-gap evidence produced
