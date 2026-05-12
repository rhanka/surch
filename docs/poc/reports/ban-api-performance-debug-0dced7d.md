# BAN API performance debugging report

Date: 2026-05-12
Baseline branch: `main`
Relevant commits:

- `8929921` batches `_bulk` mutations and index rebuilds.
- `0dced7d` caches BM25 request statistics during `_search`.

## Problem statement

The blocking issue was not the HTTP stack itself. The local API behaved slowly because
the OpenSearch-compatible API path still used bootstrap in-memory execution patterns
that did not match the Lucene/OpenSearch execution model:

- ingestion rebuilt the full in-memory inverted index after each bulk item;
- BM25 scoring recomputed corpus-wide field lengths and document frequencies for
  every scored document;
- search still scans JSON sources for query matching instead of executing from
  postings and collecting top documents first.

That explains how Surch could be structurally inspired by Lucene/OpenSearch while
still being slower: the low-level crates already contain postings, scoring, and
collector primitives, but the HTTP API had not yet been fully wired to that engine
path.

## Root causes found

### `_bulk`

Before `8929921`, each write operation called `InMemoryIndex::rebuild_index()`.
A bulk load of N documents therefore performed roughly N full index rebuilds.
`DocumentIndex::add_document_with_mapping` also rebuilt the term dictionary on
each document addition. Together this produced quadratic behavior.

Fix:

- `_bulk` now resolves and applies document write operations in one state mutation pass;
- each touched index is rebuilt once after the batch;
- `DocumentIndex::add_documents_with_mapping` builds the term dictionary once for a
  batch.

### `_search`

Before `0dced7d`, `bm25_field_score` called `compute_avg_doc_len` for every scored
document and recomputed `doc_freq` for every document/token pair.

Fix:

- each search builds a `SearchScoringContext` per index;
- average field lengths are computed once per field;
- document frequencies are computed once per field/token pair;
- existing response shape and query behavior are preserved.

## Local measurements

Environment: local release binary, Surch HTTP on `127.0.0.1:7700`, OpenSearch HTTP
on `127.0.0.1:9200`, BAN Paris generated NDJSON under `target/ban-bench/`.

### Surch before/after

| Operation | Dataset | Before | After |
| --- | ---: | ---: | ---: |
| `_bulk` | Paris 500 | 140.03s client | 0.331s client |
| `_bulk` | Paris 25k | timed out in prior run | 1.573s client / `took=1465ms` |
| `_search` `Rue Payenne` | Paris 500 | 5.887s client | 0.155s client / `took=116ms` |
| `_search` `Place Patrice Chereau` | Paris 500 | 0.127s client | 0.065s client / `took=55ms` |
| `_search` `Rue Payenne` | Paris 25k | not publishable before fix | 3.052s client / `took=2929ms` |
| `_search` `Place Patrice Chereau` | Paris 25k | not publishable before fix | 2.698s client / `took=2690ms` |

### OpenSearch comparison on Paris 25k

| Operation | Surch | OpenSearch |
| --- | ---: | ---: |
| `_bulk` | 1.573s client / `took=1465ms` | 8.433s client / `took=7112ms` |
| `_search` `Rue Payenne` | 3.052s client / `took=2929ms` | 0.458s client / `took=396ms` |
| `_search` `Place Patrice Chereau` | 2.698s client / `took=2690ms` | 0.371s client / `took=305ms` |

Notes:

- The `Rue Payenne` OpenSearch total uses the default `track_total_hits` cap and
  reports `10000`; Surch currently returns the exact total.
- Top hits matched for the measured queries:
  - `Rue Payenne`: `75103_7205_00001`;
  - `Place Patrice Chereau`: `75103_vaip3v_00001`.

## Current readiness assessment for MatchID

Surch is not ready to replace Elasticsearch in MatchID yet. It is closer after
these fixes because ingestion is no longer the immediate blocker, but search on
25k BAN data is still around 6x to 8x slower than local OpenSearch for the measured
queries.

The earliest credible path is now:

1. Shadow-read pilot only, after real MatchID fixtures and query replays exist.
2. Postings-backed search execution for `match`, `multi_match`, `bool/must`, and
   fuzzy rewrites.
3. Durable API state with restart/recovery checks.
4. Repeated symmetric HTTP benchmark runs with no compatibility mismatches.

## Next engineering steps

1. Wire API `_search` to postings-backed execution:
   - map `SearchQuery` to the `surch-search` query/executor path;
   - use postings to produce candidate doc IDs;
   - use TopDocs-style collection before source hydration;
   - keep JSON scan fallback only for query types not yet planned.

2. Add per-index search statistics maintained at indexing time:
   - `doc_count`;
   - `avg_doc_len(field)`;
   - `doc_len(doc, field)`;
   - `doc_freq(field, term)`.

3. Align benchmark fixtures with MatchID:
   - commit redacted mappings/settings/query replay manifests;
   - replay against both engines;
   - report status, totals, top hits, order, and timing.

4. Define go/no-go thresholds:
   - shadow gate: 0 P0 compatibility mismatches, no `_bulk` timeout, no partial data
     after failed writes, p95 stable across repeated runs;
   - limited traffic gate: search p95 within an agreed budget and at least within
     10x OpenSearch on production-like fixtures;
   - replacement gate: durable persistence, restart/recovery, rollback runbook,
     and postings-backed execution for observed MatchID query shapes.

