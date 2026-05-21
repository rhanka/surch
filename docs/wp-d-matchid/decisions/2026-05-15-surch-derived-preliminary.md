# Decision — Surch-derived preliminary requirements

Date: 2026-05-15
Intake file: `docs/wp-d-matchid/incoming/2026-05-15-surch-derived-preliminary.md`

## Status

**Accepted as a working baseline** until matchID writes its own intake
file. The Surch-side requirements file is provisional; every clause
remains negotiable when matchID confirms.

Supersession note: the B1 oracle target is now Elasticsearch 8.6.1, not
the provisional OpenSearch 2.17 baseline used before the matchID
Elasticsearch oracle was available. Active proof: `ci-k8s` run
`26192816780`, 30 requests, 0 skipped, 0 divergence.

## Triage

| § from incoming/ | Surch status today | Decision |
|---|---|---|
| 2.1 — name multi_match + fuzziness | match + multi_match shipped, `fuzzy` query type shipped, **fuzzy as a sub-field of `match` (`{ "match": { "f": { "query": "x", "fuzziness": "AUTO" } } }`) is missing** | **gap-A1** : extend `parse_match_query` to accept the object form |
| 2.2 — city match | covered by §2.1 once A1 lands | n/a |
| 2.3 — date range on YYYYMMDD keyword | `range` query shipped against keyword fields; need to confirm DD/MM/YYYY → YYYYMMDD translation lives in `deces-backend` (Node-side), **not** in Surch | confirm, no gap on Surch |
| 2.4 — `geo_distance` on `geo_point` | **not implemented** | **gap-A2** : new `geo_point` field type + `geo_distance` query |
| 2.5 — term filters | `term` shipped; `bool.filter` ↔ `bool.must` semantics need a documented mapping | **gap-A3** : add `bool.filter` (does not score) and `bool.should` / `minimum_should_match` |
| 2.6 — scroll API | **not implemented** (`/_search?scroll=1m` + `/_search/scroll`) | **gap-A4** : stateful scroll context with TTL eviction |
| 3 — `_source` shape | matched by INSEE NDJSON we already ingest | n/a |
| 4 — acceptance criteria | artillery harness lands JSON since wp/b round 3; matchID replay fixture missing | **gap-B1** : `tests/matchid_compat/replays/deces_v1.json` with deterministic top-hit IDs now cross-checked against Elasticsearch 8.6.1; the original OpenSearch 2.17 capture was provisional |
| 5 — out of scope | mirrored in `docs/wp-d-matchid/SPEC.md` § "Out-of-scope reminders" | n/a |

## Implementation order

By increasing risk / cost :

1. **A3 — `bool.filter` / `bool.should` / `minimum_should_match`**
   (0.5 d). Pure surface-level extension of `parse_bool_query`.
2. **A1 — `fuzzy` inside `match` object form** (0.5 d). Extra
   `serde_json::Value` branch; the fuzziness `Damerau-Levenshtein`
   logic already exists in `surch-search`.
3. **B1 — matchID replay fixture** (1 d). Captures deterministic
   queries and bakes the expected top-hit ids; active oracle proof is
   Elasticsearch 8.6.1 via `ci-k8s` run `26192816780`, gated by A3 +
   A1.
4. **A4 — scroll API** (3 d). Needs a `scroll_id → cursor` table on
   `AppState` with TTL, plus `POST /_search/scroll` route, plus the
   `?scroll=1m` form on `_search`.
5. **A2 — `geo_point` + `geo_distance`** (3 d). New field type + new
   query type + scoring rule (`distance` is a constant boost, not a
   BM25 term).

Each implementation lands on `wp/a-optim` (A items) or `wp/b-test-auto`
(B items) with its commit subject citing this decision file.
