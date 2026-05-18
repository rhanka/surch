# wp/b-test-auto Plan

Track principal: B - test automation / perf reporting
Branch: `wp/b-test-auto`
Worktree: `.worktrees/wp-b`
Owner: conductor / benchmark automation owner
Status: active branch exists; latest branch head `65fc759`

## Finality

- [ ] Deliver replayable, comparable benchmark reporting with explicit
  SLO verdicts.

## Scope

- [x] Allowed delivered scope on `main`: `crates/surch-demo/src/bin/`
  and `crates/surch-demo/tests/`.
- [ ] Next allowed scope: report promotion docs and benchmark artefact
  schema wiring.
- [x] Evidence source: bench report CLI tests, report artefacts,
  promoted baselines.

## Merge State

- [x] `summary.json` delivery merged to `main`: `ec31e69`.
- [x] Formatting fix merged to `main`: `6a1fe89`.
- [x] Local gates recorded:
  `cargo test -p surch-demo --test bench_report_cli` OK.
- [x] Local gates recorded:
  `cargo test -p surch-demo render_markdown_contains_required_sections --bin bench_report` OK.
- [ ] `wp/b-test-auto` contains the next branch-specific delivery.

## Lots

- [x] Lot 0 - Baseline and constraints
  - [x] Confirm current bench plumbing.
  - [x] Confirm promoted SciFact and BAN Paris paired baselines.
  - [x] Identify missing stable summary contract.

- [x] Lot 1 - Stable summary JSON
  - [x] Add `surch.bench.summary.v1`.
  - [x] Emit `summary.json` next to `summary.md`.
  - [x] Include artillery, RSS, pair, SLO, regression, unknown file, and
    verdict sections.
  - [x] Add CLI tests for empty and populated report dirs.
  - [x] Commit main: `ec31e69`.
  - [x] Commit formatting fix: `6a1fe89`.

- [ ] Lot 2 - Official report contract
  - [ ] Declare `summary.json` as the official comparable output.
  - [ ] Ensure every benchmark producer can feed the summary contract.
  - [ ] Add or update docs for replaying a report from artefacts.

- [ ] Lot 3 - Promoted reports
  - [ ] Add paired RSS reporting for Surch vs OpenSearch.
  - [ ] Promote INSEE report with SLO verdict.
  - [ ] Promote artillery report with p50 / p95 / p99 / max.
  - [ ] Add TREC-COVID and mMARCO-fr measurement entries.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Record report paths and CI/K8s run ids.
  - [ ] Push branch/main and record final SHA.
