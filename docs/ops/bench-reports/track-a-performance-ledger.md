# Track A performance ledger

This file is the restartable state for Track A performance claims. It
does not replace the raw reports; it points to them and records which
performance axes are proven, which are only estimated, and which still
need a fresh run.

## Source reports

- `docs/ops/bench-reports/2026-05-16-vs-os-2.17.1/README.md`:
  local paired Surch vs OpenSearch 2.17.1 baseline on SciFact and BAN
  Paris 25k.
- `docs/ops/bench-reports/2026-05-19-insee-10k-k8s/README.md`:
  K8s `insee-bench` run on the Scaleway burst pool, Surch vs
  OpenSearch 2.17.1, INSEE 10k matchID-style artillery scenario.
- `docs/ops/bench-reports/2026-05-20-A-lot3-paired-K8s/README.md`:
  before/after K8s perf proof for the FoR block metadata search wiring.
- `docs/ops/bench-reports/2026-05-20-A-replay-current-main-insee-K8s/README.md`:
  current-main K8s replay kickoff for the cumulative per-algorithm
  proof line.
- `docs/ops/bench-reports/2026-05-21-A-replay-current-main-insee-K8s-rep1/README.md`:
  post-wait-loop-fix current-main K8s replay repetition `1/3`, with
  live pod monitoring samples promoted next to the benchmark summary.
- `docs/ops/memory-capacity.md`: current RAM capacity model and stats
  endpoint contract.

The reference engine in these reports is OpenSearch 2.17.1. Track D is
separate: matchID's active oracle target is Elasticsearch 8.6.1.

## Required proof policy for new Track A algorithms

Every new Track A algorithm delivery must append to this ledger before
it is called delivered. The row is cumulative and must cite the exact
commit or range, the workload, the execution environment, the reference
engine when relevant, run ids, promoted artifacts, and a verdict.

Fast iterative merges are allowed, but the merge must be honest about
the proof state:

- If the algorithm is merged before the heavy K8s run, add a ledger row
  with `Proof state = pending K8s replay`, the commit SHA, and the exact
  replay command or GitHub workflow to run.
- When the heavy run finishes, commit the promoted report and update the
  same row instead of replacing history.
- Do not claim a p50/p95/p99/max, RSS, disk, or quality win unless that
  axis has a cited artifact.
- If a later replay covers several historical algorithms, keep the
  individual rows and cite the shared report in each row that it
  substantiates.
- For final Track A replay verdicts, K8s is mandatory. Local runs can
  diagnose, but they do not close A-replay-1..3.
- A final A replay must include at least 3 successful repetitions of the
  same K8s workload for each compared ref. The promoted report must list
  every run id and artifact id.
- The report must document the measurement environment: runtime image
  tag, bench-driver image tag, namespace, node pool, pod requests and
  limits, quota/limit range, PVCs, corpus source, activeDeadlineSeconds,
  and reference engine version.
- The report must preserve cluster monitoring evidence: Job conditions,
  Job describe, pod describe, events, container restarts, driver logs,
  and node/pod resource snapshots when the cluster exposes them.
- Repeated runs must be summarized as median and IQR across repetitions;
  if the available artifact shape cannot support IQR, publish
  min/median/max and state that limitation.
- A replay group is invalid until rerun if any repetition has benchmark
  errors, missing diagnostics, missing image/config evidence, a failed
  K8s condition, or a missing SLO verdict.
- RSS peak/final remains a Track B reporting prerequisite. Until the
  RSS producer is wired into the same K8s artifacts, A rows must say
  `RSS: not captured by current harness` instead of implying a memory
  win.

## Current axis state

