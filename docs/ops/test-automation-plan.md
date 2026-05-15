# Surch test automation plan

Date: 2026-05-15

## Decision

- **Makefile-driven**, not `cargo bench` — our benchmarks run against an
  HTTP engine (Surch + OpenSearch in parallel), and `cargo bench`
  (Criterion) does not orchestrate external processes, healthchecks,
  Docker containers. Make handles target dependencies, parallelism,
  teardown traps natively. Pattern aligned with matchID (`tools/Makefile`,
  `deces-backend/Makefile`).
- **Rust binaries inside `surch-demo`** as the measurement layer:
  `ban-http-bench` (sequential, already shipped), upcoming
  `artillery-bench` (concurrent keep-alive), `beir-bench` (NDCG@10
  pipeline). Bash stays only as the orchestrator glue.

## Target hierarchy

```
make test            # cargo test --workspace (~30 s)
make bench-smoke     # BAN tiny 3 docs + 1 query, sanity (~10 s)
make bench-local     # BAN 25k + INSEE 25k local (~5 min)
make bench-recall    # SciFact + TREC-COVID NDCG@10 (~10 min)
make bench-stress    # artillery-replay vs Surch and OS (~10 min)
make bench-perf      # bench-local + bench-stress + RSS sampling
make bench-remote-scw  # provision Scaleway, run bench-perf + 1M scale, teardown
make bench-all       # full local suite (~30 min)
make report          # aggregate target/bench-reports/<sha>/*.json -> summary.md
```

Each target: `setup → wait healthy → run → collect → teardown → emit JSON + MD`.
Exit code reflects SLO pass/fail.

## SLO targets (v1)

Calibration rule: "no worse than OS in latency at equal load" + "≥ Lucene −5 points absolute on NDCG@10". Tighten after the first reproduced baselines land.

TREC-COVID is now wired in as the second BEIR correctness gate (after SciFact). It runs as part of `make bench-recall` and on its own via `make bench-trec-covid`. Unlike SciFact's binary qrels (~1 judged doc per query), TREC-COVID ships graded judgments 0/1/2 averaging ~500 per query, so the NDCG@10 implementation uses `gain = 2^rel - 1` and the IDCG sorts all judged docs by grade desc before taking the top 10. This denser, graded signal makes any BM25 regression jump out of the noise floor.

| Workload | Metric | Target Surch | Observed OS baseline | Bench file |
| --- | --- | ---: | ---: | --- |
| BAN Paris 25k | bulk ingestion | ≥ 10 000 docs/s | ~10 000 (TBD) | `scripts/bench/bench.sh` |
| BAN Paris 25k | search `Place Patrice Chereau` p50 | ≤ 10 ms | ~5 ms Surch today | idem |
| BAN Paris 25k | search `Rue Payenne` p95 | ≤ 200 ms | ~150 ms OS (TBD) | idem |
| INSEE 25k matchID | bulk ingestion | ≥ 8 000 docs/s | TBD | `scripts/bench/insee-bench.sh` |
| INSEE 25k matchID | match_NOM p95 | ≤ 50 ms | TBD | idem |
| INSEE 25k artillery 50 RPS | p95 phase 50 RPS | ≤ 200 ms | TBD | `scripts/bench/artillery-replay.sh` |
| INSEE 25k artillery 50 RPS | p99 phase 50 RPS | ≤ 500 ms | TBD | idem |
| INSEE 25k artillery 50 RPS | RSS peak | ≤ 1024 MB | OS ~512 MB heap | idem + `pidstat` |
| SciFact 5k BEIR | NDCG@10 | ≥ 0.65 (Lucene 0.688) | 0.688 Anserini | `scripts/bench/scifact-ndcg.sh` |
| SciFact 5k BEIR | Recall@10 | ≥ 0.90 | TBD | idem |
| TREC-COVID 171k | NDCG@10 | ≥ 0.55 (Anserini 0.595) | 0.595 | `scripts/bench/trec-covid-ndcg.sh` |
| TREC-COVID 171k | Recall@10 | ≥ 0.05 (Anserini ~0.057) | TBD | idem |
| TREC-COVID 171k | RSS peak | ≤ 4 GB | TBD | idem |
| mMARCO-fr 8.8M | indexation | ≥ 5 000 docs/s sustained | TBD | upcoming `scripts/bench/mmarco-fr-ndcg.sh` (remote only) |
| mMARCO-fr 8.8M | NDCG@10 | ≥ 0.30 (BM25 baseline) | 0.30 | idem |
| mMARCO-fr 8.8M | RSS peak | ≤ 12 GB | TBD | idem |

## Report schema

