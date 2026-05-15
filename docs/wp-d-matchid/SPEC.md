# Surch ⟶ matchID query evolution — consolidated spec

This is the rolling source of truth for what Surch must implement so
that matchID's `deces-backend` can route reads at Surch without code
changes. Maintained on branch `wp/d-matchid`.

Status: **empty until the first matchID intake file lands**
under `docs/wp-d-matchid/incoming/`.

## How to fill this file

Each entry below references one requirement file from `incoming/`,
mapped to its acceptance status. When matchID drops a new requirement,
copy the agreed wire shape + acceptance criteria here and pin the
decision file path.

## Index of requirements

_Empty._ The first requirement will appear here once matchID writes
`docs/wp-d-matchid/incoming/<YYYY-MM-DD>-<slug>.md` and we record the
agreed scope.

## Out-of-scope reminders

Top-level non-goals that Surch does **not** implement regardless of
matchID's request, unless the cross-team scope is renegotiated:

- Vector / dense-retrieval queries (`knn`, `dense_vector`).
- Aggregations beyond what `deces-backend` already exercises in
  production (we accept the historical surface, not every facet ES
  ships).
- Cluster / shard routing semantics — Surch is single-node for the
  matchID milestone.
- Custom analyzers beyond the ones already declared in `surch-analysis`
  (`SimpleAnalyzer`, `StandardAnalyzer`, `KeywordAnalyzer`,
  `StopAnalyzer`, `WhitespaceAnalyzer`). New analyzers require their
  own intake batch.
