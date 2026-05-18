# MatchID Compatibility Replay Contract

This directory is the commit-safe home for sanitized MatchID Elasticsearch replay
fixtures. It must never contain production secrets, customer data, raw identifiers, raw traffic
logs, unredacted payloads, reversible hashes, or private redaction mapping tables.

Raw exports must stay outside the repository. Local staging folders such as `raw/`, `incoming/`,
`private/`, and `unredacted/` are ignored here only as a last-resort guardrail; do not rely on
`.gitignore` as the redaction process.

## Directory Contract

Create fixture folders only when a sanitized export is ready to commit:

```text
tests/matchid_compat/
  README.md
  export_manifest.template.json
  datasets/
    <export_id>.json
    <export_id>.ndjson
  replays/
    <export_id>_critical.json
  redaction/
    <export_id>.md
```

`<export_id>` must be stable and non-sensitive, for example
`matchid_sanitized_2026_05_12_v1`.

## Required Artifacts

Every MatchID export must provide:

- Export manifest: copy `export_manifest.template.json` and fill capture metadata, committed file
  paths, endpoint coverage, redaction assertions, normalization rules, and UAT status.
- Dataset manifest: an `opensearch-oracle` dataset JSON that can load the fixture from an empty
  node. It must create the index with MatchID-equivalent mappings/settings/analyzers, bulk the
  sanitized NDJSON, and refresh.
- Index definition: the exact index creation body used for replay, including mappings, analyzers,
  normalizers, tokenizers, dynamic templates, aliases, routing, and settings that can affect
  search behavior.
- Bulk NDJSON: sanitized `_bulk` payload with alternating action/source lines, stable synthetic
  `_id` values, and `_routing` or version/concurrency metadata only when MatchID uses it.
- Replay manifest: an `opensearch-oracle` replay JSON with `name`, `dataset`, optional
  `comparison`, and ordered `requests`. Each request must include `name`, `method`, `path`,
  optional `body`, `expected_status`, and a full normalized `expected_response` when the response
  body matters.
- Expected responses: captured from the Elasticsearch reference, then sanitized and
  normalized. Do not generate oracle responses from Surch.
- Redaction note: a short human-readable note describing the redaction method, reviewer, known
  lossy transformations, and the private location of any non-committed evidence. Do not commit the
  value mapping table.

Parser-smoke fixtures may use partial responses elsewhere in the repo. MatchID gate fixtures must
prefer full normalized responses because the current JSON comparator rejects unexpected paths after
normalization.

## deces_v1 Elasticsearch Oracle Gate

The committed `deces_v1` replay currently executes all 30 requests against
Surch HEAD via:

```sh
cargo test -p surch-api matchid_replay_deces_v1_executes_all_non_skipped_requests --test matchid_compat
```

The external Elasticsearch 7.x oracle gate is documented in
`tests/matchid_compat/oracle/deces_v1.md`. Run it against a clean reference
node with `ELASTICSEARCH_URL` set; it loads
`tests/matchid_compat/deces/mapping.json`, bulks
`tests/matchid_compat/deces/slice-1000.ndjson`, replays
`tests/matchid_compat/replays/deces_v1.json`, and writes the human review
artifact:

```text
target/matchid-oracle/deces_v1/summary.md
```

That `summary.md` is the review surface. It has one row per replay request
and fails the run on any mismatch in HTTP status, `hits.total.value`,
`hits.hits[0]._id`, or critical response shape. Do not ask a user to inspect
raw JSON for this gate.

## Endpoints To Capture

At minimum, capture the endpoints MatchID actually uses:

- Index setup: `PUT /<index>`, `GET /<index>/_mapping`, `GET /<index>/_settings`, aliases or
  templates if they are part of serving.
- Load path: `POST /_bulk` and `POST /<index>/_refresh`.
- Read path: `GET|POST /<index>/_search`, `GET|POST /<index>/_count`,
  `GET|POST /<index>/_mget`, and `GET|POST /<index>/_msearch`.
