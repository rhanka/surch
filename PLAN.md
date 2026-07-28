# Surch Global Plan

Updated: 2026-05-31

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

## Conductor Iteration Contract

This section answers the operational problem observed on 2026-05-20:
single-threaded status loops advanced too little per user turn.

- [ ] At the start of each non-trivial iteration, select a batch with at
  least two independent executable leaves when the worktree allows it.
- [ ] Dispatch up to four parallel agents only when ownership is
  disjoint; keep immediate blocker work local to the conductor.
- [ ] Give every dispatched agent an explicit leaf target expected to
  close at least one checkbox in a branch plan, or to unblock a run that
  closes one.
- [ ] Before final reporting, integrate agent output, run the relevant
  gates, and update the branch plan plus this file if the global state
  changed.
- [ ] If an active thread cannot advance by roughly 10% of its cited
  branch plan in one iteration, report the blocker as a concrete
  missing artefact, run, approval, or dependency instead of returning a
  soft status.

## Branch Index

- [x] `main`: current integration branch.
- [x] `wp/a-optim`: Track A long branch, head `30a7b32`;
  delivered lots tracked in `plan/wp-a-optim.md` (closed at `c5980ad`,
  branch kept for history). Track A perf is **rouvert** for follow-ups
  in `plan/wp-a-perf-followups.md`: TREC-COVID bulk scaling, skip
  lists on the codec path, next Block-Max WAND step, and the
  historical A-replay-1/2/3 line (delegated to
  `plan/perf-replay-wp-a-algo-ledger.md`).
- [ ] `wp/b-test-auto`: Track B long branch, head `65fc759`;
  detailed plan: `plan/wp-b-test-auto.md`.
- [ ] `wp/c-ops`: Track C long branch, head `2625edd`;
  detailed plan: `plan/wp-c-ops.md`.
- [ ] `wp/d-matchid`: Track D long branch, head `9e0e6b3`;
  detailed plan: `plan/wp-d-matchid.md`.
- [ ] `main` infra lane: Track E lives on `main` for now;
  detailed plan: `plan/main-infra.md`.
- [ ] Objective F (scientific perf write-up): lives on `main`;
  detailed plan: `plan/wp-f-perf-paper.md`.

## Track A - Perf / Optimisation

Statut de plan à réactualiser : P2 est livré localement mais ses gates
externes restent ouvertes ; aucun pourcentage de reste n'est publié avant
l'actualisation des plans de détail. **Surch now ingests the
full 171 k TREC-COVID corpus `1.54x` FASTER than OpenSearch**
(`56 s` vs `87 s`), reversing the `13.9x` OpenSearch advantage
pre-Lot-1 (`~17.8x` Surch bulk speedup), and **Lot 2 skip lists
improve Surch search-latency tail `p95 -13% / p99 -18%`** on the
matchID INSEE workload (isolated in
`2026-05-25-insee-lot2-skiplists-K8s`).

Lot 3 paired K8s perf-proof shows Surch hot path -21/-22/-12/-30 %
p50/p95/p99/max vs pre-FoR `c01b0a2`; runbook + numbers under
`docs/ops/bench-reports/2026-05-20-A-lot3-paired-K8s/`. The durable
axis-by-axis performance state is tracked in
`docs/ops/bench-reports/track-a-performance-ledger.md`. Active
follow-ups live in `plan/wp-a-perf-followups.md`; the cumulative
historical replay line lives in
`plan/perf-replay-wp-a-algo-ledger.md`.

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
- [x] Follow-up Lot 1 — TREC-COVID bulk scaling closed by
  `367acdc` (incremental `append_to_index`); proof in
  `docs/ops/bench-reports/2026-05-24-ndcg-gate-incremental-bulk-K8s/`,
  Surch TREC-COVID bulk `1001.95 s -> 179.86 s` (`~5.6x` speedup),
  Surch/OpenSearch ratio `13.9x slower -> 2.06x slower`.