| Axis | Current proof | Surch vs OpenSearch state | Delta / verdict | Missing proof |
| --- | --- | --- | --- | --- |
| Search latency, matchID INSEE 10k | K8s run `26101404966` | Surch p50/p95/p99/max `1.9/3.6/6.9/17.9 ms` vs OpenSearch `3.8/9.9/20.8/135.3 ms` | Surch is `2.0x/2.7x/3.0x/7.5x` faster; 0 errors on both engines | Repeat run distribution if we need confidence intervals, not just one green proof |
| FoR metadata wiring | K8s before run `26151880297` vs after run `26101404966` | Surch before `2.4/4.6/7.8/25.6 ms`; after `1.9/3.6/6.9/17.9 ms` | Surch hot path improved `-21%/-22%/-12%/-30%` p50/p95/p99/max | Bulk timing and RSS are not isolated for this commit |
| BAN Paris 25k latency | Local report `2026-05-16-vs-os-2.17.1` | Surch `took` p50 sub-ms, p95 `20 ms`, max `20 ms`; OpenSearch p50 `20 ms`, p95 `108 ms`, max `108 ms` | Surch is `>20x` faster at p50 and `5.4x` faster at p95/max | Needs K8s rerun if this becomes a release gate |
| Bulk indexing | Local report `2026-05-16-vs-os-2.17.1` plus K8s `ndcg-gate` run `26157480132` | Local SciFact: Surch `3.545 s`, OpenSearch `17.612 s`; local BAN 25k: Surch `17.882 s`, OpenSearch `21.707 s`; K8s SciFact: Surch `4.098 s`, OpenSearch `12.088 s`; K8s TREC-COVID: Surch `5.116 s`, OpenSearch `28.711 s` | Surch is faster in all cited bulk captures (`3.0x` to `5.6x` on the K8s BEIR run) | No current bulk-only run with paired RSS after the FoR/FST/source-sharing sequence |
| Quality guardrail | Local SciFact report `2026-05-16-vs-os-2.17.1` plus K8s `ndcg-gate` run `26157480132` | SciFact held on K8s: Surch NDCG@10 `0.6576`, Recall@10 `0.8100`; OpenSearch NDCG@10 `0.6537`, Recall@10 `0.8033`. TREC-COVID failed for Surch: NDCG@10 `0.0000`, Recall@10 `0.0000`; OpenSearch was `0.1141` / `0.0026` | SciFact floor `NDCG@10 >= 0.65` held; TREC-COVID is a blocker, not a win | Diagnose TREC-COVID request/index mismatch before making it a quality gate |
| RSS / memory | `docs/ops/memory-capacity.md` + K8s pod limits | Model says BAN 25k is about `85 MB`; INSEE 1.3M projects to about `4.5 GB`; K8s INSEE 10k pod cap was Surch `512Mi` | Capacity model exists, but it is not a Surch-vs-OpenSearch RSS delta | Add paired RSS reporting under Track B and promote a report with peak/final RSS |
| Disk / encoded format | Track A delivered codec/FoR metadata groundwork | No production on-disk postings format is shipped yet | Disk delta is not claimable today | Defer to the follow-up codec/disk-format plan; do not report a disk win until measured |
| Error-rate / SLO | K8s run `26101404966` and local SciFact/BAN reports | INSEE 10k: both engines `0/13170` errors; p95 SLO `<= 200 ms`, max SLO `<= 500 ms` | PASS for Surch and OpenSearch; Surch has much larger headroom | Keep this in every promoted perf report |

## Progress ledger by delivered optimisation

