# DSL translation matrix — deces-backend ↔ Surch

Gap-by-gap mapping of every OpenSearch query / mapping / response
primitive emitted by `deces-backend` against ES 7.x today, and the
Surch equivalent that closes it. One row per `Gap` id from
`docs/wp-d-matchid/gap-analysis.md`. ES wire shapes are copy-pasted
verbatim from
`docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
(§2.1 to §2.12). Surch-side implementation notes are sized for matchID
integrators: they describe **what changes for matchID**, not how Surch
implements it.

Status legend (mirrors `gap-analysis.md`):

- **gap** — Surch does not implement this primitive yet
- **in flight (round 5)** — landing on `wp/d-matchid` or `wp/b-test-auto`
  during round 5 (A1, A3, A8, B1, B2, C-SNAPSHOT-RAW)
- **partial** — Surch implements a subset; remaining mismatch noted
- **implemented** — on `main` and gated by at least one cargo test or
  replay fixture

## Query DSL primitives

| Gap | ES wire shape (verbatim) | Surch equivalent | Status | Depends-on |
|---|---|---|---|---|
| **A1** | `{ "match": { "PRENOMS": { "query": "JEAN", "fuzziness": "AUTO" } } }` | identical wire shape once A1 lands; `fuzziness: "AUTO"\|"0"\|N` accepted | in flight (round 5) | — |
| **A2** | `{ "geo_distance": { "distance": "1km", "GEOPOINT_NAISSANCE": { "lat": 48.85, "lon": 2.35 } } }` | identical; units `km`, `m`, `mi`, `yd`, `ft`, `NM` accepted | gap | A13 (`geo_point` mapping) |
| **A3** | `{ "bool": { "filter": [ … ], "should": [ … ], "minimum_should_match": 1, "boost": 2 } }` | identical; `filter` clauses run in filter-context (no score), `boost` is a float multiplier | in flight (round 5) | — |
| **A4** | `POST /:index/_search?scroll=1m` then `POST /_search/scroll { "scroll": "1m", "scroll_id": "…" }` | identical URL + body; Surch returns `_scroll_id` and accepts the same lifetime grammar | gap | A14 (response shape) |
| **A5** | `{ "function_score": { "query": { … }, "functions": [ … ], "score_mode": "sum", "boost_mode": "multiply" } }` | identical; declarative-only (`field_value_factor`, `gauss`/`exp`/`linear` decay, `score_mode`, `boost_mode`); **no `script_score`** | gap | — |
| **A6** | `{ "prefix": { "DATE_NAISSANCE": "1962" } }` | identical; backed by postings-side prefix iterator when `index_prefixes` is declared on the field | gap | A13 (`index_prefixes`) |
| **A7** | `{ "range": { "DATE_NAISSANCE": { "gte": "19620101", "lte": "19620931" } } }` | identical; `gte`/`lte` only, no `gt`/`lt`, no date-math; open-ended forms accepted | gap | A13 (`date{format}`) |
| **A8** | `{ "match_all": {} }` (as default and inside `bool.must`) | identical; matches every doc with `_score = 1.0` in score-context, free in filter-context | in flight (round 5) | — |
| **A9** | `{ "from": 40, "size": 20 }` (top-level) | identical pagination semantics; TopN window grows to `from + size` then slices | gap | — |
| **A10** | `{ "sort": [ { "DATE_NAISSANCE_NORM": "asc" }, { "NOM.raw": "asc" } ] }` | identical; sort over keyword sub-fields and normalised-date sub-fields; ties broken by `_doc` | gap | A13 (sub-field accessors) |
| **A11** | `{ "min_score": 5, "query": { … } }` (top-level) | identical; hits with `_score < min_score` dropped after scoring, before `from`/`size` | gap | — |
| **A12** | `{ "aggs": { "names": { "terms": { "field": "NOM.raw", "size": 100 } }, "bucketResults": { "composite": { … , "after": { … } } } } }` | identical; `terms`, `date_histogram` (`calendar_interval`, `format`), `composite` (`after_key` round-trip), `cardinality` | gap | A13 |
| **A15** | `_msearch` NDJSON: header line + body line per sub-query, each body is one of A1..A12 | identical; Surch already accepts the NDJSON envelope, inner-query parity follows A1..A12 | partial | A1..A12 |

## Mapping / index-time primitives

| Gap | ES wire shape (verbatim) | Surch equivalent | Status | Depends-on |
|---|---|---|---|---|
| **A13** | `tokenizer: edge_ngram_tokenizer { type: edge_ngram, min_gram: 2, max_gram: 20 }`, `analyzer: { autocomplete_analyzer, norm }`, `normalizer: norm { filter: [lowercase, asciifolding] }`, `index_prefixes: { min_chars: 2, max_chars: 10 }`, `type: geo_point`, `type: date, format: yyyyMMdd` | identical YAML/JSON accepted by `PUT /:index`; `edge_ngram` and `norm` analyzers ship under `surch-analysis` once A13 lands | gap | — |

## Response shape primitives

| Gap | ES wire shape (verbatim, what deces-backend reads back) | Surch equivalent | Status | Depends-on |
|---|---|---|---|---|
| **A14** | `{ "hits": { "total": { "value": 42, "relation": "eq" }, "hits": [ … ] }, "_scroll_id": "…", "aggregations": { "<name>": { "buckets": [ … ], "after_key": { … } } } }` | identical ES 7.x shape; `_scroll_id` only when `?scroll=` was set; `aggregations.<name>_count.value` for `cardinality` | gap | A4, A12 |

## Test-automation primitives

| Gap | What it tests | Surch artifact | Status | Depends-on |
|---|---|---|---|---|
| **B1** | Top-10 hit-id parity against OS 2.17.1 for ~30 representative searches (advanced + block + fullText + UI + bulk-match) | `tests/matchid_compat/replays/deces_v1.json` plus runner in `crates/surch-tests-matchid-compat/` | gap | A1, A3, A8 |
| **B2** | INSEE 10k frozen slice (deces-2020-m01) used as fixture corpus for B1 and the NDCG@10 budget | `tests/matchid_compat/deces/insee_10k.ndjson.zst` plus checksum manifest | gap | A13 |

## Implementation notes (per gap, ≤ 30 words for matchID)

- **A1.** matchID emits today; Surch will accept the object form
  verbatim. Identical scoring to ES BM25; `fuzziness: "AUTO"` maps to
  Damerau-Levenshtein edit distance 1 for short terms, 2 for long.
- **A2.** Surch will score `geo_distance` hits with a **constant**
  `_score = 1.0` (it is a filter, not a ranker). deces-backend already
  ignores `_score` for geo-filtered paths.
- **A3.** Surch enforces ES semantics: `filter` clauses do not
  contribute to `_score`; `should` clauses are score-only unless
  `minimum_should_match` makes them mandatory.
- **A4.** Scroll context lives in-process with a TTL (default `1m`,
  user-extendable). Surch is single-node, so no shard-routing concern.
- **A5.** `function_score` parses and evaluates declaratively; absence
  of `functions[]` is treated as identity (matches the no-op shape
  deces-backend emits today).
- **A6.** `index_prefixes` adds a write-time fan-out (n-grams of length
  `min_chars..=max_chars`). Surch storage cost ≈ 1.2× the parent text
  field — budget the disk.
- **A7.** `range` on `date{format: yyyyMMdd}` is lexicographic on the
  string sub-field. matchID's existing DD/MM/YYYY → YYYYMMDD translation
  in Node stays unchanged.
- **A8.** Identity match; constant `_score = 1.0` in score-context,
  free in filter-context. No change for matchID.
- **A9.** `from + size` slicing; Surch caps the deep-paging window at
  10 000 by default (configurable), same as OS default
  `index.max_result_window`.
- **A10.** Sort over keyword sub-fields (`NOM.raw`) and normalised-date
  sub-fields (`DATE_NAISSANCE_NORM`). Ties broken by `_doc` (stable
  insertion order).
- **A11.** `min_score` applied after scoring, before
  `from`/`size`/`sort`. Identical to ES.
- **A12.** `composite.after_key` round-trips byte-for-byte; iterating
  page-by-page partitions the keyspace deterministically.
- **A13.** Mapping changes are **index-time**: PUT `/:index` with the
  `deces_index.yml` verbatim. No runtime impact once the index is
  built.
- **A14.** ES 7.x shape (`hits.total.{value,relation}`). matchID
  already targets 7.x — no client change.
- **A15.** Surch already accepts `_msearch` NDJSON since round 4
  (`crates/surch-api/src/msearch.rs`); inner-query parity follows the
  A1..A12 schedule.
- **B1.** First gate matchID checks: top-10 hit-id parity on 30
  curated queries. Ties allowed inside the same `_score` bucket.
- **B2.** 10k-row frozen INSEE slice (deces-2020-m01). Will land under
  `tests/matchid_compat/deces/` with a zstd-compressed NDJSON file and
  a SHA-256 manifest.

## Test-gate file map (where each gap is exercised in Surch)

| Gap | Test gate file (Surch repo) |
|---|---|
| A1 | `tests/matchid_compat/replays/deces_v1.json` (once B1 lands) + `crates/surch-api/tests/search_match.rs` |
| A2 | `tests/matchid_compat/replays/deces_v1.json` (geo subset) + `crates/surch-api/tests/search_geo.rs` |
| A3 | `tests/matchid_compat/replays/deces_v1.json` (bool subset) + `crates/surch-api/tests/search_bool.rs` |
| A4 | `crates/surch-api/tests/scroll_lifecycle.rs` |
| A5 | `crates/surch-api/tests/function_score.rs` |
| A6 | `crates/surch-api/tests/prefix.rs` |
| A7 | `crates/surch-api/tests/range_date.rs` |
| A8 | `crates/surch-api/tests/match_all.rs` |
| A9 | `crates/surch-api/tests/pagination.rs` |
| A10 | `crates/surch-api/tests/sort.rs` |
| A11 | `crates/surch-api/tests/min_score.rs` |
| A12 | `crates/surch-api/tests/aggregations.rs` |
| A13 | `crates/surch-api/tests/mapping_deces.rs` |
| A14 | `crates/surch-api/tests/response_shape_es7.rs` |
| A15 | `crates/surch-api/tests/msearch_inner_dsl.rs` |
| B1 | `tests/matchid_compat/replays/deces_v1.json` + `crates/surch-tests-matchid-compat/src/lib.rs` |
| B2 | `tests/matchid_compat/deces/insee_10k.ndjson.zst` + `crates/surch-tests-matchid-compat/tests/insee_10k_smoke.rs` |

Test gate files are **proposed names** for files that do not yet exist
on `main`. They will land alongside the implementation commit for each
gap and are referenced here so matchID can grep for them once a gap
flips to `implemented`.
