# deces_v1 Elasticsearch Oracle Gate

This runbook turns the committed `deces_v1` replay into an explicit
Elasticsearch 8.6.1 gate. It is intentionally human-readable:
the run produces `summary.md` and exits non-zero on any mismatch, so the
user does not need to inspect raw JSON.

## Inputs

- Replay: `tests/matchid_compat/replays/deces_v1.json`
- Mapping: `tests/matchid_compat/deces/mapping.json`
- Bulk slice: `tests/matchid_compat/deces/slice-1000.ndjson`
- Index: `deces`

## Required Comparison

For every non-skipped replay request, compare:

- HTTP `status`
- `hits.total.value`
- `hits.hits[0]._id`
- critical shape:
  - normal search responses must contain `hits.total.value`
  - normal search responses with an expected top hit must contain
    `hits.hits[0]._id`
  - `_msearch` responses must contain `responses[]`; compare the sum of
    each sub-response `hits.total.value` and the first sub-response top id
  - scroll-opening responses with `?scroll=` must contain a non-empty
    `_scroll_id`

Volatile fields such as `took`, shard counts, and scores are not part of
this first gate. The gate is actionably scoped to status, totals, top-hit
identity, and response shape.

## External Gate

Prerequisites:

- A clean Elasticsearch 8.6.1 node is running.
- `ELASTICSEARCH_URL` points at that node, for example
  `http://127.0.0.1:9200`.
- Python 3 is available.

Validate local inputs without sending HTTP requests:

```sh
python3 scripts/matchid/deces_v1_elasticsearch_oracle.py --dry-run
```

Run the external gate from the repository root:

```sh
ELASTICSEARCH_URL="${ELASTICSEARCH_URL:-http://127.0.0.1:9200}" \
  python3 scripts/matchid/deces_v1_elasticsearch_oracle.py
```

Expected human artifact:

```text
target/matchid-oracle/deces_v1/summary.md
```

The committed replay remains the source of request bodies. The generated
`summary.md` is the review surface: it shows one row per request and a
PASS/FAIL verdict, including the exact mismatch when status,
`hits.total.value`, top id, or critical shape diverges.

## Replay Request Coverage

- `adv_nom_prenoms_date`
- `adv_nom_commune_sexe`
- `adv_nom_prenoms_commune`
- `adv_fuzzy_nom_with_match`
- `adv_fuzzy_auto_prenoms_sexe`
- `adv_bool_should_msm`
- `adv_match_filter_commune`
- `adv_bool_must_not_sexe`
- `block_msearch_a`
- `block_msearch_b`
- `block_msearch_c`
- `block_msearch_d`
- `block_msearch_e`
- `ft_multi_match_nom_prenoms`
- `ft_match_simple_nom`
- `ft_multi_match_from_size`
- `ft_match_min_score`
- `sort_date_naissance_asc`
- `sort_nom_desc`
- `sort_match_date_deces_desc`
- `fs_match_wrap`
- `fs_bool_wrap`
- `prefix_nom`
- `prefix_prenoms`
- `prefix_date_naissance_short`
- `scroll_full`
- `scroll_filtered_size_500`
- `scroll_match_all_size_1000`
- `range_date_naissance_window`
- `range_date_naissance_lte_open`
