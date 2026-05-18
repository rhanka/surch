# wp/d-matchid Plan

Track principal: D - matchID
Branch: `wp/d-matchid`
Worktree: `.worktrees/wp-d`
Owner: conductor / SearchEngine / APIServer depending on slice
Status: active branch exists; latest branch head `9e0e6b3`

## Finality

- [ ] Prove matchID parity against OpenSearch / ES-7.x, not only
  against Surch HEAD.

## Scope

- [x] Delivered code scope:
  `crates/surch-api/src/search.rs`,
  `crates/surch-api/tests/search.rs`,
  `crates/surch-api/tests/matchid_compat.rs`,
  `tests/matchid_compat/replays/deces_v1.json`.
- [x] Delivered doc scope:
  `docs/wp-d-matchid/gap-analysis.md`.
- [x] Delivered oracle runbook scope:
  `tests/matchid_compat/oracle/deces_v1.md`,
  `tests/matchid_compat/README.md`,
  `crates/surch-api/tests/matchid_compat.rs`.
- [ ] Next scope: execute OpenSearch oracle and refresh fixture
  expectations.
- [x] Evidence source: matchID replay and search integration tests.

## Merge State

- [x] `bool.must_not` support merged to `main`: `3cdac1f`.
- [x] Gap-analysis refresh merged to `main`: `e532a08`.
- [x] Local gates recorded:
  `cargo test -p surch-api bool_must_not --test search` OK.
- [x] Local gates recorded:
  `cargo test -p surch-api matchid_replay_deces_v1_executes_all_non_skipped_requests --test matchid_compat` OK.
- [x] Local gates recorded: `cargo test -p surch-api --test search`
  OK.
- [x] Oracle runbook merged and documented on `main`: `e8aca54`
  (branch commit `9e0e6b3`).
- [ ] Oracle OpenSearch replay executed against a reference node.

## Lots

- [x] Lot 0 - Baseline and constraints
  - [x] Confirm matchID intake and replay fixtures.
  - [x] Identify A3 `bool.must_not` as blocking gap.
  - [x] Confirm doc stale state.

- [x] Lot 1 - A3 `bool.must_not`
  - [x] Implement `bool.must_not`.
  - [x] Update search tests.
  - [x] Enable all B1 replay requests against Surch HEAD.
  - [x] Commit code: `3cdac1f`.

- [x] Lot 2 - Gap-analysis resync
  - [x] Update A3 state in gap-analysis.
  - [x] Update B1 replay state in gap-analysis.
  - [x] Commit docs: `e532a08`.

- [ ] Lot 3 - OpenSearch oracle harness
  - [x] Define the action path for OpenSearch / ES-7.x via
    `OPENSEARCH_URL`.
  - [x] Document comparison of status, total hits, top ids, and critical
    response shape.
  - [x] Document the human review artefact:
    `target/matchid-oracle/deces_v1/summary.md`.
  - [ ] Replay `tests/matchid_compat/replays/deces_v1.json` against
    OpenSearch.
  - [ ] Persist oracle expectations or documented deltas.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Push branch/main and record SHA / run ids.
