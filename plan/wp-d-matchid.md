# wp/d-matchid Plan

Track principal: D - matchID
Branch: `wp/d-matchid`
Worktree: `.worktrees/wp-d`
Owner: conductor / SearchEngine / APIServer depending on slice
Status: active branch exists; latest branch head `d5c6da0`

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
- [ ] Next scope: OpenSearch oracle harness and fixture refresh.
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
- [ ] Oracle OpenSearch replay merged and documented.

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
  - [ ] Define launch mode for OpenSearch / ES-7.x.
  - [ ] Replay `tests/matchid_compat/replays/deces_v1.json` against
    OpenSearch.
  - [ ] Compare status, total hits, top ids, and critical response shape.
  - [ ] Persist oracle expectations or documented deltas.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Push branch/main and record SHA / run ids.
