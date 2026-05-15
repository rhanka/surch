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

| Gap | Requirement (batch) | Workload | Status | Surch artifacts | Notes |
|---|---|---|---|---|---|
| **A1** | `match` object form with `fuzziness` sub-field | interactive lookup | gap | _pending_ | extend `parse_match_query` in `crates/surch-api/src/search.rs` |
| **A2** | `geo_point` field type + `geo_distance` query | UI "near city X" filter | gap | _pending_ | new field type in `surch-index/mapping.rs`, new query in `surch-api/src/search.rs` |
| **A3** | `bool.filter`, `bool.should`, `minimum_should_match` | interactive lookup | gap | _pending_ | extend `parse_bool_query` |
| **A4** | Stateful scroll API | bulk CSV path | gap | _pending_ | new module `surch-api/src/scroll.rs` with `scroll_id` table + TTL |
| **B1** | matchID replay fixture | acceptance | gap | _pending_ | capture deterministic top-hit ids against OS 2.17, add to `tests/matchid_compat/` |
| _confirmed_ | `match`, `multi_match`, `bool.must`, `range`, `term`, `fuzzy` standalone | interactive lookup | implemented | `crates/surch-api/src/search.rs` parsers + executors | matched against `tests/opensearch_compat/` replay |
| _confirmed_ | highlight | response shaping | implemented | `crates/surch-api/src/search.rs` highlight path | |
| _confirmed_ | `_search?track_total_hits=`, `_mget`, `_count` | id lookup + total | implemented | `crates/surch-api/src/{search,mget,count}.rs` | |

## How to read this table

- `Gap` = stable id used in commit subjects and PR titles
- `Requirement (batch)` = which `incoming/` file introduced it
- `Workload` = which matchID code path triggers it (1-2 words)
- `Surch artifacts` = source files, test fixtures and bench reports
  that close the gap; bullet-list when several
- `Notes` = link to the decision file under
  `docs/wp-d-matchid/decisions/` and any scope adjustment we agreed on