- [x] Follow-up Lot 1.5 closed by `8a5150f` / promoted as
  `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.5-ram-K8s/`.
  `_refresh` drops the live `PostingsBuilder`; logical free works
  but system RSS recovers only `268 MiB` (`5859 -> 5591 MiB`) due
  to glibc default allocator inertia — addressed by new Lot 1.7.
- [x] Follow-up Lot 1.6 closed by `2e4361e` / promoted as
  `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.6-K8s/`.
  Deferred FST term-dictionary build off the bulk path: TREC-COVID
  Surch bulk `139 -> 56 s`, **Surch now `1.54x` faster than
  OpenSearch**; RSS peak `3424 -> 2156 MiB`. NDCG unchanged.
- [x] Follow-up Lot 1.7 closed by `b9f6636` / promoted as
  `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.7-jemalloc-K8s/`.
  Switched the Surch global allocator to jemalloc
  (`tikv-jemallocator` 0.6) +
  `MALLOC_CONF=background_thread:true,dirty_decay_ms:0,muzzy_decay_ms:0`.
  Surch RSS peak `5591 -> 3424 MiB` (`-39 %`), Surch RSS final
  `5591 -> 1382 MiB` (`-75 %`), bulk TREC-COVID
  `189 -> 139 s` (`-26 %` allocator bonus).
- [x] Follow-up Lot 2 — Skip lists on the codec FoR path
  (`d73c862`, leapfrog AND). Search-latency gain isolated via a
  paired `insee-bench` (control `b9f6636` vs `d73c862`, same
  jemalloc stack) promoted as
  `docs/ops/bench-reports/2026-05-25-insee-lot2-skiplists-K8s/`:
  Surch tail `p95 -13% / p99 -18%`, p50 flat, NDCG unchanged.
- [x] Follow-up Lot 3 — Next Block-Max WAND step (MaxScore
  block-leapfrog via Lot 2 skip lists). Landed + correctness-proven
  (ranking bit-stable, `ci` green), but **latency-neutral on
  INSEE 10k** (posting lists too short); promoted as
  `docs/ops/bench-reports/2026-05-25-lot3-bmw-skiplist-K8s/`. Kept,
  not claimed as a latency win — benefit regime (large corpora)
  needs a latency harness (Objective F F-gap-4).
- [ ] Follow-up Lot 4 — Historical A-replay-1/2/3 promotion, owned by
  `plan/perf-replay-wp-a-algo-ledger.md`.
- [x] Record a current perf + quality guardrail for the complete hot path:
  `docs/ops/bench-reports/track-a-performance-ledger.md` summarizes
  search latency, bulk, quality, RSS/memory, disk, and SLO axes with
  deltas and missing proof called out explicitly.
- [x] Start the cumulative non-rewrite Track A replay line:
  `perf-replay/wp-a-algo-ledger` commit `2100976` creates
  `plan/perf-replay-wp-a-algo-ledger.md`; K8s run `26193166785`
  promoted the first current-main replay report under
  `docs/ops/bench-reports/2026-05-20-A-replay-current-main-insee-K8s/`.
- [x] Define the required Track A replay proof protocol: K8s is
  mandatory for final replay verdicts, each compared ref needs at least
  three successful repetitions, and promoted reports must preserve image
  tags, pod/cluster configuration, monitoring diagnostics, run ids,
  artifacts, and repeated-run aggregation.
- [x] Preserve failed/invalid replay attempts in the trace: K8s runs
  `26200481514` and `26201223312` are documented as diagnostics only
  and do not count toward the required 3/3 final repetitions.
- [x] Promote the first post-wait-loop-fix current-main repetition:
  `ci-k8s` run `26202012197` on
  `ac558e6d08c7566f8cbc0b96c56a5b943eb1ae79`, artifact
  `7126271947`, report
  `docs/ops/bench-reports/2026-05-21-A-replay-current-main-insee-K8s-rep1/`.
