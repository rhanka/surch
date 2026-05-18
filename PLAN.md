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
- [ ] `wp/c-ops`: Track C long branch, head `2625edd`;
  detailed plan: `plan/wp-c-ops.md`.
- [ ] `wp/d-matchid`: Track D long branch, head `9e0e6b3`;
  detailed plan: `plan/wp-d-matchid.md`.
- [ ] `main` infra lane: Track E lives on `main` for now;
  detailed plan: `plan/main-infra.md`.

## Track A - Perf / Optimisation

Reste estime: ~30% (5 open / 18 leaf tasks).

- [x] Land scalar top-K finalization: `5081cc7`.
- [x] Land lazy `_source` hydration for scored top-K: `3157afb`.
- [x] Land MaxScore/WAND skipping for OR-match top-K: `ed76014`.
- [x] Extend WAND to `multi_match` and drop stale postings builders:
  `65ccfbe`.
- [x] Land Block-Max WAND per-128 max contribution skipping:
  `e38bf91`.
- [x] Land per-index LRU search response cache: `644f62b`.
- [x] Share stored document sources: `4e9405a`, merge `f910094`.
- [x] Replace nested term map with FST term dictionary:
  `c5f3155`, merge `0800f98`.
- [x] Persist per-block stats next to postings:
  `b680232`, merge `6df877d`.
- [x] Add memory metrics and `GET /_surch/stats`:
  `b8ed2bc`, merge `7caf339`.
- [x] Publish historical paired reference baseline in
  `docs/ops/bench-reports/2026-05-16-vs-os-2.17.1/README.md`.
- [x] Add codec block metadata helper:
  `6f56fd2` on `main`, `30a7b32` on `wp/a-optim`.
- [x] Align `surch-index` block metadata sizing with the codec source of
  truth: `2da9249` makes `BLOCK_SIZE` derive from `FOR_BLOCK_SIZE`.
- [ ] Finish runtime wiring from encoded FoR postings metadata into the
  search execution path.
- [ ] Add skip lists on top of the codec path.
- [ ] Add the next Block-Max WAND step on top of encoded block metadata.
- [ ] Refresh memory baselines after the FST / shared-source / FoR
  sequence.
- [ ] Record a current perf + quality guardrail for the complete hot path.

## Track B - Test Automation / Perf Reporting

Reste estime: ~30% (3 open / 10 leaf tasks).

- [x] Bench plumbing exists:
  `scripts/bench/run-pair.sh`, `scripts/bench/rss-sample.sh`,
  `make bench-*`, `make report`, `artillery_bench`, `bench_report`.
- [x] Promoted historical SciFact paired baseline exists:
  Surch `NDCG@10 0.6576`, reference `0.6537`;
  Surch `Recall@10 0.8100`, reference `0.8033`.
- [x] Promoted BAN Paris 25k baseline exists:
  Surch `p50 0 ms`, `p95 20 ms`, `max 20 ms` vs reference
  `20 / 108 / 108 ms`.
- [x] `ec31e69` emits `summary.md` plus stable `summary.json`
  (`surch.bench.summary.v1`); `6a1fe89` fixes rustfmt.
- [x] `bd00e9e` adds promoted human output via `--promote-dir`:
  `summary.md` stays local, promoted reports write `README.md`, and
  `summary.json` remains the agent/CI machine contract.
- [x] BAN HTTP Surch/Elasticsearch reports now emit
  `surch.bench.ban_http.v1` and are rendered by `bench_report` into
  human Markdown plus `summary.json`.
- [x] BAN HTTP CLI now presents the paired path as Surch/Elasticsearch:
  `--elasticsearch-url` is the documented flag and `--opensearch-url`
  remains only a legacy alias.
- [ ] Ensure remaining benchmark producers can feed the summary
  contract.
- [ ] Add paired RSS reporting for Surch vs Elasticsearch.
- [ ] Promote official Elasticsearch/Surch paired reports for INSEE,
  artillery,
  TREC-COVID, and mMARCO-fr.

## Track C - Ops / Packaging / Snapshots

Reste estime: ~50% (6 open / 12 leaf tasks).

- [x] Docker, Helm, release, signing, and SBOM work landed.
- [x] Snapshot and SLM work started on `wp/c-ops`.
- [x] SLM policy API merged on `main`.
- [x] `0a4ca02` refreshes snapshot/packaging plans against repo state.
- [x] `b14ca94` replaces stale `_pending_` workpackage rows with
  shipped SHAs.