- Any observed ancillary endpoint: `_analyze`, `_search/template`, PIT, scroll, aliases,
  `_terms_enum`, or compatible error paths if MatchID calls them.

Preserve query string parameters in `path` exactly, including `routing`, `preference`, `typed_keys`,
`track_total_hits`, pagination, timeout, and search type parameters. Authentication headers and
tokens are never part of committed fixtures.

Each captured request needs a workflow label in the manifest, for example `candidate_lookup`,
`dedupe_review_list`, `bulk_profile_mget`, or `dashboard_count`. If a known traffic class is absent
from the export, record it as a blocker or an explicit out-of-scope item in the export manifest.

## Redaction Rules

Replace every sensitive value with deterministic synthetic data:

- Names, emails, phone numbers, postal addresses, national identifiers, organization identifiers,
  account IDs, tokens, API keys, cookies, free-text comments, raw document IDs, and tenant names.
- Any value that can identify a person, customer, organization, dataset source, infrastructure
  host, internal project, or commercial relationship.

Preserve the invariants that drive compatibility:

- Field names, field types, mappings, analyzers, normalizers, boosts, copy fields, dynamic fields,
  nested/object structure, and multi-fields.
- Tokenization shape: word boundaries, casing patterns, punctuation, prefixes/suffixes, language
  class, accent/ASCII behavior, numeric length, date granularity, and fuzzy-edit distance.
- Cardinality, nullability, missing-vs-null behavior, arrays, duplicate terms, nested counts,
  object depth, routing distribution, and shard-affecting metadata.
- Sort/filter distributions, tie cases, pagination boundaries, top-hit order, `_source` filtering,
  stored fields, docvalue fields, highlights, aggregations, and error envelopes when used.
- Query operators and request options: `bool`, `match`, `multi_match`, `term`, `terms`, `range`,
  `fuzzy`, `minimum_should_match`, boosts, fuzziness, analyzers, slop, sort, collapse, rescore, and
  timeout options.

Use stable opaque synthetic IDs such as `mid_doc_000001`. If the same original value appears in
documents, queries, and expected hits, replace it with the same synthetic value everywhere. Keep the
private mapping outside git. Do not use unsalted hashes, reversible hashes, base64 encodings, or
encrypted production values as committed fixture data.

Never normalize away a field that participates in matching, filtering, sorting, authorization,
deduplication, or workflow branching.

## Replay Format

Dataset manifests should follow the existing oracle shape:

```json
{
  "name": "matchid_sanitized_YYYY_MM_DD_v1",
  "description": "Sanitized MatchID replay dataset",
  "operations": [
    {
      "kind": "create_index",
      "path": "/matchid_sanitized",
      "body": "matchid_sanitized_YYYY_MM_DD_v1.create_index.json",
      "expected_status": 200
    },
    {
      "kind": "bulk",
      "path": "/_bulk",
      "body": "matchid_sanitized_YYYY_MM_DD_v1.ndjson",
      "expected_status": 200
    },
    {
      "kind": "refresh",
      "path": "/matchid_sanitized/_refresh",
      "expected_status": 200
    }
  ]
}
```

Replay manifests should follow the existing oracle request shape:

```json
{
  "name": "matchid_sanitized_critical",
  "dataset": "matchid_sanitized_YYYY_MM_DD_v1",
  "comparison": {
    "ignored_paths": ["took", "_shards.total"],
    "score_tolerance": 0.001
  },
  "requests": [
    {
      "name": "candidate_lookup_exact_name",
      "method": "POST",
      "path": "/matchid_sanitized/_search?track_total_hits=true",
      "body": {
        "query": {
          "match": {
            "candidate_name": {
              "query": "Synthetic Person 0001",
              "operator": "AND"
            }
          }
        },
        "_source": ["candidate_id", "candidate_name"]
      },
      "expected_status": 200,
      "expected_response": {
        "timed_out": false,
        "_shards": {
          "successful": 1,
          "skipped": 0,
          "failed": 0
        },
        "hits": {
          "total": {
            "value": 1,
            "relation": "eq"
          },
          "max_score": null,
          "hits": [
            {
              "_index": "matchid_sanitized",
              "_id": "mid_doc_000001",
              "_source": {
                "candidate_id": "mid_doc_000001",
                "candidate_name": "Synthetic Person 0001"
              }
            }
          ]
        }
      }
    }
  ]
}
```

