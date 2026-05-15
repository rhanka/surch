# Surch ⟶ matchID query evolution — consolidated spec

Rolling source of truth for what Surch must implement so that matchID's
`deces-backend` can route reads at Surch without code changes.
Maintained on branch `wp/d-matchid`.

Status: **draft, derived from public deces-backend code**, awaiting
confirmation by the matchID team.

## Index of requirements

Each entry references an `incoming/` file and the decision file that
records scope and acceptance.

### Batch — 2026-05-15-deces-backend-dsl-inventory (authoritative)

- Intake: `incoming/2026-05-15-deces-backend-dsl-inventory.md`
- Scope: every OpenSearch DSL primitive emitted by `deces-backend`
  against ES 7.x today (wire shapes copy-pasted verbatim from
  `matchID/packages/deces-backend/src/{queries,buildRequest,runRequest}.ts`
  and from `deces_index.yml`).
- Workloads covered: advanced UI search, block-match (`msearch`),
  full-text, bulk-match (`scroll`), autocomplete (`prefix`),
  aggregations (`composite` + `cardinality` + `date_histogram`).

### Batch — 2026-05-15-surch-derived-preliminary (HTTP-surface view)

- Intake: `incoming/2026-05-15-surch-derived-preliminary.md`
- Decision: `decisions/2026-05-15-surch-derived-preliminary.md`
- Complementary higher-level view: HTTP surface of
  `/deces/api/v1/search`, `/search/csv`, `/id/{id}`, and the artillery
  scenario acceptance budget (p95 < 200 ms, max < 500 ms at 50 RPS).

### WP-D deliverables (matchID-facing artefacts)

- `gap-analysis.md` — single source of truth for matchID compat
  status. Tracks every gap (A1..A15, B1, B2) with
  `gap | partial | implemented | declined` + the Surch artifacts
  that close it. Retired (or archived as "compat achieved on
  YYYY-MM-DD") once every row is `implemented`.
- `swap-guide.md` — operational playbook for the matchID team:
  pre-requisites, the env-var-flip cutover (only valid once
  `gap-analysis.md` is 100% green), rollback path, and minimum
  observability via `GET /_prometheus_metrics`. Shadow mode and
  incremental-by-workload strategies are explicitly **out of scope**
  until full compat — matchID does not bascule until Surch is a
  drop-in replacement.

Note: an earlier `dsl-translation-matrix.md` was retired because the
Surch wire shape is identical to ES 7.x by design; the matrix was a
copy of `gap-analysis.md` columns under a misleading name.

## Active gaps

Numbered ids — stable, used in commit subjects and PR titles.

| Gap | Title | Owning WP |
|---|---|---|
| **A1** | `match` object form with `fuzziness: AUTO\|0\|N` | wp/d-matchid |
| **A2** | `geo_point` field type + `geo_distance` query | wp/d-matchid |
| **A3** | `bool.filter` / `bool.should` / `minimum_should_match` / clause `boost` | wp/d-matchid |
| **A4** | Stateful scroll API: `?scroll=1m` + `POST /_search/scroll` | wp/d-matchid |
| **A5** | `function_score` wrapper (`field_value_factor`, decay, `score_mode`, `boost_mode`) | wp/d-matchid |
| **A6** | `prefix` query + `index_prefixes` mapping option | wp/d-matchid |
| **A7** | `range` query against `date` fields with `format: yyyyMMdd` | wp/d-matchid |
| **A8** | `match_all` query (default + filter-context) | wp/d-matchid |
| **A9** | `from` + `size` pagination beyond `size`-only | wp/d-matchid |
| **A10** | `sort` over keyword / normalised-date sub-fields | wp/d-matchid |
| **A11** | `min_score` top-level body filter | wp/d-matchid |
| **A12** | Aggregations: `terms`, `date_histogram`, `composite` + `after_key`, `cardinality` | wp/d-matchid |
| **A13** | Mapping primitives: `edge_ngram` tokenizer + analyzer, `normalizer` (lowercase + asciifolding), `index_prefixes`, `geo_point`, `date` with `format` | wp/d-matchid |
| **A14** | Response shape parity: `hits.total.{value,relation}` (ES 7.x), `_scroll_id`, `aggregations.*` buckets | wp/d-matchid |
| **A15** | `_msearch` inner-query DSL parity (NDJSON already exposed) | wp/d-matchid |
| **B1** | matchID replay fixture under `tests/matchid_compat/` | wp/b-test-auto |
| **B2** | INSEE 10k slice frozen fixture under `tests/matchid_compat/deces/` | wp/b-test-auto |

## Implementation order (proposal)

By increasing risk / cost, biased toward unblocking the artillery
replay first :

1. **A3** — `bool.filter` / `should` / `minimum_should_match` / `boost`
2. **A1** — `match` fuzziness sub-field
3. **A8** — `match_all`
4. **A11** — `min_score`
5. **A9 + A10** — `from`/`size`/`sort`
6. **A14** — ES-7.x `hits.total.value/relation` shape
7. **B1** — first replay fixture (30 representative searches)
8. **A6** — `prefix` query (incl. `index_prefixes` mapping)
9. **A7** — `range` on date with `format: yyyyMMdd`
10. **A5** — `function_score` wrapper
11. **A4** — scroll API
12. **A12** — aggregations
13. **A2 + A13** — geo + custom analyzers + edge_ngram + normalizer
14. **B2** — INSEE 10k slice frozen fixture

## Out-of-scope reminders

- Vector / dense-retrieval queries (`knn`, `dense_vector`).
- Cluster / shard routing semantics — Surch is single-node for the
  matchID milestone.
- Custom analyzers beyond what `deces_index.yml` requires (no
  language-specific stemmers, no synonyms, no shingles).
- `script_score` / inline scripting — `function_score`'s declarative
  shape (A5) is enough for matchID.
- Snapshot API — matchID ships raw-index tarball + boot-time
  re-ingest. Surch will expose its own raw-index export instead
  (tracked under wp/c-ops).
- nested / join, suggesters, partial updates.