- [x] `92a8ed9` covers and implements SLM `retention.max_count`
  pruning for successful snapshots.
- [ ] Finish snapshot REST coverage.
- [ ] Run and document S3/MinIO end-to-end snapshot coverage.
- [ ] Finish restore coverage.
- [ ] Finish remaining SLM retention behavior beyond `max_count`.
- [ ] Keep release verification reproducible from CI artefacts.
- [ ] Preserve a minimal path to inspect failing release/snapshot runs.

## Track D - matchID

Reste estime: ~20% (2 open / 9 leaf tasks).

- [x] Intake flow exists under `docs/wp-d-matchid/incoming/`,
  `decisions/`, and `gap-analysis.md`.
- [x] Replay fixtures exist under `tests/matchid_compat/`.
- [x] `3cdac1f` implements `bool.must_not`.
- [x] `e532a08` syncs gap-analysis with A3 and B1 replay state.
- [x] B1 replay executes all 30 requests against Surch HEAD.
- [x] `e8aca54` documents the `deces_v1` Elasticsearch 7.x oracle
  gate and human `summary.md` output.
- [x] The `deces_v1` Elasticsearch oracle gate is now an executable
  script with a local `--dry-run`, so the external run no longer depends
  on copying Python out of Markdown.
- [ ] Execute the Elasticsearch 7.x oracle gate and refresh fixture
  expectations from that reference, not Surch.
- [ ] Keep `docs/wp-d-matchid/gap-analysis.md` in sync with the oracle
  replay and document remaining parity gaps.

## Track E - Infra K8s / poc-k8s

Reste estime: ~31% (4 open / 13 leaf tasks).

- [x] Infra surface exists in `.github/workflows/ci-k8s.yml`,
  `deploy/k8s/jobs/`, and `docs/ops/k8s-ci.md`.
- [x] Recent `main` fixes hardened burst-bench failure handling and PVC
  bootstrap.
- [x] `23e60b8` makes `ci-k8s` fail fast when the expected GHCR image
  is missing.
- [x] `ci-k8s` run `26038117579` failed in 16s instead of the prior
  30m timeout pattern; `ci` run `26038398172` was green.
- [x] `5c25463` aligns image handoff on `sha-<full commit SHA>` across
  `docker-build.yml`, `release.yml`, `ci-k8s.yml`, and `make bench-k8s`;
  missing-image errors now print the exact remediation command.
- [x] Docker builder toolchain aligned with the Cargo.lock MSRV floor
  (`rustc >= 1.91.1`) after `docker-build` run `26057290880` exposed
  the stale `rust:1.88` base image.
- [x] K8s Job manifests now consume the same `sha-<full commit SHA>`
  image tag that `docker-build.yml`, `ci-k8s.yml`, and `make bench-k8s`
  verify before dispatch.
- [x] `ci-k8s` run `26058595173` proved the image tag reaches K8s and
  uploaded
  `k8s-bench-ndcg-gate-236980c600a60c40a8f28e2c433558c59ec5d5f7`.
- [x] `ci-k8s` wait logic now fails early on pod phase `Failed`,
  terminal container waiting / terminated reasons, and non-zero
  container exits.
- [x] Runtime blockers from `26058595173` are diagnosed: the distroless
  Surch runtime image cannot run `/bin/sh` as a benchmark driver, and
  the reference engine sidecar needs a compatible per-container security
  context.
- [ ] Make `ci-k8s` the standard heavy-run target for burst and
  large-corpus checks.
- [x] Always publish run diagnostics and artefacts on failure.
- [ ] Provide a shell-capable benchmark driver image/stage for
  `ndcg-gate` and `insee-bench`.
- [ ] Move the reference engine sidecar to a compatible per-container
  security context.
- [ ] Verify on GitHub Actions that a published GHCR image reaches
  `ndcg-gate` benchmark execution after the runtime fixes.
- [ ] Turn `make bench-k8s` into a real entry point.

## Delivery Finalities

- [ ] Track A finality: measurable search/index performance gains
  without quality regression.
- [ ] Track B finality: replayable, comparable benchmark reporting with
  explicit SLO verdicts.
- [ ] Track C finality: release and snapshot paths verified end to end.
- [ ] Track D finality: matchID parity proven against Elasticsearch 7.x,
  not only Surch HEAD.
- [ ] Track E finality: `ci-k8s` is a reliable heavy-benchmark target
  with preserved diagnostics.
