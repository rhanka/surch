# WP-D — matchID query evolutions (intake)

This directory is the **single intake point** for the matchID team to
declare what Surch must implement to host the `deces-backend` workload
without modification. Branch: `wp/d-matchid`. Worktree:
`.worktrees/wp-d`.

## Where matchID writes the spec

1. **First-class intake — `docs/wp-d-matchid/incoming/`**
   One Markdown file per requirement batch, named with an ISO date and a
   short topic slug:
   ```
   docs/wp-d-matchid/incoming/2026-05-16-fuzzy-by-field.md
   docs/wp-d-matchid/incoming/2026-05-20-bool-with-rescore.md
   ```
   matchID can open a pull request against branch `wp/d-matchid` adding
   one or several files under this directory. We do not edit those
   files after they land; they remain the verbatim ask.

2. **Canonical consolidated spec — `docs/wp-d-matchid/SPEC.md`**
   We (Surch side) maintain this file as the rolling source of truth.
   It cites the `incoming/` files and records the agreed scope, the
   wire shape (REST endpoint, query DSL fragments), the acceptance
   criteria, and the test fixtures expected to gate each requirement.

3. **Decisions — `docs/wp-d-matchid/decisions/`**
   One Markdown file per requirement decision, also dated, recording:
   accepted / deferred / rejected, scope clarifications negotiated
   with matchID, rationale, expected effort, target Surch version,
   linked PR(s).

4. **Gap analysis — `docs/wp-d-matchid/gap-analysis.md`**
   Living table that maps each requirement to its Surch-side
   implementation status. The CI checks pulled from `wp/b-test-auto`
   (artillery on INSEE 25k, NDCG@10 SciFact + TREC-COVID) gate the
   "implemented" claim.

## How to write a requirement

A useful intake file in `incoming/` answers, in order:

1. **Workload context** — which matchID code path triggers this query,
   what real users are doing, what error they hit today against Surch.
2. **Elasticsearch 8.6.1 wire shape** — the actual JSON body and URL
   parameters matchID sends, copy-pasted from production logs after PII
   redaction.
3. **Expected response** — the JSON Surch must return so matchID code
   keeps working without change. Include the relevant `_source`
   fields, `_score` semantics, and `hits.total` shape.
4. **Acceptance criteria** — concrete checks (status code, hit IDs,
   score ordering, performance budget). Reference the BEIR / INSEE
   workloads when possible.
5. **Out of scope** — what this batch explicitly does *not* require,
   to keep us from over-engineering.

## Branch + merge policy

- All implementation work lands on `wp/d-matchid`, then merges to
  `main` like the other WPs (see `docs/ops/workpackages.md`).
- Each implementation commit references the `incoming/` file id (the
  ISO date + slug) in its subject so traceability stays trivial.
- Decisions and gap-analysis updates live alongside the implementation
  commit (or in their own commit when the analysis happens before the
  code).
- matchID never has to touch Surch code; their PR is limited to one or
  more files under `incoming/`. The Surch maintainers handle SPEC.md,
  decisions/, gap-analysis.md, and the implementation.

## Quick links

- Workpackages index: `docs/ops/workpackages.md`
- Surch test automation plan: `docs/ops/test-automation-plan.md`
- Surch packaging plan: `docs/ops/packaging-plan.md`
- matchID compatibility contract (existing): `tests/matchid_compat/README.md`
- matchID readiness assessment: `docs/roadmap/matchid-replacement-readiness.md`
