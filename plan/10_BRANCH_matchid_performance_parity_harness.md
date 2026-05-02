# Branch: BR-10 - MatchID Performance Parity Harness

## Objective
- Build the performance harness that proves Surch is at least as performant as the Elasticsearch baseline for MatchID-relevant workloads.

## Scope / Guardrails
- This branch is about measurement and parity evidence.
- Keep artillery as one layer only; do not rely on a single smoke scenario.

## Spec Sources
- Required:
  - `spec/SPEC_MATCHID_ELASTIC_PARITY_EXIT_CRITERIA.md`
  - MatchID perf references discovered during investigation

## Allowed Paths
- `spec/**`
- `tests/**`
- `scripts/**`
- `plan/10_BRANCH_matchid_performance_parity_harness.md`

## Forbidden Paths
- unrelated API or core feature work
- `rules/**`

## Dependency Gates
- [x] BR-08 technical hardening complete
- [x] MatchID performance baseline source identified
- [x] Representative workloads selected

## Environment Mapping
- Worktree: `tmp/br-10-matchid-perf`
- Mode: benchmark harness and result capture

## Plan / Lots / Todo

- [ ] **Lot 0 - Benchmark contract**
  - [x] Define workload families
  - [x] Define hardware and environment normalization rules
  - [x] Define pass/fail thresholds

- [ ] **Lot 1 - Harness implementation**
  - [x] Add replay or artillery-based scenarios for MatchID-like workloads
  - [x] Add result capture format

- [ ] **Lot 2 - Parity evidence**
  - [ ] Run Elasticsearch baseline
  - [ ] Run Surch benchmark
  - [ ] Produce parity summary

## Feedback Loop
- Block on any ambiguity around workload realism or pass/fail thresholds.
- attention: the harness is testable locally with synthetic summaries, but real MatchID workload baselines are still missing.
- attention: final BR-10 completion requires Elasticsearch and Surch benchmark summaries produced from actual MatchID-relevant workloads.
- attention: `tests/matchid_parity/matchid_perf_profile.json` freezes the first inherited workload profile, but it is still only a starting point.

## Merge Checklist
- [x] Benchmark contract frozen
- [x] Harness reproducible
- [ ] Performance parity evidence produced
