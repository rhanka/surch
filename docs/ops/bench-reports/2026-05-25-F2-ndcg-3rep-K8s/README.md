# Objective F — F2: 3-rep ndcg-gate (bulk + RSS + quality, median/IQR)

Multi-repetition K8s `ndcg-gate` group for Objective F F2 — turning
the single-run headline claims (TREC-COVID bulk, RSS, quality) into a
median + range verdict, per the Track A replay protocol (≥ 3
repetitions per ref).

- 3 successful repetitions on `main` (engine = Lot 1 → Lot 3 + A10).
- GHA runs: `26406537324`, `26407492650`, `26408421675` (all PASS).
- Surch image: `ghcr.io/rhanka/surch:sha-<main HEAD>` built for the
  group; OpenSearch 2.17.1, `-Xms1g -Xmx1g`; Scaleway burst pool,
  Surch 7 GiB cap.

## TREC-COVID bulk (full 171 k corpus) — the headline

| Engine | rep1 | rep2 | rep3 | median | min–max |
|--------|-----:|-----:|-----:|-------:|--------:|
| Surch | 78.18 s | 70.96 s | 69.70 s | **70.96 s** | 69.70–78.18 s |
| OpenSearch | 105.90 s | 111.60 s | 109.73 s | **109.73 s** | 105.90–111.60 s |

**Surch is `1.55x` faster than OpenSearch on TREC-COVID bulk at the
median, and faster in every single repetition** (Surch max 78.18 s <
OpenSearch min 105.90 s — the distributions do not overlap). This
confirms the Lot 1.6 bulk-parity-crossing result is robust, not a
single-run artefact.

## SciFact bulk (5 183 docs)

| Engine | rep1 | rep2 | rep3 | median | min–max |
|--------|-----:|-----:|-----:|-------:|--------:|
| Surch | 1493 ms | 2376 ms | 2087 ms | **2087 ms** | 1493–2376 ms |
| OpenSearch | 13 588 ms | 13 968 ms | 15 181 ms | **13 968 ms** | 13 588–15 181 ms |

Surch median `6.7x` faster on SciFact bulk; non-overlapping
distributions.

## Surch RSS peak (full TREC-COVID corpus)

| rep1 | rep2 | rep3 | median | min–max |
|-----:|-----:|-----:|-------:|--------:|
| 2159 MiB | 2180 MiB | 2168 MiB | **2168 MiB** | 2159–2180 MiB |

RSS peak is extremely stable (`±0.5 %` across reps) at `~2168 MiB`
(31 % of the 7 GiB cap), `~1.48x` the OpenSearch peak
(`~1467 MiB`, also stable). The Lot 1.6 + jemalloc memory profile is
reproducible.

## Quality (NDCG@10 / Recall@10) — bit-stable across 3 reps

| Workload | Surch | OpenSearch |
|----------|-------|------------|
| SciFact | 0.6576 / 0.8100 (all 3 reps) | 0.6537 / 0.8033 |
| TREC-COVID | 0.4750 / 0.0132 (all 3 reps) | 0.4902 / 0.0132 |

Zero quality variance across repetitions — the bulk/RAM/allocator
optimisations do not perturb retrieval results.

## Verdict (F2)

The headline claims now hold with multi-rep evidence:

- **Bulk**: Surch beats OpenSearch on TREC-COVID (`1.55x`, non-overlapping)
  and SciFact (`6.7x`); robust across 3 reps.
- **Memory**: Surch RSS peak `~2168 MiB ±0.5 %`, reproducible.
- **Quality**: bit-stable, no regression.

These three axes are paper-ready (median + range stated). Still
single-run / pending for the article: the **search-latency** axis
(insee-bench multi-rep) and the **historical optimisation isolation**
(F3 / Lot 4).

## Files
- `summary-rep-26406537324.md`, `summary-rep-26407492650.md`,
  `summary-rep-26408421675.md` — the 3 raw run summaries.
- `job.yaml` — Job manifest.