| Commit / range | Axis | Proof state | Notes |
| --- | --- | --- | --- |
| `5081cc7` scalar top-K finalization | Search latency | Historical only | Needs replay if we want an isolated proof row; current cumulative proof starts later |
| `3157afb` lazy `_source` hydration | Search latency / allocation pressure | Historical only | Needs replay with hydration-heavy query set and RSS peak/final |
| `ed76014` MaxScore/WAND OR-match skipping | Search latency | Historical only | Needs isolated replay against the pre-WAND parent and SciFact quality guardrail |
| `65ccfbe` WAND `multi_match` + postings-builder cleanup | Search latency / RAM | Historical only | Needs replay with `multi_match` workload and RSS peak/final |
| `e38bf91` Block-Max WAND per-128 contributions | Search latency / quality | Historical only | Needs replay with SciFact NDCG@10 and INSEE/BAN latency |
| `644f62b` per-index LRU search response cache | Warm search latency | Historical only | Needs cold/warm split report; cache invalidation proof remains in tests |
| `4e9405a` + merge `f910094` shared stored sources | RAM | Historical only | Needs paired RSS report before any release memory-win language |
| `c5f3155` + merge `0800f98` FST term dictionary | Prefix / term lookup latency + RAM | Historical only | Needs prefix-heavy report and RSS peak/final |
| `b680232` + merge `6df877d` per-block stats persisted next to postings | Search latency groundwork | Historical only | Covered indirectly by FoR metadata proof, not isolated |
| `b8ed2bc` + merge `7caf339` memory metrics and stats endpoint | Observability | Tested API surface, not a perf win | Use this endpoint as evidence source for future RSS/memory rows |
| `651e22a`, `4e9405a`, `c5f3155`, `b680232`, `b8ed2bc` | RAM / memory model | Capacity model exists; individual RSS wins are not fully paired against OpenSearch | Future memory claims must include peak/final RSS from the same harness and same pod/host shape |
| `df3b0aa` plus supporting index/codec commits `6f56fd2`, `2da9249` | Search latency | Proven by `2026-05-20-A-lot3-paired-K8s` | This is the only current isolated before/after Track A perf proof with K8s run ids |
| `466693f` current-main replay anchor | Search latency / SLO | Proven by `2026-05-20-A-replay-current-main-insee-K8s` | Surch `2.0/3.7/5.4/36.8 ms` p50/p95/p99/max vs OpenSearch `4.5/9.7/18.2/362.8 ms`; 0 errors on both engines; RSS not captured |
| `ac558e6` current-main replay repetition | Search latency / SLO / K8s monitoring | Repetition `1/3` accepted by `2026-05-21-A-replay-current-main-insee-K8s-rep1` | `ci-k8s` run `26202012197`, artifact `7126271947`; Surch `1.9/3.5/5.0/25.0 ms` vs OpenSearch `4.5/9.3/16.3/354.1 ms`; 0 errors; live pod max samples Surch `91m/77Mi`, OpenSearch `1200m/1476Mi`; RSS still not captured |
| Future Track A commits | Any perf axis | Must update this ledger in the same PR/commit sequence as the optimisation proof | Required fields: axis, workload, Surch numbers, OpenSearch/Elasticsearch reference where relevant, delta, run id/artifact path, and missing proof |

## Replay backlog for historical algorithms

Preferred replay mode: keep current history, create a dedicated
`perf-replay/wp-a-algo-ledger` branch, run K8s benchmarks at selected
historical SHAs, promote each report, then append rows here. This avoids
rewriting already integrated code while producing the cumulative proof
trail that future releases need.

Replay refs are allowed to add current benchmark harness plumbing around
historical application code, but they must not rewrite historical Track A
commits. When a selected SHA lacks the current GitHub Actions, Docker, or
K8s surfaces, create durable replay refs for the baseline/head pair,
document the harness-only delta, build both GHCR tags, and then dispatch
the repeated K8s runs from those refs.

Kickoff trace, 2026-05-20:

- Local branch `perf-replay/wp-a-algo-ledger` was created from
  `origin/main` at `466693f55e1a3cd8b007e058be07584251986ecb`; it is
  not pushed.
- Detailed replay plan:
  `plan/perf-replay-wp-a-algo-ledger.md`.
- Current image gate: `docker-build` run `26188578411` succeeded for
  `466693f55e1a3cd8b007e058be07584251986ecb`, and both
  `ghcr.io/rhanka/surch:sha-466693f55e1a3cd8b007e058be07584251986ecb`
  and
  `ghcr.io/rhanka/surch:bench-sha-466693f55e1a3cd8b007e058be07584251986ecb`
  exist.