- [x] Close the stable current-main repeated group on
  `61a13f871f810c98379375f2c94a10bbc696ac6e`: K8s runs
  `26202652997`, `26203320060`, and `26204062094` passed with
  artifacts `7126549971`, `7126727126`, and `7126979242`; report
  `docs/ops/bench-reports/2026-05-21-A-replay-current-main-61a13f-insee-K8s/`.
- [ ] Keep future Track A optimisation commits tied to a promoted perf
  report and an update to the Track A performance ledger.
- [ ] P1a — `bool.must` direct single-pass exact : route stricte de deux
  `match` vers le scoring fusionné `must`, sans élargir le chemin `should`.
  Plan et gates externes : `plan/p1a-bool-must-direct.md`.
- [ ] P2 — parcours checked des postings disque et multi-segments, avec
  repli intégral sur erreur, `df` global et compteurs de blocs. Développement
  livré localement, correction de revue de robustesse et harnais de mesure
  contrebalancé intégré localement ; le protocole apparie désormais la
  configuration CPU observée plutôt que de figer une taille d'hôte impossible
  avec ses caps mémoire. Les quatre phases chaudes restent fail-closed ; cold
  est diagnostique, le steal est local à chaque phase et le ratio de blocs est
  un résultat distinct de la validité. P3 remplace localement les trois copies
  résidentes par des digests BLAKE3-256 de pages de `TermEntry` et répertoire,
  avec repli checked intégral ; ses gates externes mémoire/latence restent
  ouvertes. Le harnais P3 prépare désormais le témoin match sur des `NOM`
  disjoints, avant tout bool, et conserve les états P3/mémoire/cgroup par
  phase ; la prochaine preuve reste la campagne externe A/B/C. Gates externes
  détaillées dans `plan/p2-segmented-postings.md`.
  La correction versionnée de re-revue verrouille la bijection des neuf runs,
  la matrice du gate et le smoke v4 ; toute publication reste bloquée jusqu'à
  la gate externe. Le pin C demeure `d0accd6`, donc aucune fraîcheur jemalloc
  issue de HEAD n'est revendiquée par la campagne contractuelle.

## Track B - Test Automation / Perf Reporting

Reste estime: 0% (Lot 4 closed; bonus paired-RSS replay closed via
`docs/ops/bench-reports/2026-05-23-ndcg-gate-7Gi-RSS-K8s/`).

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
- [x] Add paired RSS reporting for Surch vs Elasticsearch.
  - [x] Wire RSS peak/final into the K8s Track A replay artifact family
    before any A-replay memory-layout report claims a memory win.
- [x] Diagnose the TREC-COVID quality blocker before making it an
  acceptance gate.
  - [x] `61a13f8` makes the TREC-COVID script fail closed on HTTP
    errors and keeps bulk chunks below Surch's 16 MiB request cap; old
    run `26157480132` was a false green with hidden 413/400 curl
    failures.
  - [x] Rerun `26202629281` on `61a13f8` failed closed as intended:
    no 413 remained, but a remaining HTTP 400 stopped the gate before a
    summary could be published.
  - [x] Instrumented `ndcg-gate` run `26203362568` isolated
    `missing source line after \`index\` action` on chunk `bulk.0000`
    (split-by-byte cut between an `index` action and its source);
    `ff0d31c` rewrites the bulk chunker in awk on pair boundaries and
    `26266507485` confirms the HTTP 400 chain is fixed.
  - [x] Memory ceiling walked from 512 MiB to 4 GiB (OOM moved
    chunk 3 -> 16 -> 19 of ~21) then resolved by the node pool
    upgrade + Surch `7Gi` container cap in `d9cac15`: `ndcg-gate`
    run `26304471549` ingests the full 171 k corpus end-to-end in
    ~30 min (`conclusion=success`, artifact `7167929039`).
  - [x] Promoted `docs/ops/bench-reports/2026-05-22-ndcg-gate-7Gi-K8s/`
    with the full SciFact + TREC-COVID cross-engine numbers and
    cross-linked the Track A performance ledger Bulk + Quality rows.
    TREC-COVID is now a real cross-engine baseline (Surch NDCG@10
    `0.4750` vs OpenSearch `0.4902`, Surch trails by `-3.1%`),
    SciFact stays the active acceptance gate.
