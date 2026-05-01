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
- [ ] BR-08 technical hardening complete
- [ ] MatchID performance baseline source identified
- [ ] Representative workloads selected

## Environment Mapping
- Worktree: `tmp/br-10-matchid-perf`
- Mode: benchmark harness and result capture

## Plan / Lots / Todo

- [ ] **Lot 0 - Benchmark contract**
  - [ ] Define workload families
  - [ ] Define hardware and environment normalization rules
  - [ ] Define pass/fail thresholds

- [ ] **Lot 1 - Harness implementation**
  - [ ] Add replay or artillery-based scenarios for MatchID-like workloads
  - [ ] Add result capture format

- [ ] **Lot 2 - Parity evidence**
  - [ ] Run Elasticsearch baseline
  - [ ] Run Surch benchmark
  - [ ] Produce parity summary

## Feedback Loop
- Block on any ambiguity around workload realism or pass/fail thresholds.

## Merge Checklist
- [ ] Benchmark contract frozen
- [ ] Harness reproducible
- [ ] Performance parity evidence produced
