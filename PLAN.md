# Surch Global Plan

Updated: 2026-05-18

This is the live conductor plan for Surch. It tracks the repo by the
official tracks A-E and points to branch-level plans under `plan/`.

Rules for maintaining this file are in `AGENTS.md`. This file is the
global status source; branch files carry executable detail.

## Tracking Rules

- [x] Track reporting follows A-E:
  - A: perf / optimisation
  - B: test automation / perf reporting
  - C: ops / packaging / snapshots
  - D: matchID
  - E: infra K8s / poc-k8s
- [x] Branch-level plans live under `plan/*.md`.
- [x] `% reste` is derived from unchecked leaf checkboxes in this file
  plus the referenced branch plan when finer detail exists.
- [ ] Keep this file updated whenever a branch status, merge status, or
  delivery gate changes.

## Branch Index

- [x] `main`: current integration branch.
- [x] `wp/a-optim`: Track A long branch, head `30a7b32`;
  detailed plan: `plan/wp-a-optim.md`.
- [ ] `wp/b-test-auto`: Track B long branch, head `65fc759`;
  detailed plan: `plan/wp-b-test-auto.md`.
- [ ] `wp/c-ops`: Track C long branch, head `8d0ba97`;
  detailed plan: `plan/wp-c-ops.md`.
- [ ] `wp/d-matchid`: Track D long branch, head `d5c6da0`;
  detailed plan: `plan/wp-d-matchid.md`.
- [ ] `main` infra lane: Track E lives on `main` for now;
  detailed plan: `plan/main-infra.md`.

## Track A - Perf / Optimisation

Reste estime: ~60% (4 open / 7 leaf tasks).

- [x] Land first hot-path wins on `main`: top-K collection, lazy
  `_source` hydration, WAND / Block-Max WAND, search cache, shared
  stored fields.
- [x] Publish paired OpenSearch baseline in
  `docs/ops/bench-reports/2026-05-16-vs-os-2.17.1/README.md`.
- [x] Add codec block metadata helper:
  `6f56fd2` on `main`, `30a7b32` on `wp/a-optim`.
- [ ] Wire the FoR postings codec metadata into the runtime engine path.
- [ ] Add skip lists on top of the codec path.
- [ ] Add the next Block-Max WAND step on top of block metadata.
- [ ] Finish the FST term dictionary path and refresh memory baselines.

## Track B - Test Automation / Perf Reporting

Reste estime: ~50% (4 open / 8 leaf tasks).

- [x] Bench plumbing exists:
  `scripts/bench/run-pair.sh`, `scripts/bench/rss-sample.sh`,
  `make bench-*`, `make report`, `artillery_bench`, `bench_report`.
- [x] Promoted paired SciFact baseline exists:
  `NDCG@10 0.6576` vs OpenSearch `0.6537`,
  `Recall@10 0.8100` vs `0.8033`.
- [x] Promoted BAN Paris 25k baseline exists:
  Surch `p50 0 ms`, `p95 20 ms`, `max 20 ms` vs OpenSearch
  `20 / 108 / 108 ms`.
- [x] `ec31e69` emits `summary.md` plus stable `summary.json`
  (`surch.bench.summary.v1`); `6a1fe89` fixes rustfmt.
- [ ] Promote the human report surface:
  `target/bench-reports/<sha>/summary.md` locally and
  `docs/ops/bench-reports/<date>-<context>/README.md` when promoted.
- [ ] Keep `summary.json` as an agent/CI-validated machine contract next
  to `summary.md`; the user should not be asked to review raw JSON.
- [ ] Add paired RSS reporting for Surch vs OpenSearch.
- [ ] Promote official paired reports for INSEE, artillery,
  TREC-COVID, and mMARCO-fr.

## Track C - Ops / Packaging / Snapshots

Reste estime: ~55% (6 open / 11 leaf tasks).

- [x] Docker, Helm, release, signing, and SBOM work landed.
- [x] Snapshot and SLM work started on `wp/c-ops`.
- [x] SLM policy API merged on `main`.
- [x] `0a4ca02` refreshes snapshot/packaging plans against repo state.
- [x] `b14ca94` replaces stale `_pending_` workpackage rows with
  shipped SHAs.
- [ ] Finish snapshot REST coverage.
- [ ] Run and document S3/MinIO end-to-end snapshot coverage.
- [ ] Finish restore coverage.
- [ ] Finish SLM retention.
- [ ] Keep release verification reproducible from CI artefacts.
- [ ] Preserve a minimal path to inspect failing release/snapshot runs.

## Track D - matchID

Reste estime: ~30% (2 open / 7 leaf tasks).

- [x] Intake flow exists under `docs/wp-d-matchid/incoming/`,
  `decisions/`, and `gap-analysis.md`.
- [x] Replay fixtures exist under `tests/matchid_compat/`.
- [x] `3cdac1f` implements `bool.must_not`.
- [x] `e532a08` syncs gap-analysis with A3 and B1 replay state.
- [x] B1 replay executes all 30 requests against Surch HEAD.
- [ ] Refresh OpenSearch / ES-7.x oracle fixtures so replay
  expectations come from OpenSearch, not Surch.
- [ ] Keep `docs/wp-d-matchid/gap-analysis.md` in sync with the oracle
  replay and document remaining parity gaps.

## Track E - Infra K8s / poc-k8s

Reste estime: ~50% (4 open / 8 leaf tasks).

- [x] Infra surface exists in `.github/workflows/ci-k8s.yml`,
  `deploy/k8s/jobs/`, and `docs/ops/k8s-ci.md`.
- [x] Recent `main` fixes hardened burst-bench failure handling and PVC
  bootstrap.
- [x] `23e60b8` makes `ci-k8s` fail fast when the expected GHCR image
  is missing.
- [x] `ci-k8s` run `26038117579` failed in 16s instead of the prior
  30m timeout pattern; `ci` run `26038398172` was green.
- [ ] Make `ci-k8s` the standard heavy-run target for burst and
  large-corpus checks.
- [ ] Repair the GHCR image handoff so `ndcg-gate` runs the benchmark:
  align the image tag contract between `docker-build.yml` and
  `ci-k8s.yml`, then make `make bench-k8s` fail with a clear next
  command when the image is missing.
- [ ] Always publish run diagnostics and artefacts on failure.
- [ ] Turn `make bench-k8s` into a real entry point.

## Delivery Finalities

- [ ] Track A finality: measurable search/index performance gains
  without quality regression.
- [ ] Track B finality: replayable, comparable benchmark reporting with
  explicit SLO verdicts.
- [ ] Track C finality: release and snapshot paths verified end to end.
- [ ] Track D finality: matchID parity proven against OpenSearch /
  ES-7.x, not only Surch HEAD.
- [ ] Track E finality: `ci-k8s` is a reliable heavy-benchmark target
  with preserved diagnostics.
