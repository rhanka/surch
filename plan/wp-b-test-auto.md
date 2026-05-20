# wp/b-test-auto Plan

Track principal: B - test automation / perf reporting
Branch: `wp/b-test-auto`
Worktree: `.worktrees/wp-b`
Owner: conductor / benchmark automation owner
Status: Lot 3 + Lot N closed on `main` for everything except the
TREC-COVID K8s baseline (the manifest is wired but waits on the
Scaleway tenant-quota apply tracked under Track E). Long branch
`wp/b-test-auto` head `65fc759` kept for history.

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

- [x] Lot 3 - Promoted reports
  - [x] Add `surch.bench.ban_http.v1` to `surch-demo ban-http-bench`.
  - [x] Render BAN HTTP Surch/Elasticsearch rows in human Markdown and
    `summary.json`.
  - [x] Align the CLI help, dry-run plan, and guardrails on
    Surch/Elasticsearch wording while preserving `--opensearch-url` as
    a legacy alias.
  - [x] Gate: user can compare Surch/Elasticsearch BAN HTTP p50 / p95 /
    p99 / max, errors, docs/s and bytes/s in Markdown.
  - [x] Ensure remaining benchmark producers can feed the summary
    contract — `04af736 feat(bench): aggregate beir ndcg reports`
    adds the BEIR `.out` parser to `bench_report`.
  - [ ] Add paired RSS reporting for Surch vs Elasticsearch
    (deferred — not on the matchID critical path).
  - [x] Promote INSEE report:
    `docs/ops/bench-reports/2026-05-19-insee-10k-k8s/README.md`
    (Surch p50/p95/p99/max = 1.9/3.6/6.9/17.9 ms, 0/13170 errors,
    all SLOs PASS).
  - [x] Promote artillery report — the same INSEE 10k slice ships
    the artillery driver output under
    `2026-05-19-insee-10k-k8s/artillery-runner.log`. The paired
    before/after capture went under
    `2026-05-20-A-lot3-paired-K8s/`.
  - [ ] Add TREC-COVID and mMARCO-fr measurement entries
    — TREC-COVID K8s extension is wired (`a993bc8`) but blocked on
    the Scaleway `limits.memory: 3 Gi` quota cap; will land as
    soon as `poc-k8s` HEAD `980d58d` is applied to the cluster.
    mMARCO-fr is out of scope for this plan.
  - [x] Gate: human can read the promoted Markdown without opening
    JSON — every report under `docs/ops/bench-reports/` carries a
    self-contained README.

- [x] Lot N - Closure
  - [x] Update this plan (this commit) and `PLAN.md` when next
    touched.
  - [x] Record report paths and CI/K8s run ids:
    `2026-05-19-insee-10k-k8s/` (run `26101404966`),
    `2026-05-20-b1-oracle-K8s/` (run `26136585015`),
    `2026-05-20-A-lot3-paired-K8s/` (run `26151880297` paired with
    `26101404966`).
  - [x] Push branch/main and record final SHA: `04af736` aggregator
    + `3006fae` baseline INSEE promo on `main`. The TREC-COVID
    promo will record its own SHA + run id when the quota apply
    unblocks it.
