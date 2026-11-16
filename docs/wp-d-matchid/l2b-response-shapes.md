# L2b — ES 8.6.1 response shapes for deces-backend

Lot: `plan/mvp-matchid.md` L2 (contract `deces_v2`), sub-lot L2b.
Owner bar: deces-backend outputs identical to those produced on ES 8.6.1
(timing fields excluded). Scores are L7; this lot covers the response
bytes around them.

Every expectation below is `es-source-derived`: written from the
Elasticsearch `v8.6.1` source (`server/src/main/java/org/elasticsearch/`),
not from an ES run. No ES 8.6.1 instance was run for this lot.

## Response families and their ES 8.6.1 source

| Family | ES emission order | ES source (v8.6.1) | Surch |
|---|---|---|---|
| search envelope | `_scroll_id`, `took`, `timed_out`, (`terminated_early`), `_shards`, `hits`, `aggregations` | `action/search/SearchResponse.java:274-301`, `action/search/SearchResponseSections.java:101-113` | `search.rs` `SearchResponse` (field declaration order) |
| `_shards` | `total`, `successful`, `skipped`, `failed`, (`failures`) | `rest/action/RestActions.java:79-103` | `SearchShards`, `CountShards` |
| hits | `total{value,relation}` (omitted when `track_total_hits:false`), `max_score` (`null` when NaN), `hits` | `search/SearchHits.java:173-198` | `SearchHits` |
| hit | `_index`, `_id`, `_score` (ALWAYS, `null` when NaN), meta fields, `_source`, `fields`, `highlight`, `sort`, `matched_queries` | `search/SearchHit.java:636-723` | `build_hit` + `append_sort_values` |
| `_score` with a field sort | score only if a `_score` sort clause exists, else `null`; `max_score` `null` unless `track_scores` | `action/search/SearchPhaseController.java:384-392, 421-428`, `search/query/TopDocsCollectorContext.java:278-292` | `paginate_hits`, `run_search_paths` |
| `max_score` with `size: 0` | `null` (`EmptyTopDocsCollectorContext`, NaN) | `search/query/TopDocsCollectorContext.java:447-455, 154` | `run_search` |
| `sort` values | raw sort value per clause; a `_score` value is a Java `Float` | `search/SearchSortValues.java:67-76` | `SortValueContext`, `es_json` formatter |
| `_source` with includes | byte-level filtering copy of the stored source: document key order, compact | `search/fetch/subphase/FetchSourcePhase.java` (`hitExecute`), `search/lookup/SourceFilter.java:63-107` | serde_json `preserve_order` + `filter_source_fields` (iterates the document) |
| msearch | `took`, `responses[]`; item = search body + `status` (error item = `error` + `status`) | `action/search/MultiSearchResponse.java:153-171` | `msearch.rs` |
| count | (`terminated_early`), `count`, `_shards` (`skipped: 0`) | `rest/action/search/RestCountAction.java:88-106` | `count.rs` (already in order) |
| scroll page | same as search; `_scroll_id` on every page, the empty last page included; page `max_score` = best score of the page | `SearchResponse.java:275-277`, `TopDocsCollectorContext.java:278-286` | `scroll_handler` |
| aggregations | request order | `search/aggregations/Aggregations.java:105-122` | `AggregationResults` (ordered `Vec`) |
| terms | `doc_count_error_upper_bound`, `sum_other_doc_count`, `buckets`; bucket `key`, `doc_count` (`key_as_string` only for non-RAW long formats) | `bucket/terms/AbstractInternalTerms.java:365-380`, `bucket/terms/InternalTerms.java:125-135`, `StringTerms.java:87-92`, `LongTerms.java:88-98` | `AggResult::Terms` |
| date_histogram | `buckets`; bucket `key_as_string`, `key` (epoch ms), `doc_count`; `min_doc_count` 0 fills gaps between first and last bucket | `bucket/histogram/InternalDateHistogram.java:119-134, 535-550` | `compute_date_histogram_aggregation` |
| composite | `after_key` BEFORE `buckets`; bucket `key` (source order), `doc_count`; date source key formatted with its `format`, else the epoch-ms long | `bucket/composite/CompositeAggregation.java:31-59`, `bucket/composite/InternalComposite.java:113-115, 173-178, 494-530` | `AggResult::Composite` |
| cardinality | `value` | `metrics/InternalCardinality.java:96-100` | `AggResult::Value` |
| `_cat/indices[/{index}]` | rows in table order; `h` = requested columns (names or aliases, `*`); JSON = array of objects with STRING values; text = padded table | `rest/action/cat/RestIndicesAction.java:61-62, 84, 254-270, 666-680`, `rest/action/cat/RestTable.java:40-113, 148-196, 300-410` | `cat.rs` |
| `creation.date.string` | `strict_date_time` print of `index.creation_date` in UTC (`2026-11-15T10:12:13.123Z`) | `RestIndicesAction.java:58, 678-680` | `cat_indices_cell`, `es_json::format_es_date` |

## Key order: `preserve_order`

serde_json without `preserve_order` stores objects in a `BTreeMap`, so
every `Value` was written with sorted keys (`_id` before `_index`, a
`_source` reordered alphabetically, composite keys sorted by source name).
The workspace now enables `serde_json/preserve_order`
(`Cargo.toml` `[workspace.dependencies]`): objects keep insertion / parse
order, which gives:

- the hit order by construction (`build_hit` inserts in ES order);
- `_source` in the order the document was indexed (parse order is kept
  through the stored bytes, `filter_source_fields` iterates the document);
- composite `key` / `after_key` in declared source order, cat rows in
  requested column order, msearch `took` before `responses`.