- Historical image gate: GHCR tags for the A-replay-1..3 baseline/head
  SHAs were missing at kickoff. The existing FoR anchor
  `c01b0a297b73594be2a7f01275cc76e56b82ad08` is the only checked
  historical SHA with both runtime and bench-driver tags present.
- First current-main K8s replay dispatch:
  `gh workflow run ci-k8s.yml --ref main -f job=insee-bench` created
  `ci-k8s` run `26193166785` on `main`
  (`466693f55e1a3cd8b007e058be07584251986ecb`). Verdict: PASS, artifact
  `k8s-bench-insee-bench-466693f55e1a3cd8b007e058be07584251986ecb`
  (`7123081611`), promoted report
  `docs/ops/bench-reports/2026-05-20-A-replay-current-main-insee-K8s/`.
- First post-fix repeated current-main K8s replay:
  `make bench-k8s K8S_JOB=insee-bench K8S_REF=main` created
  `ci-k8s` run `26202012197` on `main`
  (`ac558e6d08c7566f8cbc0b96c56a5b943eb1ae79`). Verdict: PASS,
  artifact
  `k8s-bench-insee-bench-ac558e6d08c7566f8cbc0b96c56a5b943eb1ae79`
  (`7126271947`), digest
  `sha256:274ed630818f02fa12cfdc85c76112d2dc6db472d1fe947b11cf8edfdeb75994`,
  promoted report
  `docs/ops/bench-reports/2026-05-21-A-replay-current-main-insee-K8s-rep1/`.
  This counts as `1/3` for the current-main repeated run group, not as a
  final verdict.

Minimum replay set:

| Replay lot | Baseline -> head | Workload | Required proof |
| --- | --- | --- | --- |
| A-replay-1 top-K / lazy hydration | parent before `5081cc7` -> `3157afb` | BAN + INSEE smoke | p50/p95/p99/max, errors, docs/s, RSS if available |
| A-replay-2 WAND family | parent before `ed76014` -> `e38bf91` | SciFact + INSEE | NDCG@10, Recall@10, p50/p95/p99/max |
| A-replay-3 memory layout | parent before `4e9405a` -> `7caf339` | BAN 25k + INSEE 10k | RSS peak/final, stats endpoint snapshot, latency non-regression |
| A-replay-4 FoR metadata | pre-FoR `c01b0a2` -> `df3b0aa` / `c5980ad` | INSEE K8s | Already proven by `2026-05-20-A-lot3-paired-K8s`; keep as anchor |

Minimum repeated-run proof for A-replay-1..3:

- [ ] Dispatch at least 3 successful K8s repetitions per baseline ref.
- [ ] Dispatch at least 3 successful K8s repetitions per head ref.
- [ ] Promote one human report directory per replay lot with all run ids,
  artifact ids, image tags, cluster/pod config, monitoring diagnostics,
  and aggregation method.
- [ ] Record Surch and reference p50 / p95 / p99 / max for each
  repetition and the cross-run median/IQR or min/median/max.
- [ ] Add NDCG@10 and Recall@10 for A-replay-2 or any replay that moves
  ranking-sensitive search execution.
- [ ] Add RSS peak/final only after Track B emits paired RSS in the same
  run artifacts; otherwise mark RSS explicitly missing.

## Operator verdict

- Track A has enough evidence to say the current Surch hot path is
  faster than OpenSearch 2.17.1 on the promoted SciFact, BAN 25k, and
  INSEE 10k captures.
- Track A does not yet have a complete per-commit attribution report for
  every historical optimisation. The durable per-commit proof starts at
  the FoR metadata wiring report unless we replay older SHAs.
- RSS and disk wins must stay out of release language until Track B adds
  paired RSS reporting and the disk-format work produces a measured
  artifact.
