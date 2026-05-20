# wp/d-matchid Plan

Track principal: D - matchID
Branch: `wp/d-matchid`
Worktree: `.worktrees/wp-d`
Owner: conductor / SearchEngine / APIServer depending on slice
Status: Phase 3 harness exists on `main`, but the active oracle target
is Elasticsearch 8.6.1. The older 2026-05-20 B1 run used the obsolete
pre-correction oracle image and must be replayed against 8.6.1 before
Track D parity is called closed. Phase 4 widening
(A1/A2/A7/A13 multi-field, date{format}, geo_point, edge_ngram +
deces_v2 INSEE replay) is deferred to a follow-up plan when scoped.
Long branch `wp/d-matchid` head `9e0e6b3` kept for history.

## Finality

- [ ] Prove matchID parity against Elasticsearch 8.6.1, not only against
  Surch HEAD.

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
- [ ] Next scope: execute Elasticsearch 8.6.1 oracle and refresh fixture
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
- [x] Oracle runbook now points at a replayable script:
  `scripts/matchid/deces_v1_elasticsearch_oracle.py`.
- [ ] Oracle Elasticsearch 8.6.1 replay executed against a reference
  node.

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

- [x] Lot 3 - Elasticsearch oracle harness
  - [x] Define the action path for Elasticsearch 8.6.1 via
    `ELASTICSEARCH_URL`.
  - [x] Document comparison of status, total hits, top ids, and critical
    response shape.
  - [x] Document the human review artefact:
    `target/matchid-oracle/deces_v1/summary.md`.
  - [x] Extract the runbook heredoc into
    `scripts/matchid/deces_v1_elasticsearch_oracle.py`.
  - [x] Add a local `--dry-run` that validates inputs without requiring
    Elasticsearch.
  - [ ] Replay `tests/matchid_compat/replays/deces_v1.json` against
    Elasticsearch 8.6.1. The K8s `b1-oracle-gate` Job targets image
    `docker.elastic.co/elasticsearch/elasticsearch:8.6.1`.
  - [ ] Persist 8.6.1 oracle expectations or documented deltas in a
    promoted report. Historical note: GHA run `26136585015` targeted
    the obsolete pre-correction oracle image and is not the current
    matchID oracle proof.

- [x] Lot N - Closure
  - [x] Update this plan (this commit) and `PLAN.md` when next
    touched.
  - [x] Push branch/main and record SHA / run ids: matchid-replay
    crate extract `1fdd428`, b1_oracle binary `fda00e7`, K8s
    manifest `6214fc0` (+ fix-ups `c6031c1` / `c5c8a58` / `04c5d65`
    / `6402427` / `a1b9d1e` / `d9e032e`), promoted historical oracle
    report `801d047` / `929728f` on `main`. Active closure now requires
    the Elasticsearch 8.6.1 rerun; Phase 4 widening (A1/A2/A7/A13
    multi-field + date{format} + geo + edge_ngram, deces_v2 INSEE
    replay) goes under a follow-up plan when scoped.