If the current oracle runner cannot load a required body file or endpoint shape, extend the oracle
crate first; do not silently simplify MatchID traffic to fit the runner.

## Volatile Fields To Normalize

Keep normalization minimal and request-specific. Common candidates:

- `took`
- `_shards.total` when shard count differs between reference and Surch
- `hits.max_score` and `hits.hits.*._score` when exact scoring is not the gate
- `_seq_no`, `_primary_term`, `_version` for indexing acknowledgements
- `_scroll_id`, `pit_id`, `profile`, `_clusters`, node IDs, task IDs, generated timestamps, and
  similar engine/runtime metadata when present

Do not ignore these fields for MatchID gate requests:

- HTTP status and Elasticsearch-style error envelope (`error.type`,
  `error.reason`, `status`)
- `hits.total.value`, `hits.total.relation`, hit `_id`, hit order, `_index`, `_source`, `fields`,
  `sort`, `highlight`, aggregation buckets, and `_ignored`
- Query-dependent timestamps, date ranges, routing values, or tenant filters after they have been
  replaced with fixed synthetic values

Use `score_tolerance` only for numeric score drift on requests where score comparison is useful.
For requests gated only on top-hit identity/order, ignore score fields explicitly and document why.

## Go Shadow UAT Criteria

Surch can enter MatchID shadow UAT only when all of the following are true:

- The committed sanitized export contains mappings/settings, bulk data, and replay manifests for
  every critical MatchID read workflow and every endpoint observed in the capture window.
- Redaction review is complete, `approved_for_commit` is true in the export manifest, and no secret,
  PII, raw identifier, raw host, token, or reversible value appears in committed files.
- The dataset loads into a fresh Elasticsearch reference node and all replay requests
  pass against the reference oracle.
- The same dataset loads into Surch and all P0/P1 replay requests pass on status, totals, top-hit
  IDs, source fields, sort order, and compatible error envelopes.
- Every mismatch is either fixed or recorded as an accepted delta with owner, risk, expiry, and
  production impact. No unclassified mismatch remains.
- Unsupported MatchID-used query types or endpoints return explicit compatible errors; silent empty
  results or silent option drops are blockers.
- A symmetric HTTP benchmark report exists for the same fixture or a larger approved MatchID-sized
  fixture, with agreed p50/p95/p99/error budgets and at least five repeated runs when making
  performance claims.
- Shadow mode is read-only, does not serve user traffic, logs enough evidence to compare Surch vs
  Elasticsearch, and has a one-switch rollback to Elasticsearch-only reads.

## Production Blockers

Shadow UAT success is not production approval. Blocking items for production traffic include:

- Any unredacted or insufficiently reviewed fixture evidence.
- Missing MatchID mappings/settings/analyzers, missing critical traffic classes, or stale fixtures
  that no longer represent production query mix.
- Any P0/P1 replay mismatch on status, total hits, top-hit identity/order, returned fields, sort
  order, fuzzy Damerau-Levenshtein behavior up to distance 2, or error envelopes.
- Any MatchID-used API/query option that Surch ignores, downgrades, or treats as a successful empty
  result instead of implementing or returning a compatible explicit error.
- Performance outside the agreed latency/error budget on production-like data, or benchmark reports
  without oracle validation before timing.
- No durable index recovery or documented rebuild-from-source path, no crash/restart evidence, no
  backup/rebuild runbook, no observability plan, or no rehearsed rollback path.
- Any required write/update/delete, refresh, alias, routing, or consistency semantic that MatchID
  depends on but Surch has not proven.