- [ ] Bonus: replay `ndcg-gate` on `b9faefe` (RSS sampler wired) to
  drop `RSS: not captured by current harness` from the Track A
  ledger SciFact / TREC-COVID rows.
- [x] Quota-unblocked `ndcg-gate` was dispatched and promoted
  (`poc-k8s` live quota `1500m/1Gi`, `4500m/6Gi`; run
  `26157480132`).

## Track C - Ops / Packaging / Snapshots

Reste estime: 0% (Lots 1-4 closed on `main`). Lot 4 (`75a7b35`)
ships `scripts/verify-release.sh` and `docs/ops/release-verification.md`
to replay signing + SBOM + image GHCR verification fail-closed
from a release tag.

- [x] Docker, Helm, release, signing, and SBOM work landed.
- [x] Snapshot and SLM work started on `wp/c-ops`.
- [x] SLM policy API merged on `main`.
- [x] `0a4ca02` refreshes snapshot/packaging plans against repo state.
- [x] `b14ca94` replaces stale `_pending_` workpackage rows with
  shipped SHAs.
- [x] `92a8ed9` covers and implements SLM `retention.max_count`
  pruning for successful snapshots.
- [x] SLM `retention.expire_after` now prunes expired successful
  snapshots while `min_count` preserves the newest snapshots.
- [x] S3/MinIO snapshot/restore e2e coverage landed on `main`:
  `b929dff` swaps the mock for MinIO and `d409cf3` bounds container
  startup.
- [x] The MinIO e2e test now requires `SURCH_MINIO_E2E=1` plus Docker;
  default `cargo test --workspace` skips the testcontainer path after
  GitHub run `26193965044` showed runner-side MinIO startup could hang.
- [x] Finish snapshot REST coverage.
  - [x] `GET /_snapshot/{repo}/_all` lists every snapshot in the
    repository using the same `snapshots: [...]` envelope as
    unitary snapshot GETs.
  - [x] `POST|GET /_snapshot/{repo}/_verify` round-trips a probe
    blob and returns the ES `{"nodes":{"local":{"name":"surch"}}}`
    envelope.
  - [x] `GET /_snapshot/_status` and `GET /_snapshot/{repo}/_status`
    return the ES `{"snapshots": []}` empty envelope (synchronous
    take model, no in-flight snapshots).
  - [x] `GET /_snapshot/{repo}/{snap}/_status` and
    `GET /_snapshot/{repo}/_all/_status` emit the ES per-snapshot
    `state` + `shards_stats` + per-index `indices` envelope.
- [x] Finish restore coverage.
  - [x] `POST /_snapshot/{repo}/{snap}/_restore` refuses to restore over
    an existing live index with `400 snapshot_exception` and an explicit
    `already exists` reason.
- [x] Finish remaining SLM retention behavior beyond `max_count`.
- [x] Release verification reproducible from CI artefacts: `75a7b35`
  ships `scripts/verify-release.sh` (tag-driven, fail-closed) and
  `docs/ops/release-verification.md` covering sha256 + minisign +
  cosign + SBOM CycloneDX + image GHCR runtime/bench.
- [x] Preserve a minimal path to inspect failing snapshot runs.

## Track D - matchID

