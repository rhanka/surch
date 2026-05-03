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
- [x] Elasticsearch baseline source identified

## Environment Mapping
- Worktree: `tmp/br-09-matchid-parity`
- Mode: validation harness and corpus tooling

## Plan / Lots / Todo

- [ ] **Lot 0 - Corpus contract**
  - [x] Define request corpus format
  - [x] Define normalized response comparison format
  - [x] Freeze representative seed corpus source from `clients_test.csv`
  - [x] Freeze mini dataset baseline `deces-2020-m01.txt.gz` with manifest and record count

- [ ] **Lot 1 - Comparator harness**
  - [x] Implement Elasticsearch vs Surch replay harness
  - [x] Record diff categories
  - [x] Compile MatchID request seed into a first canonical `/deces/_search` corpus

- [ ] **Lot 2 - Zero-gap evidence**
  - [ ] Run corpus replay
  - [ ] Produce pass/fail summary

## Feedback Loop
- Block on any ambiguity around what counts as a zero-gap comparison field.
- attention: the harness is testable locally with fixtures, but the real MatchID-derived frozen corpus is still missing.
- attention: final BR-09 completion requires actual Elasticsearch captures from the MatchID context, not only synthetic sample fixtures.
- attention: `tests/matchid_parity/matchid_request_seed.jsonl` is a representative seed derived from MatchID test data, not yet the final golden zero-gap corpus.
- attention: `tests/matchid_parity/matchid_deces_corpus.jsonl` is a first compiled canonical corpus for the simple non-fuzzy MatchID profile; it still needs real Elasticsearch captures.
- attention: `dev-deces.matchid.io` is reachable and can return real backend captures for the seed corpus.
- attention: local MatchID backend can be started, but the local Elasticsearch `deces` shard is currently red and snapshot restore failed because the repository endpoint timed out.
- attention: `tests/matchid_parity/matchid_positive_seed.jsonl` and `tests/matchid_parity/matchid_positive_deces_corpus.jsonl` freeze a first positive corpus derived from real hits returned by `dev-deces`.
- decision: the first 6 positive cases in `matchid_positive_seed.jsonl` were manually validated against `dev-deces.matchid.io` and return `total=1` in both GET and POST search modes.
- decision: `tests/matchid_parity/dev_deces_positive_capture.jsonl` is now the first persisted real backend baseline capture for those positive cases.
- decision: `tests/matchid_parity/dev_deces_positive_capture_normalized.jsonl` is normalized into the generic comparator format, so BR-09 now has a real backend baseline that can be diffed with Surch.
- decision: `tests/matchid_parity/matchid_2020m01_manifest.json` is the frozen mini dataset descriptor for the first real Surch parity ingestion loop.
- attention: a local dataprep-backend path is partially prepared through `scripts/matchid_prepare_local_dataprep_project.py`, but the first filesystem/csv run currently fails inside dataprep-backend (`Dataset.before` / `Dataset.reader` path) and is not yet a reliable evidence source.

## Merge Checklist
- [ ] Golden corpus frozen
- [x] Comparator reproducible
- [ ] Zero-gap evidence produced
