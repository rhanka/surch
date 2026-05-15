# 2026-05-15 — deces-backend query-DSL inventory

> Intake batch: enumerate every OpenSearch DSL primitive that the
> `deces-backend` workload emits today against Elasticsearch 7.x and
> that Surch does not yet implement. One batch on purpose — the Surch
> maintainers can split into focused intake files when scheduling.
> Wire shapes below are copy-pasted verbatim from
> `matchID/packages/deces-backend/src/queries.ts` and
> `buildRequest.ts`; only PII has been redacted.

## 1. Workload context

- **Repo / branch:** `matchID-project/matchID` →
  `experiment/surch-replace-elastic`.
- **Code paths:**
  - `packages/deces-backend/src/buildRequest.ts` — builds the search
    body, two flavours: `buildAdaptativeBlockMatch` (bulk matching)
    and `buildAdvancedMatch` (interactive UI search).
  - `packages/deces-backend/src/queries.ts` — primitive helpers
    (`fuzzyTermQuery`, `fuzzyShouldTermQuery`, `nameQuery`,
    `dateRangeStringQuery`, `ageRangeStringQuery`, `geoPointQuery`,
    `prefixQuery`).
  - `packages/deces-backend/src/runRequest.ts` — issues the queries:
    `client.search`, `client.scroll`, `client.msearch`.
- **Index:** `deces` — 25M+ INSEE death records, schema in
  `packages/deces-dataprep/projects/deces-dataprep/datasets/deces_index.yml`.
  Mapping uses `edge_ngram` analyzer, `index_prefixes`, custom
  `normalizer`, `geo_point`, `date` with `format: yyyyMMdd`.
- **Volume:** ~50 RPS sustained on the public `deces.matchid.io`
  endpoint, ~600 RPS peak during artillery rehearsals; bulk-match
  jobs issue `msearch` with 5–10 sub-queries per request and use
  `scroll` to walk results.
- **Today's error against Surch:** every search body fails at parse
  time on `function_score`, `bool` clauses (`must`/`should`/
  `minimum_should_match`), `prefix`, `range`, `geo_distance`,
  `match_all`-as-filter, plus index-time errors on the mapping
  (`edge_ngram`, `normalizer`, `geo_point`, `date{format}`,
  `index_prefixes`). Bulk runtime errors on `_scroll`, `_msearch`,
  and composite/cardinality/date_histogram aggregations.

## 2. OpenSearch wire shapes (verbatim, deces-backend → ES 7.x)

### 2.1 `bool` compound query (everywhere)

All non-trivial searches compose with `bool` + `must` / `should` +
`minimum_should_match` + clause-level `boost`. Example from
`queries.ts::nameQuery` with `fuzzy=auto`:

```json
{
  "bool": {
    "minimum_should_match": 1,
    "should": [
      {
        "bool": {
          "should": [
            { "match": { "PRENOMS.first":  { "query": "JEAN",   "fuzziness": "auto" } } },
            { "match": { "NOM.raw":          "DUPONT" } }
          ],
          "minimum_should_match": 2,
          "boost": 2
        }
      },
      {
        "bool": {
          "should": [
            { "match": { "PRENOMS.first":  { "query": "DUPONT", "fuzziness": "auto" } } },
            { "match": { "NOM.raw":          "JEAN" } }
          ],
          "minimum_should_match": 2,
          "boost": 0.5
        }
      }
    ]
  }
}
```

### 2.2 `function_score` wrapper (every advanced + block match)

Both `buildAdvancedMatch` and `buildAdaptativeBlockMatch` wrap the
top-level query in `function_score`. Today the wrapper is
**no-op-shaped** (no scoring functions declared) but the keyword must
parse and behave as identity scoring:

```json
{
  "function_score": {
    "query": {
      "bool": {
        "must":  [ /* … */ ],
        "should": [ /* … */ ],
        "minimum_should_match": 1
      }
    }
  }
}
```

> Future work (separate intake): real scoring functions
> (`field_value_factor`, `gauss` on `DATE_DECES_NORM`,
> `script_score`). For this batch we only need the wrapper to parse
> and forward to the inner query.

### 2.3 `match` with `fuzziness` (incl. `"auto"`)

```json
{ "match": { "PRENOMS_NOM": { "query": "JEAN DUPONT", "fuzziness": "auto" } } }
```

`fuzziness` values seen in the codebase: `"auto"` and integer `1`.
Used jointly with clause-level `boost` (numeric).

### 2.4 `prefix`

```json
{ "prefix": { "DATE_NAISSANCE": "1962" } }
```

Triggered by short date inputs (< 8 chars) — `queries.ts:202`.

### 2.5 `range` (numeric + lexicographic-string-date)

```json
{ "range": { "DATE_NAISSANCE": { "gte": "19620101", "lte": "19620931" } } }
```

