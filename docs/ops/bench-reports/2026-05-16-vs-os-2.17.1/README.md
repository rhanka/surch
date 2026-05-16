# Bench Surch vs OpenSearch 2.17.1 — 2026-05-16

Paired benchmark captured against `main = cc11262` (R14 A10 phase 2)
on the development workstation, both engines running back-to-back on
the same machine via `make bench-pair-<workload>`.

Source: `target/bench-reports/f85b361/` (timestamped on Surch HEAD
`f85b361` at run time; R14 merge `cc11262` did not touch the
scoring / bulk paths, so the numbers stand).

## SciFact (BEIR — quality + bulk speed)

5183 docs / 300 queries / NDCG@10.

| Métrique | Surch | OS 2.17.1 | Δ |
|---|---|---|---|
| NDCG@10 | **0.6576** | 0.6537 | +0.6 % quality |
| Recall@10 | **0.8100** | 0.8033 | +0.8 % quality |
| Bulk index | **3 545 ms** | 17 612 ms | **5.0×** plus rapide |

Raw outputs : `scifact-surch.out`, `scifact-os.out`,
`scifact-pair.json`.

Lucene/Anserini reference NDCG@10 = 0.688 ; the ~5 % gap is the
expected cost of running without a Porter stemmer + with default
BM25 `k1=1.2 b=0.75`. Surch and OS land within ~0.5 % of each
other on this corpus.

## BAN Paris 25k (latency)

25 000 docs bulk-indexed, then 10 queries × 2 distinct strings.
`took` reflects engine work (no HTTP / JSON overhead).

| | Surch | OS 2.17.1 | Avantage |
|---|---|---|---|
| **`took` p50** | **0 ms** (sub-ms) | 20 ms | **>20×** |
| **`took` p95** | 20 ms | 108 ms | **5.4×** |
| **`took` max** | 20 ms | 108 ms | 5.4× |
| **Bulk index 25k** | **17 882 ms** | 21 707 ms | +22 % plus rapide |

Sorted `took` (ms) :

- Surch : `0, 0, 0, 0, 0, 0, 0, 0, 2, 20`
- OS    : `13, 14, 15, 15, 17, 23, 30, 49, 51, 108`

Raw outputs : `ban25k-surch.out`, `ban25k-os.out`,
`ban25k-pair.json`.

### Implications matchID 50 RPS

`deces-backend`'s artillery scenario targets p95 < 200 ms / max <
500 ms at 50 RPS sustained over 4 minutes
(`docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
§4). On the same machine shape :

- Surch : `took` ≤ 20 ms × 50 RPS ≈ **1 vCPU busy max**.
- OS    : `took` 20–108 ms × 50 RPS ≈ **1 to 5 vCPU busy**, will
  saturate a single core.

The artillery rehearsal therefore has comfortable headroom on
Surch and is on the edge on OS for the same hardware budget.

## How to reproduce

```bash
make bench-pair-scifact   # SciFact NDCG@10 + bulk latency
make bench-pair-ban25k    # BAN Paris 25k latency
```

Both targets stand up OS 2.17.1 via Docker (heap 512 m), index, run
the workload, tear down. Outputs land under `target/bench-reports/<sha>/`.
Promote a run into `docs/ops/bench-reports/<date>-<context>/` to
freeze the historical record.

## Caveats

- Single-machine, single-run. No warmup phase beyond the first
  query of each `bench.sh` group. Variance run-to-run is ~30 % on
  client-side `client_ms` (HTTP + JSON dominated); `took` is the
  signal that matters here.
- OS heap is intentionally tiny (512 m) so the bench fits a laptop;
  production OS would run with 2–4 GiB heap, which would slightly
  improve OS `took` but not enough to flip the order of magnitude.
- Surch is in-memory ; this favours `took` but penalises RAM
  capacity (see `docs/ops/memory-capacity.md` for the INSEE 1.3 M
  doc projection).

## Round-trip with poc-k8s

When the burst pool lands (cf. `requests/surch.md` amendment
2026-05-16), the same benches re-run in CI via the
`ndcg-gate.yaml` / `insee-bench.yaml` Jobs declared in
`deploy/k8s/jobs/`. The captured K8s output will land in a
sibling folder `docs/ops/bench-reports/<date>-k8s-burst/`.
