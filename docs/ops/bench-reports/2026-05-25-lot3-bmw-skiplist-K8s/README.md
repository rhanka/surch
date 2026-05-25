# Lot 3 — Block-Max WAND skip-list leapfrog (search latency, K8s)

Isolated search-latency measurement for Track A **Lot 3** — the next
Block-Max WAND step that lets the OR-match / `multi_match` MaxScore
loop leapfrog whole 128-blocks via the Lot 2 skip-list cursors
(`crates/surch-search/src/maxscore.rs`).

**Honest verdict: no measurable latency gain on the available
workload (INSEE 10k).** Lot 3 lands correctly (ranking unchanged,
`ci` green, NDCG bit-stable) but its benefit regime — long posting
lists where whole blocks can be skipped — is not exercised by the
INSEE 10k matchID workload, where posting lists are short
(≤ ~78 blocks/term). The p50/p95/p99 deltas are within single-run
noise.

## Isolation (control vs treatment, same jemalloc stack)

| SHA | Stack | Lot 3 |
|-----|-------|:-----:|
| `3625fef` (control) | jemalloc + Lot 1.6 + Lot 2 | no |
| `e293cfc` (treatment, main) | + Lot 3 + A10 (search-neutral) | **yes** |

Control insee-bench: run `26404638599` (benchmark complete, valid;
the run pre-dates the `97e81f3` teardown fix so its workflow
status is a false-fail, but the artillery data is clean —
`Complete=True`, 0 errors). Treatment insee-bench: run
`26405557238`, GREEN (first insee-bench validated by the `97e81f3`
wait-loop fix).

### Surch search latency (artillery deces, 13 170 queries, 0 errors)

| Metric | Control `3625fef` (no Lot 3) | Treatment `e293cfc` (+ Lot 3) | Delta |
|--------|---:|---:|---:|
| p50 | 1.4 ms | 1.6 ms | +0.2 ms (noise) |
| p95 | 3.7 ms | 4.0 ms | +0.3 ms (noise) |
| p99 | 7.2 ms | 8.1 ms | +0.9 ms (noise) |
| max | 47.9 ms | 38.2 ms | -9.7 ms |

The sub-millisecond p50/p95 differences are within the run-to-run
variance already observed on this workload (e.g. Lot 2 max 21.6 vs
64.1 ms across two runs). No directional latency improvement is
claimable from Lot 3 on INSEE 10k.

## Why no gain here

Lot 3's MaxScore block-leapfrog pays off when a token's posting
list spans many 128-blocks and the block-max upper bound lets the
loop skip whole blocks below the top-K threshold. On INSEE 10k:

- 10 332 docs → at most ~81 blocks per token, and matchID name/date
  terms are selective (short lists), so there are few blocks to
  skip.
- The queries are multi-field AND (already cheap), not high-cardinality
  OR top-K.

The regime where Lot 3 helps — large corpora / high-frequency terms
(TREC-COVID-scale OR-match top-K) — has no latency-percentile harness
today (`ndcg-gate` issues only 50 BEIR queries without percentiles).

## Correctness (ndcg-gate run 26397689211, main)

Ranking is unchanged by Lot 3 (the skip decision is byte-for-byte
identical to the linear Lot 1 path; the agent's tests assert
parity against a brute-force OR-match oracle):

| Workload | Surch NDCG@10 | Surch Recall@10 | vs prior lots |
|----------|--------------:|----------------:|---------------|
| SciFact | 0.6576 | 0.8100 | identical |
| TREC-COVID | 0.4750 | 0.0132 | identical |

Bulk + RSS also unchanged (Lot 3 is search-path only).

## Verdict

| Axis | Verdict |
|------|---------|
| Correctness | PASS — ranking bit-stable, `ci` green (compile + clippy + workspace tests), brute-force oracle parity tests in `crates/surch-search/tests/maxscore.rs`. |
| Search latency (INSEE 10k) | Neutral — no measurable gain, within single-run noise. |
| Benefit regime | Unproven — needs a latency harness on a large corpus (TREC-COVID-scale) with high-frequency OR-match top-K. |

## Recommendation

Lot 3 is kept (it is correctness-neutral, adds the skip-list-aware
MaxScore executor, and is a prerequisite for further WAND work), but
**it is not claimed as a latency win**. To prove its value, Objective
F should add a large-corpus search-latency benchmark (F-gap-4: a
TREC-COVID artillery latency workload), where the block-leapfrog
regime is actually exercised.

## Files
- `summary-treatment-main-lot3.md` — main (+ Lot 3) insee-bench.
- `summary-control-3625fef.md` — control (no Lot 3) insee-bench.
- `job.yaml` — Job manifest.
