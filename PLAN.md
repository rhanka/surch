# Surch Global Plan

Updated: 2026-05-20

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
  detailed plan: `plan/wp-a-optim.md` (Track A closed on `main` at
  `c5980ad`, branch kept for history; skip-lists / next Block-Max
  WAND step deferred to a follow-up plan).
- [ ] `wp/b-test-auto`: Track B long branch, head `65fc759`;
  detailed plan: `plan/wp-b-test-auto.md`.
- [ ] `wp/c-ops`: Track C long branch, head `2625edd`;
  detailed plan: `plan/wp-c-ops.md`.
- [ ] `wp/d-matchid`: Track D long branch, head `9e0e6b3`;
  detailed plan: `plan/wp-d-matchid.md`.
- [ ] `main` infra lane: Track E lives on `main` for now;
  detailed plan: `plan/main-infra.md`.

## Track A - Perf / Optimisation

Status: **closed** on `main` at `c5980ad` (2026-05-20). Lot 3 paired
K8s perf-proof shows Surch hot path -21/-22/-12/-30 % p50/p95/p99/max
vs pre-FoR `c01b0a2`; runbook + numbers under
`docs/ops/bench-reports/2026-05-20-A-lot3-paired-K8s/`. The durable
axis-by-axis performance state is now tracked in
`docs/ops/bench-reports/track-a-performance-ledger.md`.

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
- [x] Finish runtime wiring from encoded FoR postings metadata into the
  search execution path: `df3b0aa`.
- [x] Refresh memory baselines after the FST / shared-source / FoR
  sequence: `2026-05-19-insee-10k-k8s/` (post-FoR) +
  `2026-05-20-A-lot3-paired-K8s/` (paired before/after).
- [ ] (deferred to a follow-up plan) Skip lists on top of the codec
  path.
- [ ] (deferred to a follow-up plan) Next Block-Max WAND step on top
  of encoded block metadata.
- [x] Record a current perf + quality guardrail for the complete hot path:
  `docs/ops/bench-reports/track-a-performance-ledger.md` summarizes
  search latency, bulk, quality, RSS/memory, disk, and SLO axes with
  deltas and missing proof called out explicitly.
- [x] Start the cumulative non-rewrite Track A replay line:
  `perf-replay/wp-a-algo-ledger` commit `2100976` creates
  `plan/perf-replay-wp-a-algo-ledger.md`; K8s run `26193166785`
  promoted the first current-main replay report under
  `docs/ops/bench-reports/2026-05-20-A-replay-current-main-insee-K8s/`.
- [ ] Keep future Track A optimisation commits tied to a promoted perf
  report and an update to the Track A performance ledger.

## Track B - Test Automation / Perf Reporting

Reste estime: ~25% (2 open / 10 leaf tasks).

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
- [x] BEIR `ndcg-gate` now emits a promoted diagnostic report:
  `docs/ops/bench-reports/2026-05-20-ndcg-gate-K8s/`
  from GHA run `26157480132`.
- [ ] Add paired RSS reporting for Surch vs Elasticsearch.
- [ ] Diagnose the TREC-COVID quality blocker before making it an
  acceptance gate: Surch completed all 50 qids but returned
  `NDCG@10=0.0000`, `Recall@10=0.0000` while OpenSearch returned
  `NDCG@10=0.1141`, `Recall@10=0.0026`.
- [x] Quota-unblocked `ndcg-gate` was dispatched and promoted
  (`poc-k8s` live quota `1500m/1Gi`, `4500m/6Gi`; run
  `26157480132`).

## Track C - Ops / Packaging / Snapshots

Reste estime: ~35% (4 open / 12 leaf tasks).

- [x] Docker, Helm, release, signing, and SBOM work landed.
- [x] Snapshot and SLM work started on `wp/c-ops`.
- [x] SLM policy API merged on `main`.
- [x] `0a4ca02` refreshes snapshot/packaging plans against repo state.
- [x] `b14ca94` replaces stale `_pending_` workpackage rows with
  shipped SHAs.
- [x] `92a8ed9` covers and implements SLM `retention.max_count`
  pruning for successful snapshots.
- [x] S3/MinIO snapshot/restore e2e coverage landed on `main`:
  `b929dff` swaps the mock for MinIO and `d409cf3` bounds container
  startup.
- [x] The MinIO e2e test now bounds each Docker/S3/API step with named
  timeouts, so a stuck CI run fails with the blocked step instead of
  hiding inside `cargo test --workspace`.
- [ ] Finish snapshot REST coverage.
- [ ] Finish restore coverage.
- [ ] Finish remaining SLM retention behavior beyond `max_count`.
- [ ] Keep release verification reproducible from CI artefacts.
- [x] Preserve a minimal path to inspect failing snapshot runs.

## Track D - matchID

Reste estime: ~0% for the closed B1 oracle phase. Phase 4 widening is
deferred to a follow-up plan when scoped.

- [x] Intake flow exists under `docs/wp-d-matchid/incoming/`,
  `decisions/`, and `gap-analysis.md`.
- [x] Replay fixtures exist under `tests/matchid_compat/`.
- [x] `3cdac1f` implements `bool.must_not`.
- [x] `e532a08` syncs gap-analysis with A3 and B1 replay state.
- [x] B1 replay executes all 30 requests against Surch HEAD.
- [x] `e8aca54` documents the `deces_v1` Elasticsearch oracle
  gate and human `summary.md` output.
