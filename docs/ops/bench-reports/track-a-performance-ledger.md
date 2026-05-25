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
- `docs/ops/bench-reports/2026-05-21-A-replay-current-main-61a13f-insee-K8s/README.md`:
  stable-ref current-main K8s replay repeated group, 3 successful
  `insee-bench` repetitions on SHA `61a13f8`.
- `docs/ops/bench-reports/2026-05-22-ndcg-gate-7Gi-K8s/README.md`:
  first `ndcg-gate` run that ingests the full 171 k TREC-COVID corpus
  end-to-end on SHA `d9cac15` (Surch 7 GiB cap); supersedes the false
  green from `2026-05-20-ndcg-gate-K8s` for the TREC-COVID rows.
- `docs/ops/bench-reports/2026-05-23-ndcg-gate-7Gi-RSS-K8s/README.md`:
  first `ndcg-gate` run carrying a paired `surch.bench.rss.v1` RSS
  envelope on SHA `137b352` (after the `argv[0]`-basename PID fix and
  driver-log marker reconstruction); supersedes the 2026-05-22 report
  for the `Memory / RSS` ledger row.
- `docs/ops/bench-reports/2026-05-24-ndcg-gate-incremental-bulk-K8s/README.md`:
  first `ndcg-gate` run on the incremental bulk path (`367acdc` Lot 1
  axis (c)). Surch TREC-COVID bulk drops from `1001.95 s` to
  `179.86 s` (`~5.6x` speedup), bringing the Surch/OpenSearch ratio
  from `13.9x slower` to `2.06x slower`. Surch RSS rises from `4802
  MiB` to `5859 MiB` (live `PostingsBuilder` snapshot for incremental
  appends) — to be addressed in a follow-up.
- `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.5-ram-K8s/README.md`:
  first `ndcg-gate` run on Lot 1.5 (`8a5150f` — `refresh_index` drops
  the `PostingsBuilder` via `finalize_postings()`, `terms_finalized`
  flag + fallback `rebuild_index` for bulk-after-refresh). Saves
  `268 MiB` (`5859 -> 5591 MiB`) — modest because the glibc allocator
  does not return freed pages without memory pressure; full ~1 GiB
  gain requires an orthogonal allocator-level Lot.
- `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.7-jemalloc-K8s/README.md`:
  first `ndcg-gate` run on Lot 1.7 step B (`b9f6636` — switch the
  Surch global allocator to jemalloc via `tikv-jemallocator` 0.6 +
  `MALLOC_CONF=background_thread:true,dirty_decay_ms:0,muzzy_decay_ms:0`).
  Surch RSS peak `5591 -> 3424 MiB` (`-39 %`), Surch RSS final
  `5591 -> 1382 MiB` (`-75 %` — background-thread purge), Surch
  TREC-COVID bulk `189.18 -> 139.05 s` (`-26 %`). NDCG unchanged.
  Brings Surch under OpenSearch on every memory + every quality
  metric and within `1.42x` on bulk; allocator parity with
  Elasticsearch/OpenSearch achieved.
- `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.6-K8s/README.md`:
  Lot 1.6 (deferred FST term-dictionary build, `2e4361e`) + Lot 2
  (skip lists on FoR postings, `d73c862`) landed together. **Surch
  crosses OpenSearch bulk parity on TREC-COVID**: `139.05 -> 56.38 s`
  (Surch now `1.54x` FASTER than OpenSearch `86.61 s`). Surch RSS
  peak `3424 -> 2156 MiB`. NDCG unchanged. Lot 2 search-latency gain
  not yet quantified (needs `insee-bench` replay).
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
- RSS peak/final is now reportable on K8s `ndcg-gate` and
  `insee-bench` jobs: Track B wires `rss-sample.sh` against the
  argv[0]-resolved engine PIDs, the driver streams each envelope
  between `BEGIN_SURCH_K8S_RSS_FILE:<name>` / `END_…` markers, and
  `ci-k8s.yml` reconstructs `rss-{ndcg,art}-{surch,os}.json` into the
  artifact directory. Rows that have not yet been replayed with this
  harness keep `RSS: not captured by current harness`; rows that
  cite `2026-05-23-ndcg-gate-7Gi-RSS-K8s` may quote the paired RSS
  numbers directly.