Workspace implications checked before enabling it:

- `serde_json::Value` grows from 32 to 72 bytes (`IndexMap` node; guarded
  by `es_json::tests::serde_json_preserve_order_is_enabled`). No `Value` is
  retained per document: `upsert_document_deferred` serializes the parsed
  source and drops it; only transient parses (bulk batches, hydrated hits,
  the general full-scan path) pay the larger nodes.
- `Map::remove` becomes `swap_remove` (reorders): no `Map::remove` on a
  `Value` object in the non-test sources of `surch-api`, `surch-index`,
  `surch-store`, `surch-types` (grep).
- clippy `result_large_err` fired in `opensearch-oracle` (errors carrying
  two `Value`s); allowed at crate level with the reason.
- `Value` equality stays order-insensitive (`IndexMap` `PartialEq`).
- Generators that write JSON with `json!` / `Map::insert` now follow
  insertion order: the BAN bulk generator (`opensearch-oracle/src/ban.rs`)
  sorts its keys explicitly so the committed `ban_tiny.ndjson` fixture
  stays byte-identical. Workspace test run: every other crate's tests pass
  unchanged.

Envelope structs (`SearchResponse`, `SearchHits`, `AggBucket`, ...) are
explicit ordered writers through their field declaration order and do not
depend on the feature.

## Behaviour changes in this lot

- Root `match_all` and a request without `query`: `_score` and `max_score`
  are the constant boost (1.0), as `MatchAllDocsQuery`; `min_score` applies
  to it (ES `MinimumScoreCollector`).
- `_score` is always written (`null` when Surch does not score the query).
- Field sorts: hits carry `sort`; `_score` / `max_score` are `null` unless
  a `_score` clause is in the sort.
- Keyword aggregation keys and sort values read the normalised term
  (`DEPARTEMENT_DECES` `2A` -> `2a`, `SOURCE` `2020-m03` -> `2020 m03`), as
  ES doc values.
- Numeric terms / composite keys sort numerically (`65 < 80 < 100`); the
  former decimal-string key put `100` first.
- `date_histogram` and composite date sources compute the bucket start in
  epoch ms from the field format (default `strict_date_optional_time||
  epoch_millis` for a `date` field; a non-`date` field keeps the legacy
  `yyyyMMdd` reading).
- `index.creation_date` is stamped in the index settings at creation
  (persisted with them in `META`, L6), reported by
  `GET /_cat/indices/{index}`, and visible as `settings.index.creation_date`
  in `GET /:index`.
- `GET /_cat/indices/{index}` exists (comma list, `*` patterns, aliases;
  404 for a missing concrete name) with `h`, `v`, `format`.
- `/version` (`{"sort":[{"SOURCE":"desc"},{"DATE_DECES.raw":"desc"}],
  "size":1}`, no query): served by `run_sorted_match_all_top`, a walk of
  the sort fields' term dictionaries in sort order intersecting postings;
  only the window's documents are hydrated. It declines (general path) for
  a query other than `match_all`, a non-keyword sort field, a `_score`
  clause, `min_score`, aggregations or a window above 1 000.

## deces-backend usage checks

- `match_phrase`: not emitted by `packages/deces-backend/src` (grep at
  matchID `6732755`: no occurrence). The bag-of-words `match_phrase`
  residual is left as is.
- `match_all`: emitted as the default `q` fallback and inside the geo
  `bool.must` (`buildRequest.ts:121`, `queries.ts:236-239`).
- `/version`: `status.controller.ts:62-114` (`count`, the sorted top-1
  search, `cat.indices({format:'json', h:'creation.date.string'})`), hit on
  every UI load (`deces-ui/src/App.svelte:32`).

## Deviations kept (not ES-identical)

- `took` and `_scroll_id` values (timing / opaque id) — excluded by the bar.
- Root `term` (and other non-`match` leaf) queries still report
  `_score: null` where ES reports BM25 / constant scores (L7).
- `cardinality` is exact; ES 8.6.1 HyperLogLog++ estimates above
  `precision_threshold` (default 3000), so large distinct counts differ.
- Floats inside `_source` are re-printed by serde_json (`1e21`), ES prints
  them with Jackson / `Double.toString` (`1.0E21`) when filtering and
  verbatim when not; the deces documents carry no float field.
- An unfiltered `_source` is the compact re-serialisation of the indexed
  document: identical to ES only when the indexer sent compact JSON (the
  ES bulk helpers do), not for pretty-printed input.
- Error envelopes (msearch error items, 4xx) lack ES `root_cause`.
- `_cat/indices`: no `s` row sort (rows by index name), `uuid`, `health`,
  sizes are placeholders; a request with neither `format` nor `Accept`
  gets JSON (ES: text); row order for multi-index answers follows index
  names (ES: settings map order).
- `terms` on a `date` field does not write `key_as_string` (ES `LongTerms`
  with a date format does); not emitted by deces-backend.

## Tests

- `crates/surch-api/tests/es_response_shapes.rs`: golden byte tests on the
  real mapping fixture (`tests/matchid_compat/deces/mapping_real.json`),
  `took` / `_scroll_id` masked; 11 of the 12 tests carry an
  `es-source-derived` expectation (the twelfth checks that the creation
  date survives a restart).
- `crates/surch-api/src/search_l2b_tests.rs`: the sorted `match_all` walk
  equals the general path byte for byte on the single-RAM, multi-segment
  and disk-backed layouts, with deleted documents and documents missing a
  sort field; declines the other shapes.
- `crates/surch-api/src/es_json.rs`: date print / parse
  (`strict_date_time`, `yyyyMMdd`, `epoch_millis`) and the
  `preserve_order` guard.
