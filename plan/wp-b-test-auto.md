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
- [x] Delivered promotion scope on `main`: `--promote-dir` in
  `bench_report` and CLI tests.
- [ ] Next allowed scope: producer coverage and promoted benchmark
  report publication.
- [x] Human-facing report surface:
  `target/bench-reports/<sha>/summary.md` for local runs and
  `docs/ops/bench-reports/<date>-<context>/README.md` for promoted
  reports.
- [x] Machine-facing artefact:
  `target/bench-reports/<sha>/summary.json`, generated next to
  `summary.md`; agents/CI validate it, not the user.
- [x] Evidence source: bench report CLI tests, report artefacts,
  promoted baselines.

## Merge State

- [x] `summary.json` delivery merged to `main`: `ec31e69`.
- [x] Formatting fix merged to `main`: `6a1fe89`.
- [x] Local gates recorded:
  `cargo test -p surch-demo --test bench_report_cli` OK.
- [x] Local gates recorded:
  `cargo test -p surch-demo render_markdown_contains_required_sections --bin bench_report` OK.
- [x] Promotion output merged to `main`: `bd00e9e`.
- [x] BAN HTTP Surch/Elasticsearch producer is wired into
  `bench_report` via `surch.bench.ban_http.v1`.
- [x] BAN HTTP CLI presents the paired path as Surch/Elasticsearch:
  `--elasticsearch-url` is the documented flag; `--opensearch-url`
  remains a legacy alias only.
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

- [x] Lot 2 - Official report contract
  - [x] Declare `summary.md` / promoted `README.md` as the human review
    surface.
  - [x] Declare `summary.json` as the agent/CI comparison contract.
  - [x] Add a help note with exact output paths:
    `target/bench-reports/<sha>/summary.md` and
    promoted `docs/ops/bench-reports/<date>-<context>/README.md`.
  - [x] Add `--promote-dir` to write promoted `README.md` plus
    `summary.json`.
  - [x] Gate: user-facing report contains a plain-language verdict;
    JSON schema is validated by tests or CI, not by user review.
  - [x] Commit main: `bd00e9e`.

- [ ] Lot 3 - Promoted reports
  - [x] Add `surch.bench.ban_http.v1` to `surch-demo ban-http-bench`.
  - [x] Render BAN HTTP Surch/Elasticsearch rows in human Markdown and
    `summary.json`.
  - [x] Align the CLI help, dry-run plan, and guardrails on
    Surch/Elasticsearch wording while preserving `--opensearch-url` as
    a legacy alias.
  - [x] Gate: user can compare Surch/Elasticsearch BAN HTTP p50 / p95 /
    p99 / max, errors, docs/s and bytes/s in Markdown.
  - [ ] Ensure remaining benchmark producers can feed the summary
    contract.
  - [ ] Add paired RSS reporting for Surch vs Elasticsearch.
  - [ ] Promote INSEE report to
    `docs/ops/bench-reports/<date>-insee-*/README.md` with SLO verdict.
  - [ ] Promote artillery report to
    `docs/ops/bench-reports/<date>-artillery-*/README.md` with
    p50 / p95 / p99 / max.
  - [ ] Add TREC-COVID and mMARCO-fr measurement entries.
  - [ ] Gate: human can read the promoted Markdown without opening JSON.

- [ ] Lot N - Closure
  - [ ] Update this plan and `PLAN.md`.
  - [ ] Record report paths and CI/K8s run ids.
  - [ ] Push branch/main and record final SHA.
