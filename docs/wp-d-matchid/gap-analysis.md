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

| Requirement file | Workload | Status | Surch artifacts | Notes |
|---|---|---|---|---|

_No requirements yet — first matchID intake pending._

## How to read this table

- `Requirement file` = path under `docs/wp-d-matchid/incoming/`
- `Workload` = which matchID code path triggers it (1-2 words)
- `Surch artifacts` = source files, test fixtures and bench reports
  that close the gap; bullet-list when several
- `Notes` = link to the decision file under
  `docs/wp-d-matchid/decisions/` and any scope adjustment we agreed on
