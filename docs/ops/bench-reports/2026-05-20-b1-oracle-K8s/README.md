# B1 phase 3 — first oracle cross-check (Surch vs Elasticsearch 7.17)

First end-to-end run of `b1-oracle-gate` on the Scaleway burst pool
that drove the `deces_v1.json` replay against both Surch and a
single-node Elasticsearch 7.17.18 against the same INSEE 10k slice.
The gate writes `b1-oracle.json` (schema `surch.bench.b1_oracle.v1`)
and exits non-zero when at least one **unexpected** divergence
remains (i.e. not in `KNOWN_PARTIAL_NAMES`).

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26133556087>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=b1-oracle-gate`)
- Job manifest: `deploy/k8s/jobs/b1-oracle-gate.yaml` (rendered +
  archived as `job.yaml`)
- Surch image: `ghcr.io/rhanka/surch:sha-a1b9d1e…`
- Bench driver image: `ghcr.io/rhanka/surch:bench-sha-a1b9d1e…`
- Elasticsearch image: `docker.elastic.co/elasticsearch/elasticsearch:7.17.18`
- Slice: INSEE Open Licence 2.0 `Deces_2024.csv` first 10 000 rows
  (same fixture as `2026-05-19-insee-10k-k8s/`).

## Verdict

- **Total replay requests**: 30
- **Skipped via `KNOWN_PARTIAL_NAMES`**: 1 (`sort_nom_desc` — Surch's
  A10 text-sort alias is wider than ES's strict keyword sub-field
  requirement, expected divergence)
- **Unexpected divergence count**: 4 (across **2** replay entries)
- Gate result: **FAIL** (exit 1) — divergences listed below.

Captured by `driver.log`:

```text
b1_oracle: 4 unexpected divergence(s) over 30 request(s); see /reports/b1-oracle.json
  - prefix_nom     [hits.total.value]      surch=26   es=0
  - prefix_nom     [hits.hits[0]._id]      surch="ins_0000222" es=null
  - prefix_prenoms [hits.total.value]      surch=1569 es=0
  - prefix_prenoms [hits.hits[0]._id]      surch="ins_0000004" es=null
```

The 26 remaining entries (advanced search, block-match, full-text,
range, function_score, prefix on `DATE_NAISSANCE`, scroll initiators,
2 sorts on keyword fields) all matched on the four diff axes
(`hits.total.value`, `hits.total.relation`, `hits.hits[0]._id`,
`aggregations.<name>`).

## What the divergence tells us

`prefix_nom` and `prefix_prenoms` request a `prefix` query against
`NOM` / `PRENOMS`, both declared `text` with
`index_prefixes: { min_chars: 2, max_chars: 5 }` in
`tests/matchid_compat/deces/mapping.json`.

- Surch serves the prefix from the write-time prefix postings side
  table (A6 phase 2 — `crates/surch-index/src/document_index.rs::index_prefix_terms`),
  matching all 26 / 1 569 records.
- Elasticsearch 7.17 returns `total.value = 0` and an empty `hits.hits[]`.

This is a real behavioural gap, **not** a known partial. ES does
honour `index_prefixes` for `match_phrase_prefix` and the query
optimiser, but a bare `prefix` query against a `text` field on ES is
analyser-driven: the lowercased / standard-analysed query token
likely does not match the case-preserving INSEE corpus tokens.

Two concrete follow-ups (separate lots):

1. Update `tests/matchid_compat/deces/mapping.json` (and the B2 v1
   fixture) so `NOM` / `PRENOMS` declare an explicit
   `analyzer: standard` or a `normalizer`-equipped multi-field that
   ES + Surch both consume — closes the gap structurally, not just
   in test fixtures.
2. Add `prefix_nom` and `prefix_prenoms` to
   `KNOWN_PARTIAL_NAMES` (with a comment pointing at this report) so
   the gate goes green again while (1) is in flight. Probably
   undesirable — the divergence is real and we want to track it.

## What this report does NOT contain

- The full `b1-oracle.json` schema body: the first run's manifest
  was using `set -e` and exited before `cat /reports/b1-oracle.json`
  could be flushed. The follow-up commit
  (`fix(k8s): always cat b1-oracle.json even on divergence exit`)
  fixes that. The next gate run will ship the full envelope.
