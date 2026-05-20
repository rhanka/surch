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
