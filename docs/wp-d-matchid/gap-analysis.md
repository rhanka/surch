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
| **A5** | `function_score` wrapper | advanced + block match | gap | _pending_ | declarative-only: `field_value_factor`, decay functions, `score_mode`, `boost_mode`; no scripts |
| **A6** | `prefix` query + `index_prefixes` mapping option | autocomplete | gap | _pending_ | postings-side prefix iterator; mapping option drives a min/max ngram-on-write fan-out |
| **A7** | `range` on `date{format:yyyyMMdd}` | date filters | gap | _pending_ | needs A13 first (date field type) |
| **A8** | `match_all` | default + filter context | implemented | `crates/surch-api/src/search.rs::parse_match_all_query` | accepts `{}` and `{ "boost": N }`; contributes `boost` (default 1.0) to bool-must sums; standalone form keeps `_score = null` parity |
| **A9** | `from` + `size` pagination | result paging | gap | _pending_ | TopN currently slices size; from-offset needed |
| **A10** | `sort` over keyword / normalised-date sub-fields | UI table | gap | _pending_ | needs sub-field accessors (NOM.raw, DATE_NAISSANCE_NORM) |
| **A11** | `min_score` top-level body filter | full-text | gap | _pending_ | drop hits with `_score < min_score` after scoring |
| **A12** | Aggregations | analytics tab | gap | _pending_ | terms, date_histogram, composite (+ after_key), cardinality |
| **A13** | Mapping primitives | index time | gap | _pending_ | `edge_ngram` tokenizer + analyzer, custom `normalizer` (lowercase + asciifolding), `index_prefixes`, `geo_point`, `date{format}` |
| **A14** | ES-7.x response shape | client compat | gap | _pending_ | `hits.total.value` / `hits.total.relation`, `_scroll_id`, `aggregations.<name>.buckets` |
| **A15** | `_msearch` inner DSL parity | block-match | partial | `crates/surch-api/src/msearch.rs` (NDJSON wrapper shipped) | inner-query failures are the same as A1..A12 |
| **B1** | matchID replay fixture | acceptance | gap | _pending_ | 30 representative searches → top-10 hit-ids captured against OS 2.17 |
| **B2** | INSEE 10k slice frozen fixture | acceptance | gap | _pending_ | publish under `tests/matchid_compat/deces/` |
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