## Current axis state

| Axis | Current proof | Surch vs OpenSearch state | Delta / verdict | Missing proof |
| --- | --- | --- | --- | --- |
| Search latency, matchID INSEE 10k | K8s 3-rep baseline `2026-05-21-A-replay-current-main-61a13f-insee-K8s` + Lot 2 skip-list isolation `2026-05-25-insee-lot2-skiplists-K8s` | 3-rep baseline median Surch `2.1/3.6/5.0/22.0 ms` vs OpenSearch `3.9/9.3/16.7/225.6 ms`. Lot 2 isolation (single run each, same jemalloc stack): control `b9f6636` Surch `1.6/3.9/7.9/68.3 ms` -> Lot 2 `d73c862` Surch `1.6/3.4/6.5/64.1 ms` | Surch `2.6–2.8x` faster than OpenSearch p50/p95. Lot 2 skip lists improve the Surch tail `p95 -13% / p99 -18%`, p50 flat (leapfrog AND helps the multi-term tail, not the median) | Lot 2 isolation is single-run per SHA; 3-rep paired run would tighten the CI. RSS now captured (Surch `75 MB` on INSEE 10k) |
| FoR metadata wiring | K8s before run `26151880297` vs after run `26101404966` | Surch before `2.4/4.6/7.8/25.6 ms`; after `1.9/3.6/6.9/17.9 ms` | Surch hot path improved `-21%/-22%/-12%/-30%` p50/p95/p99/max | Bulk timing and RSS are not isolated for this commit |
| BAN Paris 25k latency | Local report `2026-05-16-vs-os-2.17.1` | Surch `took` p50 sub-ms, p95 `20 ms`, max `20 ms`; OpenSearch p50 `20 ms`, p95 `108 ms`, max `108 ms` | Surch is `>20x` faster at p50 and `5.4x` faster at p95/max | Needs K8s rerun if this becomes a release gate |
| Bulk indexing | Local report `2026-05-16-vs-os-2.17.1` plus K8s `ndcg-gate` runs `26304471549` (`d9cac15`, full-rebuild baseline), `26340177506` (`137b352`, paired RSS), `26350556060` (`04fde72`, incremental bulk Lot 1 fix), `26359069219` (`01ad77e`, Lot 1.5 RAM), `26360701909` (`b9f6636`, Lot 1.7 jemalloc) | Local SciFact: Surch `3.545 s`, OpenSearch `17.612 s`; K8s SciFact across runs: Surch `1.70–3.66 s`, OpenSearch `7.84–13.44 s` (Surch `2.1–7.9x` faster, best with jemalloc); K8s TREC-COVID `d9cac15`/`137b352` (pre-Lot-1): Surch `1001.95 / 1112.52 s`, OpenSearch `72.27 / 93.80 s` (OpenSearch `11.9–13.9x` faster); K8s TREC-COVID `04fde72` (Lot 1): Surch `179.86 s`, OpenSearch `87.04 s` (OpenSearch `2.06x` faster); K8s TREC-COVID `01ad77e` (Lot 1.5): Surch `189.18 s`, OpenSearch `98.66 s` (OpenSearch `1.92x` faster); K8s TREC-COVID `b9f6636` (Lot 1.7 jemalloc): Surch `139.05 s`, OpenSearch `97.83 s` (OpenSearch `1.42x` faster); K8s TREC-COVID `2e4361e` (Lot 1.6 deferred FST + Lot 2): Surch `56.38 s`, OpenSearch `86.61 s` (**Surch `1.54x` FASTER**) | Surch wins SciFact bulk consistently (`+8.1x`). **TREC-COVID bulk parity crossed**: from `13.9x slower` (pre-Lot-1) to `1.54x faster` than OpenSearch — total `~17.8x` Surch speedup (`1002 -> 56 s`). The Lot 1.6 deferred FST build removed the per-`_bulk` cumulative `terms.build()` | Lots 1, 1.5, 1.6, 1.7 closed; Surch now beats OpenSearch on bulk |
| Quality guardrail | Local SciFact report `2026-05-16-vs-os-2.17.1` plus K8s `ndcg-gate` runs `26304471549` (`d9cac15`) and `26340177506` (`137b352`) | SciFact: Surch NDCG@10 `0.6576`, Recall@10 `0.8100` vs OpenSearch `0.6537` / `0.8033` (Surch `+0.6%` / `+0.8%`, identical across the two runs). TREC-COVID full corpus: Surch NDCG@10 `0.4750`, Recall@10 `0.0132` vs OpenSearch `0.4902` / `0.0132` (Surch `-3.1%` NDCG@10, Recall@10 tied, identical across the two runs) | SciFact floor `NDCG@10 >= 0.65` held; TREC-COVID is a reproducible cross-engine BEIR baseline with Surch trailing OpenSearch by `0.0152` NDCG@10 | Diagnose the Surch TREC-COVID NDCG@10 gap before claiming a BEIR quality win; SciFact stays the active acceptance gate |
| RSS / memory | `docs/ops/memory-capacity.md` + K8s pod limits + K8s `ndcg-gate` runs `26340177506` (`137b352`, full-rebuild), `26350556060` (`04fde72`, incremental bulk), `26359069219` (`01ad77e`, Lot 1.5 RAM), `26360701909` (`b9f6636`, Lot 1.7 jemalloc) | Capacity model: BAN 25k ~85 MB, INSEE 1.3M ~4.5 GB. K8s BEIR full 171 k TREC-COVID paired sampling (1 Hz, 1200 s) Surch peak / final: `4802 / 4802` (full-rebuild) -> `5859 / 5859` (Lot 1) -> `5591 / 5591` (Lot 1.5 glibc) -> `3424 / 1382` (Lot 1.7 jemalloc) -> **`2156 / 1290` (Lot 1.6 deferred FST + Lot 2)**. OpenSearch peak essentially unchanged `1395 … 1466 MiB`. Lot 1.6 removed the per-chunk cumulative `PostingsBuilder.clone().build()`, lowering the transient bulk peak on top of the jemalloc purge | Surch full-corpus footprint is now `~1.47x` OpenSearch peak (was `~3.8x` on Lot 1.5). Allocator parity with Elasticsearch/OpenSearch achieved | INSEE-side replay still needs a paired RSS rerun |
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
| `61a13f8` stable current-main repeated group | Search latency / SLO / K8s monitoring | Proven by `2026-05-21-A-replay-current-main-61a13f-insee-K8s` | 3 K8s runs `26202652997`, `26203320060`, `26204062094`; artifact ids `7126549971`, `7126727126`, `7126979242`; median Surch `2.1/3.6/5.0/22.0 ms` vs OpenSearch `3.9/9.3/16.7/225.6 ms`; 0 errors throughout; pod samples captured; RSS still not captured |
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
- Stable-ref current-main repeated K8s replay:
  `perf-replay/current-main-61a13f` pins
  `61a13f871f810c98379375f2c94a10bbc696ac6e`. Runs `26202652997`,
  `26203320060`, and `26204062094` all passed with
  `SuccessCriteriaMet=True` and `Complete=True`; artifacts
  `7126549971`, `7126727126`, and `7126979242` are promoted under
  `docs/ops/bench-reports/2026-05-21-A-replay-current-main-61a13f-insee-K8s/`.
  This closes the first final repeated current-main Track A replay
  verdict.

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
  INSEE 10k captures. The INSEE 10k current-main claim now has a
  repeated 3-run K8s group on stable SHA `61a13f8`.
- Track A does not yet have a complete per-commit attribution report for
  every historical optimisation. The durable per-commit proof starts at
  the FoR metadata wiring report unless we replay older SHAs.
- RSS and disk wins must stay out of release language until Track B adds
  paired RSS reporting and the disk-format work produces a measured
  artifact.
