# Objective F — F2: 3-rep insee-bench (search latency, median/range)

Multi-repetition K8s `insee-bench` group for Objective F F2 — the
search-latency axis for the final-state engine (main: Lot 1 → Lot 3
+ A10), with median + range across 3 reps. Completes F2: bulk, RSS,
quality (`2026-05-25-F2-ndcg-3rep-K8s/`) and now latency all have a
multi-rep verdict.

- 3 successful repetitions on `main`. GHA runs `26409559587`,
  `26410171355`, `26410737049` (all PASS).
- Workload: INSEE 10k `deces` artillery, 13 170 queries, 6 RPS
  phases, 8 workers. OpenSearch 2.17.1 reference.

## Surch search latency (artillery deces, 0 errors all reps)

| Metric | rep1 | rep2 | rep3 | median | min–max |
|--------|-----:|-----:|-----:|-------:|--------:|
| p50 | 1.5 ms | 1.5 ms | 1.5 ms | **1.5 ms** | 1.5–1.5 ms |
| p95 | 4.1 ms | 3.8 ms | 4.3 ms | **4.1 ms** | 3.8–4.3 ms |
| p99 | 9.0 ms | 7.7 ms | 8.4 ms | **8.4 ms** | 7.7–9.0 ms |
| max | 40.6 ms | 34.7 ms | 55.0 ms | **40.6 ms** | 34.7–55.0 ms |

## OpenSearch 2.17.1 (same runs)

| Metric | rep1 | rep2 | rep3 | median |
|--------|-----:|-----:|-----:|-------:|
| p50 | 4.0 ms | 3.9 ms | 4.0 ms | **4.0 ms** |
| p95 | 12.2 ms | 11.5 ms | 12.5 ms | **12.2 ms** |
| p99 | 26.3 ms | 23.1 ms | 31.6 ms | **26.3 ms** |
| max | 231.2 ms | 195.8 ms | 223.1 ms | **223.1 ms** |

## Verdict

| Metric | Surch median | OpenSearch median | Surch advantage |
|--------|---:|---:|---:|
| p50 | 1.5 ms | 4.0 ms | **2.7x faster** |
| p95 | 4.1 ms | 12.2 ms | **3.0x faster** |
| p99 | 8.4 ms | 26.3 ms | **3.1x faster** |
| max | 40.6 ms | 223.1 ms | **5.5x faster** |

Surch search latency on the matchID INSEE workload is consistently
`2.7–3.1x` faster than OpenSearch at p50/p95/p99 across 3 reps, with
a rock-stable p50 (`1.5 ms`, zero variance) and low p95/p99
variance. The max is noisy (`34.7–55.0 ms`) as expected for a tail
extreme, but still `5.5x` better than OpenSearch. Both engines: 0
errors / 13 170 queries every rep; all artillery SLOs PASS.

## Scope note

This is the **final-state** latency (all of Lot 1 → Lot 3 + A10),
not an isolation of a single optimisation. The per-lot search
isolations are separate:
- Lot 2 skip lists: `2026-05-25-insee-lot2-skiplists-K8s/`
  (`p95 -13% / p99 -18%` vs the jemalloc control).
- Lot 3 MaxScore leapfrog: `2026-05-25-lot3-bmw-skiplist-K8s/`
  (latency-neutral on INSEE 10k; benefit regime needs a large-corpus
  latency harness — F-gap-4).

## F2 status

With this report F2 is **complete for the available workloads**:
bulk, RSS, quality (3-rep) and search latency (3-rep) all carry a
median + range verdict. Remaining for the article: the historical
optimisation isolation (F3 / Lot 4) and additional workloads
(F4: BEIR multi-dataset, corpus-size scaling, large-corpus latency
harness).

## Files
- `summary-rep-26409559587.md`, `summary-rep-26410171355.md`,
  `summary-rep-26410737049.md` — raw run summaries.
- `job.yaml` — Job manifest.