Reste estime: B1 oracle phase closed. Phase 4 widening
(`plan/wp-d-matchid-phase4.md`) is now **active**: A10 write-time
sub-field fan-out landed (`0ea5218`+`3e764d7`+`f9470a5`) and keeps
matchID parity (b1-oracle `26404122287`: 30/30, 0 divergence,
promoted as `docs/ops/bench-reports/2026-05-25-b1-oracle-A10-ES861-K8s/`).
Remaining Phase 4 lots: A1/A13 multi-field + edge_ngram, A7 runtime
dates, A2 geo widening, A5 scoring widening, A6/A13 keyword-prefix,
A12 composite (consuming A10's `.raw`/`.norm` in sort/agg), B2
deces_v2. Tracked in that file.

- [x] A10 write-time sub-field fan-out (`.raw`/`.norm` stored at
  index time) landed; B1 parity preserved (b1-oracle 30/30). The
  query-side consumption (sort/agg on `.raw` without source-scan)
  is the A1/A12 follow-up.

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
- [x] Clean the active matchID docs so B1 and the swap guide point at
  Elasticsearch 8.6.1 as the live oracle; OpenSearch 2.17 remains only
  historical/client-compat context.
- [x] Historical note: `ci-k8s` run `26136585015` targeted the obsolete
  pre-correction oracle image; it is no longer the active matchID target
  and must not be used as final D parity proof.

## Track E - Infra K8s / poc-k8s

Reste estime: 0% (closure leaf met by the paired-RSS `ndcg-gate`
run promoted as
`docs/ops/bench-reports/2026-05-23-ndcg-gate-7Gi-RSS-K8s/`).

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
- [x] `ci` cargo-test job is now fail-closed with `timeout-minutes: 20`
  after run `26193965044` exposed an open-ended testcontainer hang.
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
- [x] `ci-k8s` now samples `kubectl top` during the wait loop after run
  `26200481514` showed post-completion pod metrics can be unavailable;
  that run is a K8s smoke, not a final A-replay repetition.
- [x] `ci-k8s` run `26201223312` diagnosed the next wait-loop edge:
  `insee-bench` produced a benchmark summary, live pod samples, and
  final `SuccessCriteriaMet=True` / `Complete=True`, but the workflow
  false-failed on expected restartable init-container sidecar exits
  after the benchmark driver exited `0`.
- [x] When a Job pod reaches `phase=Succeeded`, `ci-k8s` now waits for
  `condition=complete` before evaluating terminal sidecar exits, keeping
  the early failure checks without masking a successful Job.
- [x] Re-run `insee-bench` after the sidecar-completion wait-loop fix:
  runs `26202012197`, `26202652997`, `26203320060`, and `26204062094`
  uploaded benchmark summaries, Job conditions, pod diagnostics, and
  live pod metrics samples; `26202012197` is a single-repeat
  diagnostic, and the final repeated Track A group is the stable
  `61a13f8` triplet.
- [x] `ci-k8s` is now the standard heavy-run target: the paired
  `ndcg-gate` run promoted as
  `docs/ops/bench-reports/2026-05-23-ndcg-gate-7Gi-RSS-K8s/`
  produces summary, bench JSON, paired RSS envelopes
  (`surch.bench.rss.v1`), Job conditions, pod describe, live metrics
  samples, and cluster events in a single artifact reconstructed
  from driver-log markers after Job completion.
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

## Objective F - Scientific perf write-up

Reste estime: ~10% for the current Track A/F5 readout (F1/F2/F4/F5
closed for the available workloads; remaining proof is the 28M `deces`
scale run plus any future historical toggles that the paper chooses to
claim). Goal: turn the replayed Surch optimisation evaluations into a
publishable scientific article. Detailed plan + gap analysis:
`plan/wp-f-perf-paper.md`.

- [x] Feasibility assessed and current Track A/F5 readout finalized as
  an engineering performance report. Recent lots are cleanly
  K8s-isolated, the available workloads have multi-rep confirmation
  where final claims are made, and caveats are explicit.
- [x] F1 — methodology section shipped: `docs/paper/methodology.md`
  (system under test, environment, workloads, metric schemas,
  SLO/quality guardrails, fairness controls, isolation protocol,
  the single-run limitation, reproducibility).
- [x] F2 — 3-rep re-runs DONE for the available workloads:
  ndcg-gate bulk+RSS+quality (`2026-05-25-F2-ndcg-3rep-K8s/`: Surch
  TREC-COVID bulk median `70.96 s` non-overlapping vs OS `109.73 s`;
  RSS `2168 MiB ±0.5%`; NDCG bit-stable) and insee-bench latency
  (`2026-05-25-F2-insee-3rep-K8s/`: Surch median
  `1.5/4.1/8.4/40.6 ms` vs OS `4.0/12.2/26.3/223.1 ms`, `2.7–3.1x`
  faster).
- [~] F3 — historical optimisation isolation switched from impossible
  old-SHA replay to forward toggles on `perf-isolation`: WAND/MaxScore,
  LRU cache, top-K/lazy hydration measured; remaining historical family
  only matters if the final paper claims those optimisations one-by-one.
- [x] F4 — additional workloads delivered for the first draft:
  TREC-COVID large-corpus latency harness, 3-rep latency report, hits
  equivalence probe, and BEIR NFCorpus/FiQA quality widening.
- [x] F5 — Track A article/reporting readout assembled in
  `docs/paper/draft.md` with four-axis scorecard, optimisation
  trajectory, rendered SVG figures under `docs/paper/figures/`, caveats,
  and final A+F5 performance summary.
- [ ] F6 — next scale proof: run full `deces` 28M indexation ES/Surch
  and report duration, throughput, RSS, final doc count, and failure
  mode under the same no-cheat rules. **Preflight 2026-06-01**:
  direct dispatch is blocked because matchID `surch-eval-perf.yml`
  is still a 1.36M/dev workflow (`FILES_TO_PROCESS=deaths.txt.gz`,
  dev bucket, no expected-count/RSS/failure-mode gate). Last safe
  1.36M proof: matchID run `26704547454` (`surch-eval`
  `25894cf8`) shows Surch bulk `90.33 s` vs ES `120.40 s`;
  engine p50 `1.1 ms` vs ES `2.4 ms`; tail still behind
  (`p95/p99` Surch `10.7/14.1 ms` vs ES `5.7/8.2 ms`). Required
  before 28M: patch matchID workflow/script for full/prod inputs,
  fail-closed `docs == count == expected_count`, RSS capture,
  human `summary.md`, machine JSON artifacts, and a runner/remote
  shape that is not the default 2-vCPU GitHub runner.

## Delivery Finalities

- [ ] Track A finality: measurable search/index performance gains
  without quality regression.
- [ ] Track B finality: replayable, comparable benchmark reporting with
  explicit SLO verdicts.
- [x] Track C finality: release and snapshot paths verified end to
  end. Release verifier `scripts/verify-release.sh` shipped in
  `75a7b35`; snapshot REST + SLM retention + S3/MinIO restore
  e2e all on `main`.
- [ ] Track D finality: matchID parity proven against Elasticsearch 8.6.1,
  not only Surch HEAD.
- [ ] Track E finality: `ci-k8s` is a reliable heavy-benchmark target
  with preserved diagnostics.
- [~] Objective F finality: current Track A/F5 readout is publishable as
  an engineering performance report for the available workloads; the
  remaining frontier is the 28M `deces` scale proof before claiming full
  production-scale parity.

## Hors-track maintenance

- [ ] Dependabot demo deps: 4 moderate alerts on
  `demo/package-lock.json` (Svelte / @sveltejs/kit / svelte / cookie,
  all `scope: development`). The Surch crates carry no open alert.
  Fix as an isolated commit on `demo/`, not a Track A-E item.
- [x] Legacy `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md` (dated
  2026-05-04, predates the A-E track split) archived under
  `archive/plan/`.
