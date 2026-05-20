# Track A Lot 3 — Paired K8s perf-proof (before/after FoR-meta wiring)

Closes the Lot 3 "Perf proof" entry in `plan/wp-a-optim.md`. Pairs a
K8s `insee-bench` run against the **last `main` HEAD before the FoR
block-meta wiring** (`c01b0a2`) with the already-promoted post-wiring
baseline (`df3b0aa` family, captured under
`2026-05-19-insee-10k-k8s/`). Both runs use the same INSEE 10 000-row
slice, same Scaleway burst pod shape (DEV1-XL, 30 min cap), same
bootstrap + warmup pipeline.

## Method

The bench-driver image at `c01b0a2` predates `82320ca fix(k8s):
bootstrap deces index before insee-bench` and the follow-up
`75367c9` / `495403c` (warmup + pre-clean + wait shards). A first
naive `insee-bench` on `c01b0a2` therefore drove 13 170 / 13 170
errors (run `26141672559`).

To get a comparable "before" measurement, we created the throwaway
branch `perf-baseline/before-for-meta` (still on origin) pinned at
`c01b0a2`, cherry-picked the bootstrap/warmup/pre-clean trio
(`82320ca`, `75367c9`, `495403c`) — and **nothing else** — and re-ran
the K8s bench. The FoR-meta wiring (`df3b0aa`) is the only Surch
change that differs between the two runs.

Captured runs:

| Phase | SHA | Manifest | GHA run | Verdict |
|---|---|---|---|---|
| BEFORE | `f2b03f0…` (= `c01b0a2` + bootstrap cherry-picks) | bootstrap + warmup + pre-clean | [26151880297](https://github.com/rhanka/surch/actions/runs/26151880297) | success (16 min) |
| AFTER  | `495403c…` | bootstrap + warmup + pre-clean | [26101404966](https://github.com/rhanka/surch/actions/runs/26101404966) | success (15 min) |

## Paired latency (global, 50 RPS sustained 4 min, 0 errors / 13 170 issued)

| Engine | Build | p50 | p95 | p99 | max |
|---|---|---:|---:|---:|---:|
| Surch | BEFORE (`c01b0a2`) | 2.4 ms | 4.6 ms | 7.8 ms | 25.6 ms |
| Surch | AFTER  (`df3b0aa+`) | **1.9 ms** | **3.6 ms** | **6.9 ms** | **17.9 ms** |
| Δ |  | **-21 %** | **-22 %** | **-12 %** | **-30 %** |
| OS 2.17 | BEFORE | 4.2 ms | 11.1 ms | 23.2 ms | 429.7 ms |
| OS 2.17 | AFTER  | 3.8 ms | 9.9 ms | 20.8 ms | 135.3 ms |
| Δ |  | -10 % | -11 % | -10 % | -69 % |

## Verdict

- **Surch hot path improves by 12-30 % across every percentile after
  the FoR-meta wiring** on a reproducible burst-pod shape, with
  zero errors on either side.
- The OpenSearch shifts (Surch and OS both ran in the same pod, same
  bootstrap, same warmup) trace to between-run variance on a
  single-node OS cluster — the `max` envelope on the BEFORE OS run
  shows the usual cold-start outlier (~430 ms) that the AFTER OS run
  didn't see; the rest of the OS percentiles move by ~10 %, well
  inside the noise margin observed on
  `2026-05-19-insee-10k-k8s/`. **No regression** on the OS side.

## What this report does NOT settle

- This is a **single paired capture**, not a multi-run distribution.
  A regression that fits inside ±10 % of one run would be invisible
  here. The Lot 3 closure does not commit to a confidence interval
  beyond "directionally positive on K8s".
- Bulk-load timings are not compared — only the search hot path. The
  FoR-meta wiring should be neutral on bulk by construction
  (`PostingsBuilder` was not touched), but a paired bulk number is
  a possible follow-up.
- The `perf-baseline/before-for-meta` branch is left on origin so a
  re-bench is one `gh workflow run` away.

## Source data

- `before-artillery-runner.log` — the full driver log of run
  `26151880297`. Contains all 6 phases + the global per-engine
  summary that the table above references.
- After numbers are pulled verbatim from
  `docs/ops/bench-reports/2026-05-19-insee-10k-k8s/README.md`.
