# Bench Surch vs OpenSearch 2.17.1 — 2026-05-19 (K8s, INSEE 10k)

First fully-green run of the K8s `insee-bench` Job on the Scaleway
burst pool. Both engines indexed the real INSEE 10k slice
(`tests/matchid_compat/deces/slice-10000.ndjson.gz` — first 10 000 rows
of `Deces_2024.csv`, Open Licence 2.0), then served the matchID-style
artillery scenario (multi_match + bool.must alternating, 8 workers,
6 phases peaking at 50 RPS over 4 minutes).

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26101404966>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=insee-bench`)
- Job manifest: `deploy/k8s/jobs/insee-bench.yaml` (rendered + applied,
  archived as `job.yaml`)
- Surch image: `ghcr.io/rhanka/surch:sha-495403ca41784dbe1f7ab58c3967e7e8247596fa`
- Bench driver image: `ghcr.io/rhanka/surch:bench-sha-495403ca41784dbe1f7ab58c3967e7e8247596fa`
- Artefact: `k8s-bench-insee-bench-495403ca41784dbe1f7ab58c3967e7e8247596fa`
- Slice: INSEE Open Licence 2.0 `Deces_2024.csv` first 10 000 rows,
  sha256 `1f71d52c554900fbfb055be75ddff4fc04bb891cbce8725295f5fd7e68eace02`,
  357 kB gzipped / ~1.8 MB / 20 000 NDJSON lines.

## Pod shape (single Pod, two init engines + one driver)

| Container | Image | CPU req / limit | Mem req / limit |
|---|---|---|---|
| `surch` (init) | `ghcr.io/rhanka/surch:sha-495403c` | 150m / 800m | 128Mi / 512Mi |
| `opensearch` (init) | `opensearchproject/opensearch:2.17.1` | 250m / 1200m | 256Mi / 1536Mi |
| `artillery-runner` (driver) | `ghcr.io/rhanka/surch:bench-sha-495403c` | 100m / 500m | 128Mi / 512Mi |

Bootstrap pipeline before measurement:

1. `DELETE /deces` (best-effort, cleans previous run residue from the
   shared `surch-scratch` PVC).
2. Wait for cluster `YELLOW`.
3. `PUT /deces` with `tests/matchid_compat/deces/mapping.json` (text
   `index_prefixes` 2–5 on `NOM`/`PRENOMS`, keyword on date /
   commune / source, integer on `SOURCE_LINE`).
4. Wait for the new `deces` primary shard to be active.
5. `POST /_bulk` 10 000 docs from the gzipped slice.
6. `POST /deces/_refresh`.
7. Warmup: 50 `match_all` hits per engine to prime JIT + caches.

## Artillery results (`surch.bench.artillery.v1`)

| Engine | issued | errors | p50 ms | p95 ms | p99 ms | max ms |
|---|---:|---:|---:|---:|---:|---:|
| Surch | 13 170 | 0 | **1.9** | **3.6** | **6.9** | **17.9** |
| OS 2.17.1 | 13 170 | 0 | 3.8 | 9.9 | 20.8 | 135.3 |

Speedup Surch vs OS 2.17.1: **2.0× p50, 2.7× p95, 3.0× p99, 7.5× tail**.

Per-phase (Surch / OS), 30 s phases ramping 2→2→5→10→20 RPS, then
50 RPS for 4 min:

```
Surch phase 6: rps=50 dur=240 s issued=12000 errors=0 p50=1.9ms p95=3.6ms p99=6.9ms max=17.9ms
OS    phase 6: rps=50 dur=240 s issued=12000 errors=0 p50=3.7ms p95=8.9ms p99=19.8ms max=135.3ms
```

Raw run: `artillery-runner.log`.

## SLO checks (from `bench_report`)

All checks PASS on both engines:

- `artillery p95 ≤ 200 ms` — OS observed 9.9, Surch observed 3.6.
- `artillery max ≤ 500 ms` — OS observed 135.3, Surch observed 17.9.
- `artillery error rate ≤ 1 %` — both 0.000 % (0 / 13 170).

## Implications matchID 50 RPS

`deces-backend`'s artillery scenario targets `p95 < 200 ms` / `max <
500 ms` sustained at 50 RPS for 4 minutes. On the K8s burst-pool
shape used here (1 vCPU effective per engine):

- Surch: **p95 3.6 ms** at 50 RPS → ~55× headroom under the SLO.
- OS:    **p95 9.9 ms** at 50 RPS → ~20× headroom under the SLO.

Both engines fit, but Surch keeps an order of magnitude more spare
budget for richer query mixes (multi_match + boost, function_score,
bool with filter clauses).

## What is intentionally NOT in this report

- TREC-COVID NDCG@10 — still pending K8s promotion (gap B follow-up).
- RSS Surch/OS paired sampling — not run inside `insee-bench` (RSS
  harness is local; K8s pod limits act as the proxy).
- BEIR retrieval rows — `insee-bench` does not run BEIR; that is
  `ndcg-gate`'s territory (see `2026-05-16-vs-os-2.17.1/` for the
  promoted SciFact + Anserini-reference numbers).
