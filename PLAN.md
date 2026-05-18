# Surch Conductor Plan

Updated: 2026-05-18

This is the live conductor plan for Surch. Official tracking runs on
five tracks:

- Track A: perf / optimisation
- Track B: test automation / perf reporting
- Track C: ops / packaging / snapshots
- Track D: matchID
- Track E: infra K8s / poc-k8s

The old phase plan from 2026-05-04 remains useful as an architecture
reference, but it is no longer the primary day-to-day tracking format.

Official work branches:

- `wp/a-optim`
- `wp/b-test-auto`
- `wp/c-ops`
- `wp/d-matchid`

Track E currently lives on `main` through CI and `deploy/k8s/` changes
until a dedicated infra branch is needed.

## Fait

### Track A - perf / optimisation

- `main` already carries the first hot-path wins: top-K collection,
  lazy `_source` hydration, WAND / Block-Max WAND, search cache, and
  shared stored fields.
- The current optimisation branch is `wp/a-optim` with its worktree in
  `.worktrees/wp-a`.
- The published paired baseline against OpenSearch lives in
  `docs/ops/bench-reports/2026-05-16-vs-os-2.17.1/README.md`.

### Track B - test automation / perf reporting

- Bench plumbing already exists: `scripts/bench/run-pair.sh`,
  `scripts/bench/rss-sample.sh`, `make bench-*`, `make report`,
  `crates/surch-demo/src/bin/artillery_bench.rs`, and
  `crates/surch-demo/src/bin/bench_report.rs`.
- JSON artefacts are expected under `target/bench-reports/<sha>/`.
- Promoted paired baselines already exist for:
  - SciFact: `NDCG@10 0.6576` vs OpenSearch `0.6537`,
    `Recall@10 0.8100` vs `0.8033`
  - BAN Paris 25k: `p50 0 ms`, `p95 20 ms`, `max 20 ms`
    vs OpenSearch `20 / 108 / 108 ms`

### Track C - ops / packaging / snapshots

- Docker, Helm, release, signing, and SBOM work is already landed.
- Snapshot and SLM work is already started on `wp/c-ops`; the SLM policy
  API is merged on `main`.
- `ci` and `ci-k8s` are the current automation anchors for this track.

### Track D - matchID

- The intake flow exists under `docs/wp-d-matchid/incoming/`,
  `decisions/`, and `gap-analysis.md`.
- Replay fixtures already exist under `tests/matchid_compat/`.
- Actual implementation is ahead of the stale doc on several points:
  A6, A12, B1, and A13 have already moved.

### Track E - infra K8s / poc-k8s

- The infra surface already exists in `.github/workflows/ci-k8s.yml`,
  `deploy/k8s/jobs/`, and `docs/ops/k8s-ci.md`.
- Recent fixes on `main` hardened burst-bench failure handling and PVC
  bootstrap for K8s jobs.

## A faire

### Track A - perf / optimisation

- Wire the live FoR postings codec into the engine path.
- Add skip lists and the next Block-Max WAND step on top of the codec.
- Finish the FST term dictionary path and keep memory baselines current.

### Track B - test automation / perf reporting

- Unify every benchmark output into one comparable JSON schema.
- Add paired RSS reporting for Surch vs OpenSearch.
- Promote official paired reports for INSEE and artillery, not only PoC
  notes.
- Measure and report TREC-COVID and mMARCO-fr instead of keeping only
  targets.

### Track C - ops / packaging / snapshots

- Finish snapshot REST coverage, S3 e2e, restore, and SLM retention.
- Keep release verification reproducible from CI artefacts.
- Preserve a minimal path to inspect failing release and snapshot runs.

### Track D - matchID

- Fix A3 first: `bool.must_not` must no longer be ignored silently.
- Refresh OpenSearch-oracle fixtures so replay expectations come from
  OpenSearch, not from Surch itself.
- Bring `docs/wp-d-matchid/gap-analysis.md` back in sync with the code.

### Track E - infra K8s / poc-k8s

- Make `ci-k8s` the standard heavy-run target for burst and large-corpus
  checks.
- Always publish run diagnostics and artefacts on failure.
- Turn `make bench-k8s` into a real entry point instead of a placeholder.

## Attendus

### Track A - perf / optimisation

- Every major hot-path change must report before/after metrics.
- Minimum acceptance gate for merges to `main`:
  - SciFact `NDCG@10 >= 0.65`
  - explicit non-regression on Rue Payenne

### Track B - test automation / perf reporting

- Every meaningful perf advance must produce a replayable benchmark
  report.
- The minimal recurring report must state:
  - latency `p50 / p95 / p99 / max`
  - ingestion throughput (`docs/s` or indexed corpus duration)
  - RSS peak and final
  - `NDCG@10`
  - `Recall@10`
  - SLO verdict (`pass` / `fail`)
  - OpenSearch baseline comparison when available
- Heavy perf runs should prefer CI / K8s over local execution.

### Track C - ops / packaging / snapshots

- Snapshot, packaging, and release work must be reported with the exact
  CI or K8s run ids and produced artefacts.
- CI must fail closed on broken snapshot or release paths.

### Track D - matchID

- Every matchID status must say which replay or fixture was exercised,
  what was compared to OpenSearch, and what remains partial.
- A replay that passes only against Surch-generated expectations is not
  sufficient to declare parity.

### Track E - infra K8s / poc-k8s

- K8s jobs are expected to emit diagnostics, pod logs, and artefacts
  even on failure.
- Cost and timeout guardrails must stay inside the workflows and helper
  scripts, not only in oral process.

### Reporting format for all tracks

- User-facing status and restart reports must use exactly three
  top-level sections:
  - `Fait`
  - `A faire`
  - `Attendus`
- Each section is multi-track and must cover Track A through Track E,
  even if a track only says `RAS`.
- Do not use wide Markdown tables for status reports; prefer short
  bullets with paths, SHAs, run ids, and verdicts inline.
