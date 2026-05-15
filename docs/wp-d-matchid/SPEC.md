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

These two documents are the matchID-side outputs of WP-D. They are
not new requirements — they translate the gaps above into shapes
matchID integrators can act on.

- `dsl-translation-matrix.md` — gap-by-gap mapping (A1..A15, B1, B2)
  of ES wire shape ↔ Surch equivalent ↔ status ↔ test gate file.
  Cross-referenced by every implementation PR landing on `wp/a-optim`
  and `wp/b-test-auto`.
- `swap-guide.md` — operational playbook for the matchID team:
  pre-requisites, three swap strategies (env-var flip, shadow mode,
  incremental by workload), rollback paths, and minimum
  observability via `GET /_prometheus_metrics`.

## Active gaps

Numbered ids — stable, used in commit subjects and PR titles.

| Gap | Title | Owning WP |
|---|---|---|
| **A1** | `match` object form with `fuzziness: AUTO\|0\|N` | wp/a-optim |
| **A2** | `geo_point` field type + `geo_distance` query | wp/a-optim |
| **A3** | `bool.filter` / `bool.should` / `minimum_should_match` / clause `boost` | wp/a-optim |
| **A4** | Stateful scroll API: `?scroll=1m` + `POST /_search/scroll` | wp/a-optim |
| **A5** | `function_score` wrapper (`field_value_factor`, decay, `score_mode`, `boost_mode`) | wp/a-optim |
| **A6** | `prefix` query + `index_prefixes` mapping option | wp/a-optim |
| **A7** | `range` query against `date` fields with `format: yyyyMMdd` | wp/a-optim |
| **A8** | `match_all` query (default + filter-context) | wp/a-optim |
| **A9** | `from` + `size` pagination beyond `size`-only | wp/a-optim |
| **A10** | `sort` over keyword / normalised-date sub-fields | wp/a-optim |
| **A11** | `min_score` top-level body filter | wp/a-optim |
| **A12** | Aggregations: `terms`, `date_histogram`, `composite` + `after_key`, `cardinality` | wp/a-optim |
| **A13** | Mapping primitives: `edge_ngram` tokenizer + analyzer, `normalizer` (lowercase + asciifolding), `index_prefixes`, `geo_point`, `date` with `format` | wp/a-optim |
| **A14** | Response shape parity: `hits.total.{value,relation}` (ES 7.x), `_scroll_id`, `aggregations.*` buckets | wp/a-optim |
| **A15** | `_msearch` inner-query DSL parity (NDJSON already exposed) | wp/a-optim |
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