```json
{ "range": { "AGE_DECES": { "gte": 60, "lte": 80 } } }
```

`gte`/`lte` only (no `gt`/`lt`, no date-math). Open-ended forms
(`{ "lte": "…" }` alone, `{ "gte": "…" }` alone) used too —
`queries.ts:169-186`.

### 2.6 `geo_distance` inside `bool.filter`

```json
{
  "bool": {
    "must":   { "match_all": {} },
    "filter": {
      "geo_distance": {
        "distance": "1km",
        "GEOPOINT_NAISSANCE": { "lat": 48.85, "lon": 2.35 }
      }
    }
  }
}
```

Distance units seen: `km`, `m`, `mi`, `yd`, `ft`, `NM` (regex in
`queries.ts:229`).

### 2.7 `match_all` as a filter context primitive

Standalone `{ "match_all": {} }` is the default when no input is
provided (`buildSimpleMatch`) and inside `bool.must` of the
geo-filter (see 2.6).

### 2.8 Top-level body fields

```json
{
  "min_score": 5,
  "track_total_hits": true,
  "_source": [ "UID", "NOM", "PRENOM", "DATE_DECES", "GEOPOINT_NAISSANCE", "…" ],
  "query":   { "bool": { "must": [ /* match */ ] } },
  "sort":    [ { "DATE_NAISSANCE_NORM": "asc" }, { "NOM.raw": "asc" } ],
  "size":    20,
  "from":    40,
  "aggs":    { /* see 2.10 */ }
}
```

`track_total_hits` can be `true` or `false`. `min_score` is set
when full-text. `from` + `size` paginate.

### 2.9 `scroll` lifecycle

`runRequest.ts:19-50` calls:

```ts
client.search({ index, body, scroll: "1m" })
client.scroll({ scroll_id, scroll: "1m" })
```

→ Surch must accept `?scroll=1m` on `POST /:index/_search`, return a
`_scroll_id` in the response, and accept `POST /_search/scroll`
with `{ "scroll": "1m", "scroll_id": "…" }`. Typical scroll size:
1k–5k docs per page, used by bulk-match jobs.

### 2.10 Aggregations: `terms`, `date_histogram`, `composite`,
`cardinality`

Single-aggregation paths emit:

```json
{
  "terms": { "field": "NOM.raw", "size": 100 }
}
```

```json
{
  "date_histogram": {
    "field":             "DATE_NAISSANCE_NORM",
    "calendar_interval": "month",
    "format":            "yyyyMMdd"
  }
}
```

Multi-aggregation path uses **composite**:

```json
{
  "composite": {
    "size":    1000,
    "sources": [
      { "lastName":   { "terms":          { "field": "NOM.raw" } } },
      { "birthDate":  { "date_histogram": { "field": "DATE_NAISSANCE_NORM",
                                            "calendar_interval": "month",
                                            "format": "yyyyMMdd" } } }
    ],
    "after": { "lastName": "MARTIN", "birthDate": "19620101" }
  }
}
```

With companion per-field `cardinality` aggs:

```json
{ "lastName_count": { "cardinality": { "field": "NOM.raw" } } }
```

`composite.after` is propagated from a prior response's
`after_key`.

### 2.11 `_msearch` (bulk-match)

`runRequest.ts:74` calls `client.msearch(bulkRequest)` — NDJSON
body, alternating header / body lines, each body is one of the
shapes in 2.1–2.10. Surch already exposes `_msearch`; the failure
is on the inner query DSL above.

### 2.12 Index mapping settings used today

Verbatim excerpt from `deces_index.yml` (analyzers + custom field
types):

```yaml
settings:
  analysis:
    analyzer:
      autocomplete_analyzer:
        tokenizer: edge_ngram_tokenizer
      norm:
        tokenizer: standard
        filter: [ lowercase, asciifolding ]
    normalizer:
      norm:
        type: custom
        filter: [ lowercase, asciifolding ]
    tokenizer:
      edge_ngram_tokenizer:
        type:      edge_ngram
        min_gram:  2
        max_gram:  20
        token_chars: [ letter, digit ]
mappings:
  properties:
    NOM:
      type:       text
      analyzer:   norm
      fields:
        raw: { type: keyword, normalizer: norm }
    PRENOMS:
      type:       text
      analyzer:   norm
      index_prefixes: { min_chars: 2, max_chars: 10 }
    DATE_NAISSANCE:
      type:       date
      format:     yyyyMMdd
    GEOPOINT_NAISSANCE:
      type:       geo_point
```

## 3. Expected response shapes

deces-backend reads:

- `hits.total.value` and `hits.total.relation` (ES 7.x shape, not
  the bare integer of ES 6.x).
- `hits.hits[]._source.*` — all `_source` fields requested in 2.8.
- `hits.hits[]._score` — used to rank and to apply `min_score`.
- `_scroll_id` — returned on every scrolled response, must be
  re-usable on the next `POST /_search/scroll`.
