# wp/a-optim Plan

Track principal: A - perf / optimisation
Branch: `wp/a-optim`
Worktree: `.worktrees/wp-a`
Owner: conductor / StorageEngine / SearchEngine depending on slice
Status: closed, all lots delivered on `main` (Lot 3 paired K8s
perf-proof landed in `c5980ad` on 2026-05-20)
Replay follow-up: `plan/perf-replay-wp-a-algo-ledger.md` tracks the
historical cumulative proof line without reopening the delivered Lot 3
closure.

## Finality

- [ ] Deliver measurable search/index performance gains without quality
  regression.
- [x] Maintain an axis-by-axis performance ledger for Track A under
  `docs/ops/bench-reports/track-a-performance-ledger.md`.
- [ ] For future Track A progress, commit the performance proof and the
  ledger update in the same delivery slice.

## Scope

- [x] Allowed current scope: `crates/surch-codec/src/postings_block.rs`.
- [x] Delivered index scope: `crates/surch-index/src/postings.rs`,
  `crates/surch-index/tests/postings.rs`.
- [ ] Next allowed scope: encoded codec metadata consumption in search
  execution paths, to be narrowed before code changes.
- [x] Out of scope for current delivered lot: broad query execution
  refactors, benchmark result rewriting, release/ops changes.
- [x] Evidence source: targeted codec tests and promoted perf baselines.

## Merge State

- [x] Branch pushed to origin: `30a7b32`.
- [x] Cherry-picked to `main`: `6f56fd2`.
- [x] `main` push verified.
- [x] Local gate recorded:
  `cargo test -p surch-codec inspect_postings_blocks` OK.
- [x] Index gate recorded:
  `cargo test -p surch-index --test postings` OK for `2da9249`.
- [x] Runtime perf gate recorded after engine integration:
  `2026-05-20-A-lot3-paired-K8s/` (-21 % p50, -22 % p95, -12 % p99,
  -30 % max on Surch hot path vs pre-FoR `c01b0a2`).
- [x] Axis ledger recorded:
  `docs/ops/bench-reports/track-a-performance-ledger.md` lists Surch vs
  OpenSearch deltas, before/after FoR deltas, and the missing RSS/disk
  proof that must not be claimed yet.

## Lots

- [x] Lot -2 - Earlier hot-path deliveries already on `main`
  - [x] Scalar top-K finalization: `5081cc7`.
  - [x] Lazy `_source` hydration: `3157afb`.
  - [x] MaxScore/WAND OR-match skipping: `ed76014`.
  - [x] WAND `multi_match` extension and stale postings-builder drop:
    `65ccfbe`.
  - [x] Block-Max WAND per-128 contribution skipping: `e38bf91`.
  - [x] Search response cache: `644f62b`.
  - [x] Shared stored document sources: `4e9405a`, merge `f910094`.
  - [x] FST term dictionary: `c5f3155`, merge `0800f98`.
  - [x] Per-block stats persisted next to postings:
    `b680232`, merge `6df877d`.
  - [x] Memory metrics and `GET /_surch/stats`:
    `b8ed2bc`, merge `7caf339`.

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
  - [x] Locate the current postings metadata integration point:
    `surch-index` builds block metas consumed by `surch-api` scoring.
  - [x] Align `surch-index::postings::BLOCK_SIZE` with
    `surch-codec::postings_block::FOR_BLOCK_SIZE`.
  - [x] Add a focused test proving index block metas follow the codec
    block size.
  - [x] Wire encoded FoR payload metadata into search execution without
    changing ranking semantics.
  - [x] Gate: `cargo test -p surch-index --test postings`.
  - [x] Gate: `cargo test -p surch-search --test execution`.
  - [x] Commit main: `2da9249`.

- [x] Lot 3 - Perf proof
  - [x] Run before/after smoke perf on a small reproducible workload
    (`docs/ops/bench-reports/2026-05-19-criterion-for-meta/` — local
    Criterion smoke, noise-dominated on the dev workstation; signal
    re-routed to K8s).
  - [x] Run quality guardrail when search hot path changes
    (`cargo test -p surch-search --test execution` + matchid_compat
    2/2 — both green on every commit since `df3b0aa`).
  - [x] Record p50 / p95 / p99 / max and quality verdict in the
    report. Paired K8s capture at
    `docs/ops/bench-reports/2026-05-20-A-lot3-paired-K8s/`: Surch
    p50/p95/p99/max **-21 / -22 / -12 / -30 %** vs pre-FoR
    (`c01b0a2` + bootstrap cherry-picks), 0 errors / 13 170 issued
    each side. GHA runs `26151880297` (before) +
    `26101404966` (after).
  - [x] Refresh memory baselines after the FST / shared-source / FoR
    sequence: `2026-05-19-insee-10k-k8s/` promoted as the new
    post-FoR INSEE 10k baseline; SciFact + BAN baselines stay on
    `2026-05-16-vs-os-2.17.1/`.

- [x] Lot N - Closure
  - [x] Update this plan (this commit) and the live PLAN.md when
    next touched.
  - [x] Push branch and main integration: `df3b0aa` (Lot 2 wiring)
    + `c5980ad` (Lot 3 paired K8s report) on `main`. The
    measurement-only branch `perf-baseline/before-for-meta` stays on
    origin so a re-bench is one `gh workflow run` away.
  - [x] Record CI run id and final SHA: `main = c5980ad` at Lot 3
    closure; CI on the head commit tracked in `gh run list --branch
    main --workflow ci`.

- [ ] Follow-up - Historical cumulative replay ledger
  - [x] Start the non-rewrite replay trace on local branch
    `perf-replay/wp-a-algo-ledger`.
  - [x] Create the detailed replay plan:
    `plan/perf-replay-wp-a-algo-ledger.md`.
  - [x] Verify current-main GHCR images exist after `docker-build`
    run `26188578411`.
  - [x] Dispatch the first image-ready `ci-k8s` replay run and record
    its run id: `insee-bench` run `26193166785` on `main`
    `466693f55e1a3cd8b007e058be07584251986ecb`.
  - [ ] Promote the `26193166785` result and update this follow-up
    once the run finishes.
  - [ ] Promote A-replay-1 through A-replay-3 reports and update the
    Track A performance ledger rows.
