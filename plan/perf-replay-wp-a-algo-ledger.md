# perf-replay/wp-a-algo-ledger Plan

Track principal: A - perf / optimisation
Branch: `perf-replay/wp-a-algo-ledger`
Worktree: `/home/antoinefa/src/surch`
Owner: `#1/#2 worker Track A`
Status: started on 2026-05-20; local branch only, not pushed. First
current-main K8s replay was dispatched as `ci-k8s` run `26193166785`;
verdict PASS, promoted report:
`docs/ops/bench-reports/2026-05-20-A-replay-current-main-insee-K8s/`.

## Finality

- [x] Start a cumulative replay trace from the live Track A ledger.
- [ ] Promote human replay reports for A-replay-1 through A-replay-3.
- [ ] Update the Track A ledger rows with run ids, artifacts, latency,
  quality, and RSS evidence as each replay lands.
- [ ] Keep the historical Track A commits intact; no rebase, amend, or
  history rewrite is part of this line.

## Scope

- [x] Read `PLAN.md`, `plan/wp-a-optim.md`, and the Track A performance
  ledger before changing the trace.
- [x] Verify branch/worktree state and current `ci` / `ci-k8s` /
  `docker-build` status.
- [x] Check whether existing GHCR tags make an immediate K8s dispatch
  valid.
- [ ] Produce replay reports under
  `docs/ops/bench-reports/<date>-A-replay-*/` when runs complete.

## Hors scope

- [x] No Track D files.
- [x] No pushes without an explicit conductor instruction.
- [x] No synthetic rewrite of old optimisation commits.
- [x] No claim of RSS, disk, or quality improvement without a cited
  artifact.

## Merge State

- [x] Local branch created from `origin/main`:
  `466693f55e1a3cd8b007e058be07584251986ecb`.
- [ ] Branch pushed to origin.
- [x] Current-main replay report committed.
- [x] Ledger updated from the current-main replay artifact.
- [ ] Historical replay reports committed.
- [ ] Main integration requested by conductor.

## Replay Points

These points come from
`docs/ops/bench-reports/track-a-performance-ledger.md`. `origin/main`
contains the historical SHAs, but most are not independently addressable
as remote branch heads or tags today.

| Lot | Baseline SHA | Head SHA | Remote ref usable today | Dispatch state |
| --- | --- | --- | --- | --- |
| A-replay-1 top-K / lazy hydration | `71ceb2755ad33d4cc1b8d8da0003ae876edc228f` | ledger head `3157afbae0f2d37ac3d92462f08b92f6b6dee317`; TopN point `5081cc74a961c2fe67eec9c7fee8bbc3df86019b` / merge `eaff76cbbbefca55fb6d498f342f0e31e553cfa9` | reachable through `origin/main`, no dedicated remote ref | GHCR tags missing; no `docker-build.yml` / `ci-k8s.yml` at these SHAs, so direct workflow dispatch is blocked |
| A-replay-2 WAND family | `3157afbae0f2d37ac3d92462f08b92f6b6dee317` | `e38bf916a0f197e0bb4f63e50ee9efc10cf3c704` | reachable through `origin/main`, no dedicated remote ref | GHCR tags missing; no `docker-build.yml` / `ci-k8s.yml` at these SHAs, so direct workflow dispatch is blocked |
| A-replay-3 memory layout | `65fc7599946a2e5e1d81b989a1fb6606fc2d7a21` | `7caf3397970d9a183ebc5bc7631cb2e9f0aaea5c` | baseline is `origin/wp/b-test-auto`; head reachable through `origin/main`, no dedicated head ref | GHCR tags missing; baseline has workflow files, head lacks `docker-build.yml`; direct paired dispatch is blocked |
| Current cumulative smoke | `69240116599e8e86f629f13f3d7611d73ff1a07d` | `466693f55e1a3cd8b007e058be07584251986ecb` | `origin/main` | `insee-bench` run `26193166785` PASS; promoted report `2026-05-20-A-replay-current-main-insee-K8s` |

The A-replay-1 row in the existing ledger is non-linear as written:
`3157afb` is an ancestor of `5081cc7`, so `71ceb275 -> 3157afb`
isolates lazy hydration while TopN scalar finalization needs the
additional `5081cc7` or `eaff76c` replay point. The trace keeps both
facts instead of editing old history.