- [x] The `deces_v1` Elasticsearch oracle gate is now an executable
  script with a local `--dry-run`, so the external run no longer depends
  on copying Python out of Markdown.
- [x] Execute the Elasticsearch 8.6.1 oracle gate and refresh fixture
  expectations from that reference, not Surch:
  `ci-k8s` run `26192816780` PASS, 30 requests, 0 skipped,
  0 divergence; promoted report
  `docs/ops/bench-reports/2026-05-20-b1-oracle-ES861-K8s/`.
- [x] Keep `docs/wp-d-matchid/gap-analysis.md` in sync with the
  Elasticsearch 8.6.1 oracle replay and document remaining parity gaps.
- [x] Historical note: `ci-k8s` run `26136585015` targeted the obsolete
  pre-correction oracle image; it is no longer the active matchID target
  and must not be used as final D parity proof.

## Track E - Infra K8s / poc-k8s

Reste estime: ~4% (1 open / 25 leaf tasks).

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
- [x] Docker build now publishes a separate shell-capable benchmark
  driver tag `bench-sha-<full commit SHA>` next to the distroless
  runtime tag.
- [x] `docker-build` run `26063701483` proved the runtime image still
  publishes, then failed the new bench-driver stage because
  `.dockerignore` excluded `scripts/bench/scifact-ndcg.sh`.
- [x] `.dockerignore` now re-includes only
  `scripts/bench/scifact-ndcg.sh` from the ignored scripts tree, so the
  bench-driver stage can copy the K8s SciFact gate script.
- [x] `docker-build` run `26064128510` published both runtime and
  bench-driver images for `6a493e0`.
- [x] `ci-k8s` run `26064198159` proved the published GHCR runtime and
  bench-driver images are pullable by K8s, and `ndcg-driver` reached
  benchmark execution then exited `0` after reaching the report-write
  path.
- [x] The next K8s blocker is diagnosed: regular engine sidecars kept
  the Pod running after the driver completed, so the Job timed out at
  30 min despite benchmark execution.
- [x] `scifact-ndcg.sh` now uses a writable temporary bulk NDJSON when
  the BEIR corpus mount is read-only and fails closed on shell, `jq`, or
  `curl` errors.
- [x] `ndcg-gate` and `insee-bench` use the bench driver tag for
  scripts/tools while keeping `surch-api` on the runtime image.
- [x] `ndcg-gate` and `insee-bench` now declare Surch and the reference
  engine as restartable init-container sidecars, so the Job can complete
  when the benchmark driver exits.
- [x] The reference engine sidecar declares its own `1000:1000`
  security context instead of inheriting the Surch runtime user.
- [x] `make bench-k8s` prints both the runtime and bench driver image
  tags before dispatch.
- [x] `f6687db` added a shell/tar-capable bench driver path plus an
  `ndcg-gate` summary output; `docker-build` run `26066037314` and
  `ci-k8s` run `26066084990` proved the images and Job completion path,
  then exposed that post-completion `kubectl cp` cannot be the only
  report collection path.
- [x] `09d1f15` reconstructs benchmark summaries from marked driver
  logs for both `ndcg-gate` and `insee-bench` when direct `/reports`
  copy is unavailable after container termination.
- [x] `docker-build` run `26066406292` published both runtime and
  bench-driver images for `09d1f15`.
- [x] `ci-k8s` run `26066458910` completed `ndcg-gate` with
  `SuccessCriteriaMet=True`, `Complete=True`, and artifact
  `k8s-bench-ndcg-gate-09d1f15dedb3e176ae6a9d5f89ef49100496776f`
  containing `ndcg-gate.summary.md` and `ndcg-gate.bench.json`.
- [ ] Make `ci-k8s` the standard heavy-run target for burst and
  large-corpus checks.
- [x] Apply the Surch tenant quota bump from `poc-k8s` HEAD `980d58d`
  to the live cluster: quota now reads `requests.cpu=1500m`,
  `requests.memory=1Gi`, `limits.cpu=4500m`, `limits.memory=6Gi`.
- [x] Always publish run diagnostics and artefacts on failure.
- [x] Provide a shell-capable benchmark driver image/stage for
  `ndcg-gate` and `insee-bench`.
- [x] Move the reference engine sidecar to a compatible per-container
  security context.
- [x] Verify on GitHub Actions that a published GHCR image reaches
  `ndcg-gate` benchmark execution after the runtime fixes.
- [x] Verify on GitHub Actions that the restartable sidecar manifests
  report Job `Complete=True`.
- [x] Turn `make bench-k8s` into a real entry point.

## Delivery Finalities

- [ ] Track A finality: measurable search/index performance gains
  without quality regression.
- [ ] Track B finality: replayable, comparable benchmark reporting with
  explicit SLO verdicts.
- [ ] Track C finality: release and snapshot paths verified end to end.
- [ ] Track D finality: matchID parity proven against Elasticsearch 8.6.1,
  not only Surch HEAD.
- [ ] Track E finality: `ci-k8s` is a reliable heavy-benchmark target
  with preserved diagnostics.
