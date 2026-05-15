# Preliminary requirements — Surch-derived from public matchID code

Status: **draft, pending matchID confirmation**. This file is Surch-side
synthesis of what the `deces-backend` HTTP surface implies it expects from
the underlying ES-compatible engine. It lives in `incoming/` so it can be
referenced by `SPEC.md` and `gap-analysis.md`; the real intake from
matchID will land as a separate dated file and supersede this one.

Sources :

- `https://deces.matchid.io/deces/api/v1/search` (live API probed during
  earlier Surch investigation)
- `https://github.com/matchid-project/deces-backend` (public source code;
  archived but production-relevant)
- `backend/tests/performance/scenarios/test-backend-v1.yml` (artillery
  scenario already replayed by `scripts/bench/artillery-replay.sh` and
  `crates/surch-demo/src/bin/artillery_bench.rs`)
- INSEE Deces NDJSON already loaded under `target/insee/` for the bench

## 1. Workload context

`deces-backend` is a Node.js service that exposes a custom JSON wrapper
over an ES backend. End users hit it through the matchID UI (browser),
through batch `multipart` CSV uploads, and through HTTP API calls. Three
high-level workloads matter:

1. **Interactive lookup**: GET / POST `/deces/api/v1/search` with a
   subset of `{firstName, lastName, birthDate, deathDate, birthCity,
   deathCity, sex, fuzzy}`. ~50 RPS sustained per the artillery
   scenario, p95 budget 200 ms / max 500 ms / error rate ≤ 1 %.
2. **Bulk CSV download**: POST `/deces/api/v1/search/csv` (multipart
   upload of a queries CSV). The backend internally drives a `scroll`
   loop over the underlying ES engine until the cursor exhausts or
   `total < 500 000`.
3. **Direct id lookup**: GET `/deces/api/v1/id/{id}` — a `_get` /
   `_mget` against the ES backend.

## 2. ES queries we infer the backend sends

### 2.1 Name + birthDate match (the artillery v1 mix)

For every interactive lookup, the backend builds a `bool.must` over a
multi-field name match + an optional date filter. Concretely, with
`firstName=Jean`, `lastName=Dupont`, `birthDate=24/06/1905` :

```json
{
  "size": 20,
  "track_total_hits": true,
  "query": {
    "bool": {
      "must": [
        { "match": { "PRENOMS": { "query": "Jean",   "fuzziness": "AUTO" } } },
        { "match": { "NOM":     { "query": "Dupont", "fuzziness": "AUTO" } } },
        { "term":  { "DATE_NAISSANCE": "19050624" } }
      ]
    }
  }
}
```

Fuzziness toggle is exposed to clients via the `fuzzy=false|true|N`
query string parameter and forwarded as `fuzziness: "AUTO" | "0" | N`.

### 2.2 City match

`birthCity` / `deathCity` map to `match` queries over
`COMMUNE_NAISSANCE` / `COMMUNE_DECES` with the same fuzziness handle.

### 2.3 Date range

`birthDate` accepts `>DD/MM/YYYY`, `<DD/MM/YYYY`, `DD/MM/YYYY-DD/MM/YYYY`.
The backend translates these to `range` queries on
`DATE_NAISSANCE` / `DATE_DECES` keyed as `YYYYMMDD` strings.

### 2.4 Geo

`birthGeoPoint` / `deathGeoPoint` are exposed in `_source` and accept
queries like `birthGeoPoint=lat,lon,radius` which the backend translates
to `geo_distance` on `GEOPOINT_NAISSANCE` / `GEOPOINT_DECES`. Used
mostly by the UI for "near …" filters.

### 2.5 Sex / source / department filters

`sex`, `source`, `birthDepartment`, `deathDepartment` translate to
`term` filters under `bool.filter`.

### 2.6 Scroll loop (bulk path)

The bulk CSV path repeats:

```http
POST /<index>/_search?scroll=1m
{ "size": 1000, "query": { ... } }
```

then:

```http
POST /_search/scroll
{ "scroll": "1m", "scroll_id": "<prev>" }
```

until the cursor is exhausted. Total fetched is the response `total`.

## 3. Document shape (ES `_source`)

```
PRENOMS                text  (analysed, accent-folded)
NOM                    text  (analysed, accent-folded)
SEXE                   keyword (single char, 1 = M, 2 = F)
DATE_NAISSANCE         keyword (YYYYMMDD, lex-sortable)
COMMUNE_NAISSANCE      text  (analysed)
CODE_INSEE_NAISSANCE   keyword
GEOPOINT_NAISSANCE     geo_point ({lat, lon})
DATE_DECES             keyword (YYYYMMDD)
NUM_DECES              keyword
AGE_DECES              integer
COMMUNE_DECES          text
GEOPOINT_DECES         geo_point
SOURCE                 keyword (e.g. INSEE_2024)
SOURCE_LINE            integer
```

The custom backend then layers its own composite score
(`scores.name`, `scores.birthLocation`, `scores.es`) computed in
Node — Surch does **not** need to reproduce that maths, it only needs
to return ES `_score` plus the requested `_source` fields.

## 4. Acceptance criteria

The portage is "good enough" when, on the same INSEE `Deces_2024 +
Deces_2025` dataset (~1.3 M docs) :

- the artillery `test-backend-v1.yml` scenario (50/50 mix, 2 → 50 RPS,
  4 min) passes `p95 < 200 ms`, `max < 500 ms`, error rate < 1 %, with
  Surch in place of OpenSearch behind `deces-backend`;
- top-hit id for at least 100 reference deterministic queries (to be
  captured in a replay fixture under `tests/matchid_compat/`) is the
  same as OpenSearch 2.17.1;
- the scroll path drains a 500 k-doc cursor in under 60 s.

## 5. Out of scope for this batch

- ML rerank / learning-to-rank.
- Aggregations (facets) — `deces-backend` does its own faceting in Node.
- `function_score` / `script_score` — backend handles composite scoring
  in Node.
- Cross-cluster search.
- Document update / delete by query (matchID re-indexes from scratch).
- Custom analyzers beyond `surch-analysis` (accent fold + lowercase is
  enough for the name fields).
