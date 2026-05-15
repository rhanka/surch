# Surch ⟶ matchID query evolution — consolidated spec

Rolling source of truth for what Surch must implement so that matchID's
`deces-backend` can route reads at Surch without code changes.
Maintained on branch `wp/d-matchid`.

Status: **draft, derived from public deces-backend code**, awaiting
confirmation by the matchID team.

## Index of requirements

Each entry references an `incoming/` file and the decision file that
records scope and acceptance.

### Batch — 2026-05-15-surch-derived-preliminary

- Intake: `incoming/2026-05-15-surch-derived-preliminary.md`
- Decision: `decisions/2026-05-15-surch-derived-preliminary.md`
- Scope: HTTP surface and ES query shapes used by `deces-backend` for
  interactive lookup (artillery v1), bulk CSV (`/search/csv` →
  scroll), and direct id lookup.

Active gaps coming out of this batch:

| Gap id | Title | Owning WP |
|---|---|---|
| **A1** | `match` object form with `fuzziness: AUTO\|0\|N` | wp/a-optim |
| **A2** | `geo_point` field type + `geo_distance` query | wp/a-optim |
| **A3** | `bool.filter`, `bool.should`, `minimum_should_match` | wp/a-optim |
| **A4** | Stateful scroll API: `?scroll=1m`, `POST /_search/scroll` | wp/a-optim |
| **B1** | matchID replay fixture under `tests/matchid_compat/` | wp/b-test-auto |

## Out-of-scope reminders

- Vector / dense-retrieval queries (`knn`, `dense_vector`).
- Aggregations beyond what `deces-backend` exercises in production
  (none today — facets are computed in Node, not in ES).
- Cluster / shard routing semantics — Surch is single-node for the
  matchID milestone.
- Custom analyzers beyond the ones already declared in `surch-analysis`
  (`SimpleAnalyzer`, `StandardAnalyzer`, `KeywordAnalyzer`,
  `StopAnalyzer`, `WhitespaceAnalyzer`). New analyzers require their
  own intake batch.
- `function_score` / `script_score` — `deces-backend` does its own
  composite scoring in Node on top of ES `_score`.
