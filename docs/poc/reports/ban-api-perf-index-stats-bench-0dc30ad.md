# BAN Paris 25k bench: index-time BM25 stats

Date: 2026-05-14
Baseline: HEAD `0dc30ad` (perf(api): use postings for match candidates).
Change under test: move BM25 field/term statistics from request-time
recomputation to indexing-time accumulation. Adds
`DocumentIndex::field_stats(field) -> FieldLengthStats`, exposes the
`mappings.properties.<f>.norms` boolean, and rewrites
`SearchScoringContext` to look up scoring data by internal `u32` doc id
instead of re-tokenizing source per scored document.

Both binaries built with `cargo build --release -p surch-api`. Surch
started on `127.0.0.1:7700` (`RUST_LOG=warn`). Dataset
`target/ban-bench/ban-paris-25000.ndjson`
(SHA-256 `0b5283ba27c8a293e67b7f46dd37da6b34e9efaa2bf3fde9f10f78bf589d94dc`,
same dataset pinned in `ban-paris-http-019d91e.md`). Index reset before
each run, 1 warmup query per shape, 5 measured iterations per query.
OpenSearch was not started for this run; this benchmark only measures
the Surch delta. Raw samples are kept locally under `target/bench-mon/`.

## Methodology caveat

The bench was run on a desktop with concurrent Svelte/Vite workloads
(`svelte-check` and `vite dev`), and load varied during the session.
Each pair (HEAD vs working tree) was therefore run as close in time as
possible. Multiple HEAD and working-tree runs were captured to make
the median informative. The bench is a regression signal, not a
publishable Surch/OpenSearch ratio.

## Iterations of the working tree

1. **v1** — original diff. `SearchScoringContext` built
   `BTreeMap<String, _>` per (field, token) by walking postings and
   cloning each document id String. On Paris 25k this was a clear
   regression: bulk +140%, `Rue Payenne` (18 k hits) +58%,
   `Place Patrice Chereau` (559 hits) -53%.
2. **v2** — same shape, but all scoring maps keyed by internal `u32`
   doc id with a one-shot public→internal translation per query.
   Net regression remained on `Rue Payenne` because
   `term_stats(field, token)` still allocated a `(String, String)`
   tuple key per scored document (~72 k allocations per query).
3. **v2.1** — flattened the per-token lookup to a nested
   `BTreeMap<String, BTreeMap<String, TermScoringStats>>`, so the hot
   path uses `BTreeMap::get(&str)` and does not allocate during
   scoring. This is the committed version.

## Numbers (server `took`, ms, median of 5 iterations)

Aggregate medians across 2–3 paired runs (HEAD vs v2.1) in similar
load conditions:

| Operation | HEAD `0dc30ad` | Working tree v2.1 | Delta |
| --- | ---: | ---: | --- |
| `_bulk` Paris 25k | ~1500 | ~2400 | +60% (slower) |
| `_search` `Rue Payenne` (18 194 hits) | ~190 | ~120 | -37% (faster) |
| `_search` `Place Patrice Chereau` (559 hits) | ~95 | ~5 | -95% (faster) |

Sample raw client medians for one of the cleanest paired sequences
(13:57 v2.1 run1 vs 14:01 HEAD run2, system relatively quiet):

| Operation | HEAD took | v2.1 took |
| --- | ---: | ---: |
| `_bulk` | 917 ms | 1614 ms |
| `_search` `Rue Payenne` | 144 ms | 94 ms |
| `_search` `Place Patrice Chereau` | 90 ms | 3 ms |

Both runs returned identical `count=25000`, identical total hits,
identical top-hit IDs.

## Interpretation

- Bulk ingestion pays a real cost: the analyzer now records per-field
  doc lengths and a doc count in `FieldLengthStats` at indexing time.
  This is exactly the work that previously happened lazily on every
  scored document via `compute_avg_doc_lens` and source tokenization.
- Low-cardinality queries (`Place Patrice Chereau`, 559 hits) collapse
  from ~95 ms to ~5 ms server time. The dominant savings come from
  no longer walking 25 k JSON sources to tokenize the `label` field
  while scoring.
- High-cardinality queries (`Rue Payenne`, 18 194 hits) drop from
  ~190 ms to ~120 ms. Less spectacular because the per-doc work was
  already amortized across many matches.
- Earlier iterations (v1, v2) were regressions on these same
  queries. They are useful only as a record of the implementation
  path; only v2.1 should be considered the actual change.

## Acceptance

This is a net win for the BAN search workload. The +60% bulk cost is
the indexing-time cost we deliberately accepted to remove per-request
source tokenization. The change is safe to merge.

Follow-ups that remain open from
`docs/poc/reports/ban-api-performance-debug-0dced7d.md`:

- step 1 (postings-backed candidates) is still partial — TopDocs-style
  collection before source hydration is not implemented;
- steps 3 and 4 (MatchID fixtures alignment and go/no-go thresholds)
  have not started.