## Replay proof protocol

The replay line is a performance proof, not a one-off smoke. Each
A-replay-1..3 point must use the same K8s harness family that produces
the promoted `ci-k8s` reports, with enough repeated measurements and
environment evidence to make the comparison meaningful.

- [ ] Run every accepted replay point in K8s, not as a local perf proof.
- [ ] Execute at least 3 successful repetitions of the same workload for
  each compared ref before publishing a final verdict.
- [ ] Keep each repetition as a separate `ci-k8s` run id and artifact;
  do not collapse reruns into a single undocumented number.
- [ ] Record the exact runtime image and bench-driver image tags:
  `ghcr.io/rhanka/surch:sha-<full_sha>` and
  `ghcr.io/rhanka/surch:bench-sha-<full_sha>`.
- [ ] Record cluster and pod shape for every replay group: namespace,
  node pool, node type when visible, quota/limit range, pod requests and
  limits, activeDeadlineSeconds, mounted PVCs, and corpus generation
  source.
- [ ] Capture cluster monitoring around each run group: Job conditions,
  pod phases, container restarts, `kubectl describe job`, pod describe,
  namespace events, and node/pod resource snapshots when available.
- [ ] Publish aggregate run statistics for Surch and the reference
  engine: p50 / p95 / p99 / max per repetition, then median and IQR
  across the three repetitions. If IQR cannot be computed from the
  report shape, publish min / median / max and state that limitation.
- [ ] Keep SLO verdicts fail-closed: any repetition with benchmark
  errors, missing artifact, failed Job condition, missing config
  evidence, or untracked image tag makes the replay group `invalid`
  until rerun.
- [ ] Do not count `ci-k8s` run `26200481514` as one of the final
  repeated A-replay proofs: it is a useful K8s smoke with a passing
  benchmark, but the pod metrics were unavailable post-completion and
  node metrics were forbidden for the workflow service account. Count
  only repetitions produced after live top sampling is present.
- [ ] For search-ranking-sensitive changes, pair the latency replay with
  `ndcg-gate` or a promoted quality artifact; report NDCG@10 and
  Recall@10 beside latency before claiming the optimisation safe.
- [ ] Do not claim RSS peak/final in A reports until Track B has wired
  paired RSS capture into the same K8s replay artifact.

Required artifact set per replay group:

- [ ] One human report directory under
  `docs/ops/bench-reports/<date>-A-replay-<n>-<context>-K8s/`.
- [ ] A `README.md` or `summary.md` that lists all repetition run ids,
  artifact ids, refs, image tags, pod requests/limits, cluster quota,
  workload size, SLO verdict, and aggregation method.
- [ ] Raw diagnostics from each run preserved through the GitHub
  artifact: GHA summary, Job YAML, Job describe, pod describe, events,
  driver logs, benchmark summary, and machine JSON when produced.
- [ ] Ledger updates in
  `docs/ops/bench-reports/track-a-performance-ledger.md` that cite the
  promoted report and explicitly mark missing RSS if Track B has not
  landed the RSS producer yet.

Reference strategy, without history rewrite:

- [ ] Prefer durable replay refs such as
  `perf-replay/a-replay-<n>-base` and
  `perf-replay/a-replay-<n>-head` that point at the selected historical
  code plus the minimum benchmark harness plumbing needed for K8s.
- [ ] If a historical SHA lacks `docker-build.yml`, `ci-k8s.yml`, or K8s
  manifests, create a modern replay branch that preserves the historical
  application code and cherry-picks only harness/CI plumbing; document
  the harness delta in the promoted report.
- [ ] Do not amend, rebase, or replace historical Track A commits to
  make them dispatchable.
- [ ] Build and verify the runtime and bench-driver GHCR tags for every
  selected replay ref before dispatching the three K8s repetitions.

Track B coordination:

- [ ] Treat paired RSS peak/final as a Track B prerequisite for any
  memory-win claim in A-replay-3.
- [ ] When RSS lands, include Surch and Elasticsearch/OpenSearch peak
  RSS, final RSS, sampling interval, process selection method, and pod
  memory limit in the same promoted A replay report.
- [ ] Until that lands, A-replay reports must say `RSS: not captured by
  current harness` and keep the verdict limited to latency, SLO, quality,
  and indexing axes actually measured.

## Lots