```json
{
  "schema": "surch.bench.v1",
  "commit_sha": "0dc30ad", "branch": "main", "date": "2026-05-15T10:00:00Z",
  "host": "scw-dev1-m-par1", "engine": "surch", "engine_version": "0.1.0",
  "workload": "insee-25k-artillery",
  "metrics": { "p50_ms": 8.4, "p95_ms": 22.1, "p99_ms": 41.7,
               "rss_peak_mb": 412, "docs_per_sec": 12345,
               "ndcg_10": null, "recall_10": null },
  "slo_passed": true,
  "regression_vs_baseline": { "p95_delta_pct": -3.1, "rss_delta_pct": 1.2 }
}
```

Persisted under `target/bench-reports/<sha>/<workload>-<engine>.json`. A `surch-demo bench-compare --baseline main --head HEAD` downloads the latest `main` JSON from the Scaleway Object Storage bucket and exits non-zero if p95 regresses > 15 %, NDCG@10 drops > 2 absolute points, or RSS grows > 25 %.

## Remote runs on Scaleway

**Instance choice**:
- `DEV1-M` (4 vCPU / 4 GB / 0.012 €/h) for BAN, INSEE, SciFact
- `GP1-S` (4 vCPU / 16 GB / 0.062 €/h) for TREC-COVID, mMARCO-fr
- Region `fr-par-1` (aligned with matchID)

**Cost guardrails (strict, hard caps)**:
- `SCW_MAX_COST_EUR=2` (≥ 30 min on `GP1-S`, > 100 h on `DEV1-M` — safe ceiling)
- `SCW_MAX_DURATION_MIN=30`; remote run wrapped in `timeout 30m`
- `trap 'make scw-down' EXIT INT TERM` in every remote script; auto-teardown even on crash
- Pre-check: `scw instance server list state=running tags=surch-bench` must be empty before launch; refuse if a previous instance leaked
- Post-check: same query must be empty again at the end; otherwise emit a loud warning and force-delete

**Provisioning**: `scw` CLI direct, no Terraform. Terraform requires a remote state and adds surface area for ephemeral 30-minute instances. Same trade-off matchID made (`tools/Makefile:400-405`).

**Volume**: always pass `--with-volumes=all --with-ip=true` to `scw instance server delete` so the volume and the public IP get released. matchID convention: `SCW_VOLUME_TYPE=l_ssd`, 20 GB for BAN/INSEE/SciFact, 50 GB for mMARCO-fr.

**Hostname/tag convention**: `surch-bench-<sha>-<unix_ts>` (mirror of matchID `${APP_GROUP}-${APP}-${GIT_BRANCH}`).

## Implementation files

Implemented:
- `/Makefile` root with the target hierarchy above (also exposes
  `bench-pair-<workload>` as a pattern target dispatching to `run-pair.sh`
  and `bench-artillery-rs` for the Rust keep-alive harness)
- `/scripts/bench/run-pair.sh` — wraps Surch + OS bench in a single workload
  run, emits one `.out` per engine plus a `pair.json` envelope
  (schema `surch.bench.pair.v1`); trap-cleans both engines on EXIT/INT/TERM
- `/scripts/bench/rss-sample.sh` — 1 Hz sampling of `/proc/<pid>/status`
  `VmRSS` (with `ps` fallback) → JSON (schema `surch.bench.rss.v1`)
- `/scripts/bench/trec-covid-ndcg.sh` — clone of `scifact-ndcg.sh`, larger corpus
- `/crates/surch-demo/src/bin/artillery_bench.rs` — **B-RUST-HARNESS**.
  Rust binary that replaces the bash + curl `artillery-replay.sh` for the
  matchID SLO measurement story. hyper-util keep-alive connection pool,
  configurable workers, phased rate-limiting via `tokio::time::sleep_until`,
  percentiles per phase + global. JSON report schema
  `surch.bench.artillery.v1`. CLI: `artillery_bench --url --index --names
  --workers --phases --report`. Wired into the Makefile as
  `make bench-artillery-rs`. The bash `artillery-replay.sh` is kept as a
  no-build fallback.

To create:
1. `/scripts/bench/mmarco-fr-ndcg.sh` — remote-only, French baseline
2. `/scripts/scw/{wait-ssh,rsync-repo,remote-build,cost-guard}.sh` — Scaleway orchestration
3. `/crates/surch-demo/src/bin/bench_report.rs` — JSON → Markdown aggregation + regression comparison
4. `/crates/surch-demo/src/bin/bench_aggregate.rs` — pidstat `.log` → JSON

## References used

- matchID `tools/Makefile` cloud-instance up/down: https://github.com/matchid-project/tools/blob/master/Makefile#L400-L405
- matchID `Makefile` `SCW_IMAGE_ID`, `SCW_VOLUME_TYPE`: https://github.com/matchid-project/matchID/blob/master/Makefile#L121-L123
- matchID `deces-backend` `Makefile` test-perf-v1: https://github.com/matchid-project/deces-backend/blob/master/Makefile#L337
- BEIR datasets (SciFact, TREC-COVID): https://github.com/beir-cellar/beir
- BM25 baselines: https://github.com/xhluca/bm25-benchmarks
