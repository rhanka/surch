# Figure data (Objective F draft)

Plot-ready CSV series backing the article's headline curves. Each row
cites the SHA or promoted K8s report it comes from, so the figures are
reproducible from the CI artefacts. No plotting tool is bundled (the
repo is Python-free); render with whatever the author prefers
(gnuplot, a spreadsheet, etc.).

| File | Figure | x | y |
|------|--------|---|---|
| `bulk-trec-covid-by-lot.csv` | Bulk-indexing optimisation sequence | optimisation step | TREC-COVID 171k bulk seconds (Surch vs OpenSearch) |
| `rss-trec-covid-by-lot.csv` | Memory footprint per step | optimisation step | Surch RSS peak/final MiB (+ OpenSearch peak) |
| `latency-by-corpus.csv` | Search latency, Surch vs OpenSearch | corpus | p50/p95/p99/max ms per engine |

Sources: bulk/RSS series from the `ndcg-gate` lot runs +
`2026-05-25-F2-ndcg-3rep-K8s`; latency from
`2026-05-25-F2-insee-3rep-K8s` and
`2026-05-25-F4-trec-covid-latency-3rep-K8s`. See
`docs/ops/bench-reports/track-a-performance-ledger.md` for the full
provenance with GHA run ids.