- [x] Lot 0 - Intake and safety checks
  - [x] Read live Surch plans and Track A ledger.
  - [x] Confirm dirty state: only untracked `handover.md`, not touched.
  - [x] Confirm current `main` / `origin/main` head:
    `466693f55e1a3cd8b007e058be07584251986ecb`.
  - [x] Confirm latest relevant runs:
    `ci` `26188555428` in progress, `docker-build` `26188578411`
    success, latest completed `ci-k8s` `26157480132` success on
    `6924011`.

- [x] Lot 1 - Replay SHA inventory
  - [x] Resolve parent/head SHAs for A-replay-1 through A-replay-3.
  - [x] Record which SHAs have workflow files and K8s manifests.
  - [x] Record that historical GHCR tags are missing except the
    existing FoR anchor `c01b0a2`.

- [x] Lot 2 - First dispatchable K8s run
  - [x] Wait for `docker-build` `26188578411` to finish.
  - [x] Recheck GHCR tags:
    `ghcr.io/rhanka/surch:sha-466693f55e1a3cd8b007e058be07584251986ecb`
    and
    `ghcr.io/rhanka/surch:bench-sha-466693f55e1a3cd8b007e058be07584251986ecb`.
  - [x] Dispatch:
    `gh workflow run ci-k8s.yml --ref main -f job=insee-bench`.
  - [x] Record run id: `ci-k8s` `26193166785`.
  - [x] Record this session's dispatch blocker: sandboxed `gh` returned
    `error connecting to api.github.com`; out-of-sandbox dispatch
    approval timed out twice before a later approved dispatch succeeded.
  - [x] Record verdict: PASS, `SuccessCriteriaMet=True`,
    `Complete=True`, artifact `7123081611`.
  - [x] Promote report:
    `docs/ops/bench-reports/2026-05-20-A-replay-current-main-insee-K8s/`.

- [ ] Lot 3 - Historical replay enablement
  - [ ] For each replay point, choose a non-rewrite remote ref strategy:
    temporary replay branches, or a modern replay branch that
    cherry-picks only K8s harness plumbing around the historical code.
  - [ ] Build missing runtime and bench-driver images for the selected
    refs before dispatching K8s.
  - [ ] Run at least 3 successful K8s repetitions per selected replay
    ref and workload before publishing a final A-replay verdict.
  - [ ] Capture cluster/pod/image configuration and monitoring
    diagnostics for each replay group.
  - [ ] Aggregate repeated latency with median and IQR, or
    min/median/max when IQR is not derivable from the artifacts.
  - [ ] Coordinate with Track B before claiming RSS peak/final; otherwise
    mark RSS explicitly missing in A reports and ledger rows.
  - [ ] Promote `summary.md` / benchmark artifacts and update the
    ledger rows in place.

## Gates

- [x] Read-only gates completed: git state, worktrees, plans, workflow
  surfaces, GHCR tag checks.
- [x] K8s gate: `ci-k8s.yml` `job=insee-bench` on first image-ready ref.
- [ ] Quality gate where search ranking changes are compared:
  `ci-k8s.yml` `job=ndcg-gate`.
- [x] Report gate: human `README.md` or `summary.md` promoted with
  p50 / p95 / p99 / max, errors, SLO verdict, and quality/RSS fields
  when available.
- [ ] Repeatability gate: at least 3 successful K8s repetitions are
  cited for every final A-replay-1..3 verdict.
- [ ] Environment gate: cluster, pod, image, quota, PVC, and monitoring
  diagnostics are cited in the promoted replay report.
- [ ] Significance gate: repeated runs are summarized as median/IQR or
  min/median/max and any outlier or failed repetition is called out.

## Proofs

- [x] `gh run view 26188578411` showed `docker-build` success for
  `466693f55e1a3cd8b007e058be07584251986ecb`.
- [x] GHCR manifest checks showed current and A-replay-1..3 tags
  missing, while FoR anchor `c01b0a2` runtime and bench-driver tags are
  present.
- [x] `git cat-file` checks showed A-replay-1 and A-replay-2 SHAs lack
  the current workflow/K8s harness surface required for direct dispatch.
- [x] GHCR manifest checks showed current main runtime and bench-driver
  tags present after `docker-build` `26188578411`.
- [x] First K8s replay run id recorded: `26193166785`.
- [x] First K8s replay verdict recorded: PASS.
