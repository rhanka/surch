# Surch ⟷ matchID gap analysis

Rolling table of every matchID requirement filed in
`docs/wp-d-matchid/incoming/`, mapped to its Surch-side implementation
status. Updated on every commit that affects this WP.

Status legend:

- **gap** — matchID needs it, Surch does not implement it yet
- **partial** — Surch implements a subset; mismatch documented in the
  decision file
- **implemented** — Surch implements it on `main`, gated by at least
  one of: cargo test, the SciFact NDCG@10 gate, the INSEE artillery
  harness, the matchID replay fixture under `tests/matchid_compat/`
- **declined** — out of scope by mutual agreement; decision recorded
  under `decisions/`

| Gap | Title | Workload | Status | Surch artifacts | Notes |
|---|---|---|---|---|---|
| **A1** | `match` object form with `fuzziness` sub-field | name lookup | implemented | `crates/surch-api/src/search.rs::parse_match_query` | object body routes to `SearchQuery::Fuzzy`; AUTO low=3 high=6 (edits=1 for <6-char terms, edits=2 otherwise) |
| **A2** | `geo_point` + `geo_distance` | UI "near X" filter | gap | _pending_ | new field type + new query + scoring rule |
| **A3** | `bool.filter` / `bool.should` / `minimum_should_match` / clause `boost` | every advanced search | implemented | `crates/surch-api/src/search.rs` (`SearchQuery::Bool`, `parse_bool_query`, `parse_minimum_should_match`, `parse_boost`); `crates/surch-api/tests/search.rs` (10 `bool_*` cases incl. the matchID nested-should/boost shape) | filter intersects without scoring; should supports integer + `"N%"` MSM (default 1 when only `should` is present); clause `boost` multiplies the `Bool` `_score`; `must_not` and per-leaf clause boost left for a follow-up |
| **A4** | Scroll API | bulk-match | gap | _pending_ | new `surch-api/src/scroll.rs` with TTL'd `scroll_id` table |
| **A5** | `function_score` wrapper | advanced + block match | partial | `crates/surch-api/src/search.rs` (`SearchQuery::FunctionScore`, `parse_function_score_query`); `crates/surch-api/tests/search.rs` (3 `search_router_a5_*` cases) | no-op wrapper shipped: parses `function_score { query, boost?, functions?: [], score_mode?, boost_mode?, max_boost?, min_score? }` and forwards to the inner query (multiplied by `boost`, default 1.0). Empty `functions: []` accepted (matchID's current shape). Non-empty `functions` rejected with HTTP 400 — `field_value_factor`, decay, weight, etc. tracked as "function_score phase 2" |
| **A6** | `prefix` query + `index_prefixes` mapping option | autocomplete | gap | _pending_ | postings-side prefix iterator; mapping option drives a min/max ngram-on-write fan-out |
| **A7** | `range` on `date{format:yyyyMMdd}` | date filters | gap | _pending_ | needs A13 first (date field type) |
| **A8** | `match_all` | default + filter context | implemented | `crates/surch-api/src/search.rs::parse_match_all_query` | accepts `{}` and `{ "boost": N }`; contributes `boost` (default 1.0) to bool-must sums; standalone form keeps `_score = null` parity |
| **A9** | `from` + `size` pagination | result paging | implemented | `crates/surch-api/src/search.rs` (`run_topk_search`, `paginate_hits`, `MAX_RESULT_WINDOW`); `crates/surch-api/tests/search.rs` (3 `search_router_a9_*` cases) | from/size already wired in the topk shortcut + full-scan paths; this batch adds the ES-7.x `index.max_result_window = 10 000` cap (returns HTTP 400 `search_phase_execution_exception` when `from + size > 10 000`) and pins the contract via tests |
| **A10** | `sort` over keyword / normalised-date sub-fields | UI table | partial | `crates/surch-api/src/search.rs` (`sort_scored_documents`, `compare_sort_clause`, `lookup_sort_value`); `crates/surch-api/tests/search.rs` (4 `search_router_a10_*` cases) | parent-field sort (asc/desc, multi-clause, missing-last, `_score` tie-break) was already wired; this batch adds the `NOM.raw` / `DATE_NAISSANCE.norm` sub-field → parent alias so matchID's wire shape sorts deterministically even before A13 ships real multi-fields. Once A13 lands, the alias becomes a no-op (real sub-field beats the parent in `as_object().get`) |
| **A11** | `min_score` top-level body filter | full-text | implemented | `crates/surch-api/src/search.rs` (`SearchRequest::min_score`, `parse_min_score`, filter in `run_search`); `crates/surch-api/tests/search.rs` (4 `search_router_a11_*` cases) | accepts non-negative finite f64; filter applied only when scoring is enabled (no-op on `match_all` / `term` / `range` standalone, per ES 7.x); total hits reflects post-filter count; top-K shortcut falls back to full-scan when `min_score` is set |
| **A12** | Aggregations | analytics tab | gap | _pending_ | terms, date_histogram, composite (+ after_key), cardinality |
| **A13** | Mapping primitives | index time | gap | _pending_ | `edge_ngram` tokenizer + analyzer, custom `normalizer` (lowercase + asciifolding), `index_prefixes`, `geo_point`, `date{format}` |
| **A14** | ES-7.x response shape | client compat | partial | `crates/surch-api/src/search.rs::resolve_total_hits`, `SearchHitsTotal`; `crates/surch-api/tests/search.rs` (4 `search_router_a14_*` cases) | `hits.total.{value,relation}` matches ES-7.x: `"eq"` when uncapped, `"gte"` when capped by `track_total_hits=N` or by the default 10 000-doc cap. `_scroll_id` (A4) and `aggregations.<name>.buckets` (A12) remain out of scope here and are tracked under their own gaps. |
| **A15** | `_msearch` inner DSL parity | block-match | partial | `crates/surch-api/src/msearch.rs` (NDJSON wrapper shipped) | inner-query failures are the same as A1..A12 |
| **B1** | matchID replay fixture | acceptance | implemented (partial — 7 tests passing, 3 skipped on gaps A4/A6) | `tests/matchid_compat/replays/deces_v1.json`, `crates/surch-api/tests/matchid_compat.rs` | v0: 3 advanced + 2 block-match + 2 full-text execute against Surch; 2 prefix (gap A6) + 1 scroll (gap A4) skipped. Expectations captured against Surch HEAD on 2026-05-15; ES-7.x cross-check pending. Target stays 30 representative searches; will grow as A-series gaps land. |
| **B2** | INSEE 10k slice frozen fixture | acceptance | implemented (partial — 1000-doc synthetic slice ~270 kB) | `tests/matchid_compat/deces/{mapping.json,slice-1000.ndjson,README.md}`, `tools/gen_deces_slice.awk` | v0 mapping restricted to `text`/`keyword`/`integer`; will be widened to multi-fields + `date{format}` + `geo_point` + `edge_ngram`/`normalizer` when gaps A1/A2/A6/A7/A13 land. Slice is synthetic (deterministic AWK seed) — switch to a real INSEE `deces-2020-m01.txt.gz` 10k extract when matchID publishes it. |
| _confirmed_ | `match`, `multi_match`, `bool.must`, `term`, `fuzzy` standalone | interactive lookup | implemented | `crates/surch-api/src/search.rs` parsers + executors | matched against `tests/opensearch_compat/` replay |
| _confirmed_ | highlight | response shaping | implemented | `crates/surch-api/src/search.rs` highlight path | |
| _confirmed_ | `_search?track_total_hits=`, `_mget`, `_count` | id lookup + total | implemented | `crates/surch-api/src/{search,mget,count}.rs` | |

## How to read this table

- `Gap` = stable id used in commit subjects and PR titles
- `Workload` = which matchID code path triggers it (1-2 words)
- `Surch artifacts` = source files, test fixtures and bench reports
  that close the gap; bullet-list when several
- `Notes` = link to the decision file under
  `docs/wp-d-matchid/decisions/` and any scope adjustment we agreed on
