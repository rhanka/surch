# wp/d-matchid Plan

Track principal: D - matchID
Branch: `wp/d-matchid`
Worktree: `.worktrees/wp-d`
Owner: conductor / SearchEngine / APIServer depending on slice
Status: Phase 3 closed on `main` (B1 oracle 0/30 divergences on
2026-05-20). Phase 4 widening (A1/A2/A7/A13 multi-field, date{format},
geo_point, edge_ngram + deces_v2 INSEE replay) deferred to a
follow-up plan when scoped. Long branch `wp/d-matchid` head `9e0e6b3`
kept for history.

## Finality

- [ ] Prove matchID parity against Elasticsearch 7.x, not only against
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
- [ ] Next scope: execute Elasticsearch oracle and refresh fixture
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
- [ ] Oracle Elasticsearch replay executed against a reference node.

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
  - [x] Define the action path for Elasticsearch 7.x via
    `ELASTICSEARCH_URL`.
  - [x] Document comparison of status, total hits, top ids, and critical
    response shape.
  - [x] Document the human review artefact:
    `target/matchid-oracle/deces_v1/summary.md`.
  - [x] Extract the runbook heredoc into
    `scripts/matchid/deces_v1_elasticsearch_oracle.py`.
  - [x] Add a local `--dry-run` that validates inputs without requiring
    Elasticsearch.
  - [x] Replay `tests/matchid_compat/replays/deces_v1.json` against
    Elasticsearch — fully automated as the K8s `b1-oracle-gate` Job
    (Rust binary `b1_oracle` in `crates/surch-demo/src/bin/b1_oracle.rs`
    + manifest `deploy/k8s/jobs/b1-oracle-gate.yaml`). First green
    run on `d9e032e`: GHA run `26136585015`, **0 / 30 unexpected
    divergences** Surch ↔ ES 7.17.18.
  - [x] Persist oracle expectations or documented deltas. Report
    promoted at `docs/ops/bench-reports/2026-05-20-b1-oracle-K8s/`
    (envelope `surch.bench.b1_oracle.v1`, history of the three
    runs that flipped FAIL → PASS, KNOWN_PARTIAL_NAMES const
    documents the one expected divergence `sort_nom_desc` and why
    it is suppressed).

- [x] Lot N - Closure
  - [x] Update this plan (this commit) and `PLAN.md` when next
    touched.
  - [x] Push branch/main and record SHA / run ids: matchid-replay
    crate extract `1fdd428`, b1_oracle binary `fda00e7`, K8s
    manifest `6214fc0` (+ fix-ups `c6031c1` / `c5c8a58` / `04c5d65`
    / `6402427` / `a1b9d1e` / `d9e032e`), promoted report
    `801d047` / `929728f` on `main`. Phase 4 widening (A1/A2/A7/A13
    multi-field + date{format} + geo + edge_ngram, deces_v2 INSEE
    replay) goes under a follow-up plan when scoped.
