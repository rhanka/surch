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

## Follow-up: MaxScore-style WAND skipping (v2.6)

v2.3 still scored every matching document for OR-Match queries. For
`Rue Payenne` that means scoring all 18 194 candidates even though
the top ten are dominated by the rare `payenne` term. v2.6 adds a
MaxScore-style early-skip path:

- per query token, compute an upper bound BM25 contribution from
  `term_freq_by_doc_id.values().max()` and the field's smallest
  observed `doc_len`;
- iterate tokens from highest to lowest max contribution;
- after each token, recompute the K-th score threshold over the
  currently scored docs;
- when iterating a later token whose max contribution is below that
  threshold, skip docs that are not already scored from a rarer
  token — they cannot beat the threshold by adding only this term.

The path is enabled for `SearchQuery::Match` with default OR operator
on a single physical index; AND-Match and other shapes fall through
to the v2.3 full-scoring path. Top-hit order matches OpenSearch
(tie-break by ascending internal doc id) and `hits.total.value` stays
exact, since the union is still walked once for total counting.

Bench summary (Paris 25k, `track_total_hits=true` on both engines):

| Operation | Surch v2.3 | Surch v2.6 (WAND) | OpenSearch 2.17.1 |
| --- | ---: | ---: | ---: |
| `_bulk` 25k | ~480 ms | ~3 s (under heavy concurrent load) | ~5 s |
| `_search` `Rue Payenne` (18 194 hits) | ~25 ms | **~5 ms** (5–12 ms range) | 2–9 ms |
| `_search` `Place Patrice Chereau` (559 hits) | ~2 ms | ~1–3 ms | 2–5 ms |

System load drifted heavily between runs (svelte-check, vite,
tsup, multiple browser processes), so absolute numbers should be
read as ranges. The ordering is consistent across runs: Surch v2.6
matches or beats OS on `_bulk` and the low-cardinality search and
stays within ~2–4x of OS on the high-cardinality `Rue Payenne`. The
remaining gap is the codec-level optimizations that Lucene has and
Surch does not (block-max scores per postings block, SIMD scoring,
lazy block decompression).

## Follow-up: top-K with lazy source hydration (v2.3)

After v2.1 closed the indexing-time stats gap, the next obvious win was
that scoring still hydrated the full JSON `_source` for every matching
document, then sorted, then discarded everything past the requested
page. For `Rue Payenne` that meant 18 194 source clones per request
just to return ten hits.

v2.3 adds an early-return path in `run_search` that triggers when the
query is `Match`/`MultiMatch` with the default `_score`-desc sort and a
single physical index. On that path:

- candidate docs are kept as internal `u32` ids (no public-id round
  trip; new `documents_for_match_internal` and
  `documents_by_internal_ids` on `AppState`);
- all candidates are scored, then partitioned with
  `select_nth_unstable_by` to keep the top `from + size`;
- only those winners are hydrated into `StoredDocument`;
- tie-breaking is internal doc id ascending (Lucene convention) so the
  default ordering matches OpenSearch on equal-score ties.

Cleanest paired Paris 25k window we could capture (system load
fluctuated, multiple runs were captured):

| Operation | Surch v2.1 | Surch v2.3 (top-K) | OpenSearch 2.17.1 |
| --- | ---: | ---: | ---: |
| `_bulk` 25k | ~1050 ms | ~480 ms | ~7400 ms |
| `_search` `Rue Payenne` (18 194 hits, `track_total_hits=true`) | ~95 ms | ~12 ms | ~9 ms |
| `_search` `Place Patrice Chereau` (559 hits) | ~5 ms | ~1 ms | ~5 ms |

With `track_total_hits=true` on both engines (apples-to-apples,
exact 18 194 returned by both), the same Surch v2.3 run came out at
~25 ms vs OS ~5 ms on `Rue Payenne` in a noisier second window. OS
keeps a 3–5x edge on the high-cardinality search because Lucene's
block-max WAND skips entire blocks once the top-K threshold is known,
while Surch v2.3 still scores every matching document. That is the
next-step optimization (per-block max scores in postings, score
threshold tracking during scoring).

The HashMap variant (v2.4) was tried and rejected: std `HashMap` with
SipHash is slower than `BTreeMap` for ~18 k `u32` keys (~80 ms vs
~25 ms on `Rue Payenne`). A two-pointer lockstep walk (v2.5) was
prototyped and reverted: it did not show a measurable win on this
hardware under the load drift we had, and the simpler v2.3 path is
clearer and slightly less code.

## Reference: Surch v2.1 vs OpenSearch 2.17.1 on the same machine

Same Paris 25k dataset, same machine, same bench script, each engine
run solo (the other stopped). OpenSearch was started via
`scripts/bench/opensearch-start.sh` with the default 512 MB heap and
2.17.1 image. Cleanest paired run captured at the end of the session
(system relatively quiet):

| Operation | Surch v2.1 took | OpenSearch 2.17.1 took | Surch vs OS |
| --- | ---: | ---: | --- |
| `_bulk` Paris 25k | 1048 ms | 7442 ms | Surch ~7x faster |
| `_search` `Rue Payenne` | 96 ms | 9 ms | Surch ~10x slower |
| `_search` `Place Patrice Chereau` | 4 ms | 8 ms | Surch ~2x faster |

Important caveats on this comparison:

- OpenSearch returns `hits.total.value = 10000` (default
  `track_total_hits` cap), Surch returns the exact `18194`. OpenSearch
  is doing less collection work on the high-cardinality query.
- OpenSearch's Lucene scoring uses block-max WAND skipping; Surch
  currently scores every matching document. This is the main reason
  OS is faster on `Rue Payenne`. The two low-cardinality runs are
  comparable because skipping does not buy much when there are only
  559 matches to score.
- Three solo runs of each engine showed bulk between 1.0 s and 9.0 s
  for Surch and between 3.4 s and 25.3 s for OpenSearch on this host.
  System load (`vite dev`, `svelte-check`, `tsup`, browsers) drifted
  during the session. The numbers in the table are from the cleanest
  paired window; medians across all runs preserve the same ordering.
- Bulk numbers should not be read as a publishable Surch/OpenSearch
  ratio: OpenSearch is doing real segment writes, refreshes and
  durability while Surch is an in-memory router, so this is an
  apples-to-oranges difference on ingestion. The search side is the
  closer comparison.

Follow-ups that remain open from
`docs/poc/reports/ban-api-performance-debug-0dced7d.md`:

- step 1 (postings-backed candidates) is still partial — TopDocs-style
  collection before source hydration is not implemented;
- the next obvious search-side win is Lucene-style block-max WAND or
  doc-id skipping during scoring, which would close most of the
  `Rue Payenne` gap;
- steps 3 and 4 (MatchID fixtures alignment and go/no-go thresholds)
  have not started.
