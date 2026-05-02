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
- [x] BR-08 technical hardening complete
- [x] MatchID reference corpus source identified
- [ ] Elasticsearch baseline source identified

## Environment Mapping
- Worktree: `tmp/br-09-matchid-parity`
- Mode: validation harness and corpus tooling

## Plan / Lots / Todo

- [ ] **Lot 0 - Corpus contract**
  - [x] Define request corpus format
  - [x] Define normalized response comparison format
  - [x] Freeze representative seed corpus source from `clients_test.csv`

- [ ] **Lot 1 - Comparator harness**
  - [x] Implement Elasticsearch vs Surch replay harness
  - [x] Record diff categories

- [ ] **Lot 2 - Zero-gap evidence**
  - [ ] Run corpus replay
  - [ ] Produce pass/fail summary

## Feedback Loop
- Block on any ambiguity around what counts as a zero-gap comparison field.
- attention: the harness is testable locally with fixtures, but the real MatchID-derived frozen corpus is still missing.
- attention: final BR-09 completion requires actual Elasticsearch captures from the MatchID context, not only synthetic sample fixtures.
- attention: `tests/matchid_parity/matchid_request_seed.jsonl` is a representative seed derived from MatchID test data, not yet the final golden zero-gap corpus.

## Merge Checklist
- [ ] Golden corpus frozen
- [x] Comparator reproducible
- [ ] Zero-gap evidence produced
