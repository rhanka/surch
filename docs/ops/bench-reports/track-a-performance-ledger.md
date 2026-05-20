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
- `docs/ops/memory-capacity.md`: current RAM capacity model and stats
  endpoint contract.

The reference engine in these reports is OpenSearch 2.17.1. The Track D
oracle work is separate and uses Elasticsearch 7.x because matchID needs
that wire-compatibility baseline.

## Current axis state

| Axis | Current proof | Surch vs OpenSearch state | Delta / verdict | Missing proof |
| --- | --- | --- | --- | --- |
| Search latency, matchID INSEE 10k | K8s run `26101404966` | Surch p50/p95/p99/max `1.9/3.6/6.9/17.9 ms` vs OpenSearch `3.8/9.9/20.8/135.3 ms` | Surch is `2.0x/2.7x/3.0x/7.5x` faster; 0 errors on both engines | Repeat run distribution if we need confidence intervals, not just one green proof |
| FoR metadata wiring | K8s before run `26151880297` vs after run `26101404966` | Surch before `2.4/4.6/7.8/25.6 ms`; after `1.9/3.6/6.9/17.9 ms` | Surch hot path improved `-21%/-22%/-12%/-30%` p50/p95/p99/max | Bulk timing and RSS are not isolated for this commit |
| BAN Paris 25k latency | Local report `2026-05-16-vs-os-2.17.1` | Surch `took` p50 sub-ms, p95 `20 ms`, max `20 ms`; OpenSearch p50 `20 ms`, p95 `108 ms`, max `108 ms` | Surch is `>20x` faster at p50 and `5.4x` faster at p95/max | Needs K8s rerun if this becomes a release gate |
| Bulk indexing | Local report `2026-05-16-vs-os-2.17.1` | SciFact: Surch `3.545 s`, OpenSearch `17.612 s`. BAN 25k: Surch `17.882 s`, OpenSearch `21.707 s` | SciFact about `5.0x` faster; BAN about `22%` faster in the report wording | No current K8s bulk-only run after the FoR/FST/source-sharing sequence |
| Quality guardrail | Local SciFact report `2026-05-16-vs-os-2.17.1` | Surch NDCG@10 `0.6576`, Recall@10 `0.8100`; OpenSearch NDCG@10 `0.6537`, Recall@10 `0.8033` | Surch is `+0.6%` NDCG@10 and `+0.8%` Recall@10 vs OpenSearch on this capture; floor `NDCG@10 >= 0.65` held | TREC-COVID K8s promotion still blocked until the quota bump is applied and `ndcg-gate` reruns |
| RSS / memory | `docs/ops/memory-capacity.md` + K8s pod limits | Model says BAN 25k is about `85 MB`; INSEE 1.3M projects to about `4.5 GB`; K8s INSEE 10k pod cap was Surch `512Mi` | Capacity model exists, but it is not a Surch-vs-OpenSearch RSS delta | Add paired RSS reporting under Track B and promote a report with peak/final RSS |
| Disk / encoded format | Track A delivered codec/FoR metadata groundwork | No production on-disk postings format is shipped yet | Disk delta is not claimable today | Defer to the follow-up codec/disk-format plan; do not report a disk win until measured |
| Error-rate / SLO | K8s run `26101404966` and local SciFact/BAN reports | INSEE 10k: both engines `0/13170` errors; p95 SLO `<= 200 ms`, max SLO `<= 500 ms` | PASS for Surch and OpenSearch; Surch has much larger headroom | Keep this in every promoted perf report |

## Progress ledger by delivered optimisation

| Commit / range | Axis | Proof state | Notes |
| --- | --- | --- | --- |
| `0dc30ad`, `1b2e380`, `3157afb`, `ed76014`, `d778ee1`, `65ccfbe`, `8757288`, `14b7118`, `e38bf91` | Search latency | Historical effects are listed in `docs/ops/workpackages.md`, but not every commit has an isolated before/after artifact | If a final report needs per-commit attribution, replay from these SHAs in a dedicated branch and promote one report per replay |
| `651e22a`, `4e9405a`, `c5f3155`, `b680232`, `b8ed2bc` | RAM / memory model | Capacity model exists; individual RSS wins are not fully paired against OpenSearch | Future memory claims must include peak/final RSS from the same harness and same pod/host shape |
| `df3b0aa` plus supporting index/codec commits `6f56fd2`, `2da9249` | Search latency | Proven by `2026-05-20-A-lot3-paired-K8s` | This is the only current isolated before/after Track A perf proof with K8s run ids |
| Future Track A commits | Any perf axis | Must update this ledger in the same PR/commit sequence as the optimisation proof | Required fields: axis, workload, Surch numbers, OpenSearch/Elasticsearch reference where relevant, delta, run id/artifact path, and missing proof |

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