- `aggregations.<name>.buckets[]` for `terms` + `date_histogram`
  (each bucket = `{ key, doc_count }`, `date_histogram` also
  `key_as_string`).
- `aggregations.bucketResults.buckets[]` + `after_key` for
  `composite`.
- `aggregations.<name>_count.value` for `cardinality`.

No code reads `_shards`, `took`, `timed_out`, but the keys must
exist (JSON-shape parity). No code reads explanations / profile /
suggesters.

## 4. Acceptance criteria

For each primitive in §2:

1. **Parse parity** — Surch returns HTTP 200 (not 400) on a body
   containing the listed shape. Today most return 400 at parse.
2. **Result parity** — on a fixture corpus (suggest re-using the
   `tests/matchid_compat/` slice + a frozen 10k-row sample of
   INSEE deces 2020-m01 we can publish under
   `tests/matchid_compat/deces/`), the top-10 hit-IDs returned by
   Surch match the top-10 of ES 7.x for a curated query set of
   ~30 representative searches (advanced + block + fullText + UI +
   bulk-match). Ties allowed within the same `_score` bucket.
3. **Ranking budget** — NDCG@10 vs ES 7.x baseline ≥ **0.85** on
   the curated set. We will accept that fuzzy/BM25 differences
   shuffle the bottom of the page; the head must stay intact.
4. **Throughput budget** — single-node Surch sustains the artillery
   INSEE-25k profile at p95 ≤ **3× ES baseline** on the same VM
   shape (≥4 vCPU, 32 GB RAM). Bulk-match `msearch` throughput
   ≥ **0.5× ES baseline**.
5. **Scroll lifecycle** — `_scroll_id` survives at least 5 minutes
   under the 1-min `?scroll=1m` keepalive (we re-issue every
   ~50 s).
6. **Aggregation parity** — `composite.after_key` round-trips
   exactly; iterating page-by-page yields a partition of the
   keyspace.

## 5. Suggested implementation grouping (non-binding)

If splitting helps, the natural batches are:

- **Batch A — `bool` + `match`-with-options + `function_score`
  no-op wrapper** (unblocks ~80 % of the workload).
- **Batch B — `prefix`, `range`, `match_all`-in-filter** (date /
  age / id paths).
- **Batch C — `geo_distance` inside `bool.filter`** (geo-aware UI).
- **Batch D — `scroll` lifecycle** (bulk-match jobs).
- **Batch E — `terms` + `date_histogram` + `composite` +
  `cardinality` aggregations** (UI facets + analytics page).
- **Batch F — index-side: `norm`/`edge_ngram` analyzers,
  `keyword` `normalizer`, `geo_point` field type, `date{format}`,
  `index_prefixes`** (otherwise `PUT /:index` cannot be replayed
  from `deces_index.yml`).

We are happy to defer batches C and E if it accelerates A/B/D/F.

## 6. Out of scope (explicitly)

- **Snapshot / restore API.** matchID will ship the raw index
  directory as a tarball artifact (mirroring the current
  `make artifact-publish-dataprep-snapshot` flow) and accept a
  boot-time re-ingest from NDJSON if Surch volume layout changes
  between versions. **No `_snapshot` API needed from Surch.**
- **`script_score` / scripted updates / Painless.** The
  `function_score` wrapper we emit today carries no scoring
  function bodies; only parser support is requested.
- **`nested` / `join` / parent-child.** The `deces` mapping is
  flat.
- **Suggesters (`completion`, `phrase`), `_explain`, profiling,
  highlight blocks.** Code paths exist but are commented out
  (`buildRequest.ts:509-516`).
- **Real-time `_update` / partial updates.** Dataprep writes via
  `_bulk` only.
- **Security / ILM / cross-cluster / cross-version reindex.**
- **`function_score` scoring functions** (`gauss`, `script_score`,
  `field_value_factor`). Filed separately when the underlying
  hooks exist in Surch.

## 7. Pointers for the Surch side

- Fixture corpus we can publish under `tests/matchid_compat/deces/`:
  ~10k rows from INSEE `deces-2020-m01.txt.gz` + the curated
  30-query set used in §4.2. Tell us where to land it and we will
  open the artifact PR.
- Real production query samples (PII-redacted JSON) are available
  on request from
  `matchID-project/matchID@experiment/surch-replace-elastic` —
  `packages/deces-backend/src/score.spec.ts` and
  `server.spec.ts` already contain a usable seed.
- matchID side trade-off: we are willing to **swap to `keyword`-
  only fields with a Surch-side ASCII-folding analyzer** if it
  shortens delivery of batch F. We are **not** willing to drop
  fuzzy matching on names (§2.3) — it is the dominant ranker.
