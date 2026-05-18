# wp/a-optim Plan

Track principal: A - perf / optimisation
Branch: `wp/a-optim`
Worktree: `.worktrees/wp-a`
Owner: conductor / StorageEngine / SearchEngine depending on slice
Status: active, pushed; latest branch head `30a7b32`

## Finality

- [ ] Deliver measurable search/index performance gains without quality
  regression.

## Scope

- [x] Allowed current scope: `crates/surch-codec/src/postings_block.rs`.
- [ ] Next allowed scope: codec integration points in `surch-index` and
  search execution paths, to be narrowed before code changes.
- [x] Out of scope for current delivered lot: broad query execution
  refactors, benchmark result rewriting, release/ops changes.
- [x] Evidence source: targeted codec tests and promoted perf baselines.

## Merge State

- [x] Branch pushed to origin: `30a7b32`.
- [x] Cherry-picked to `main`: `6f56fd2`.
- [x] `main` push verified.
- [x] Local gate recorded:
  `cargo test -p surch-codec inspect_postings_blocks` OK.
- [ ] Runtime perf gate recorded after engine integration.

## Lots

- [x] Lot 0 - Baseline and constraints
  - [x] Confirm worktree `.worktrees/wp-a`.
  - [x] Confirm branch `wp/a-optim`.
  - [x] Identify stale local test-only delta in codec.

- [x] Lot 1 - Codec block metadata helper
  - [x] Add `FOR_BLOCK_SIZE`.
  - [x] Add `PostingsBlockMeta`.
  - [x] Add `inspect_postings_blocks(...)`.
  - [x] Add boundary, seeded corpus, and truncated-tail tests.
  - [x] Gate: `cargo test -p surch-codec inspect_postings_blocks`.
  - [x] Commit branch: `30a7b32`.
  - [x] Cherry-pick main: `6f56fd2`.

- [ ] Lot 2 - Runtime integration
  - [ ] Locate exact postings metadata integration point in
    `surch-index` / search execution.
  - [ ] Add a focused test proving block metadata is consumed in the
    runtime path.
  - [ ] Wire the helper without changing ranking semantics.
  - [ ] Gate: targeted index/search test.

- [ ] Lot 3 - Perf proof
  - [ ] Run before/after smoke perf on a small reproducible workload.
  - [ ] Run quality guardrail when search hot path changes.
  - [ ] Record p50 / p95 / p99 / max and quality verdict in the report.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Push branch and main integration.
  - [ ] Record CI run id and final SHA.
