# SPEC - OpenSearch Index And Document APIs

## Purpose

Lock the OpenSearch-compatible HTTP contract that later Surch MVP implementation branches must consume for index management and document APIs.

This spec is intentionally narrower than full OpenSearch. It defines the MVP-compatible subset, the exact route and response shapes to preserve, and the places where Surch may explicitly defer behavior.

## Consumer Branches

- BR-03 storage contract
- BR-04 indexer mappings and bulk contract
- BR-06 API index and document compatibility

## Source References

- Create Index API: `https://docs.opensearch.org/latest/api-reference/index-apis/create-index/`
- Get Index API: `https://docs.opensearch.org/latest/api-reference/index-apis/get-index/`
- Get Mapping API: `https://docs.opensearch.org/latest/api-reference/index-apis/get-mappings/`
- Index Document API: `https://docs.opensearch.org/latest/api-reference/document-apis/index-document/`
- Get Document API: `https://docs.opensearch.org/latest/api-reference/document-apis/get-documents/`
- Delete Document API: `https://docs.opensearch.org/latest/api-reference/document-apis/delete-document/`
- Bulk API: `https://docs.opensearch.org/latest/api-reference/document-apis/bulk/`
- Refresh Index API: `https://docs.opensearch.org/latest/api-reference/index-apis/refresh/`
- Flush API: `https://docs.opensearch.org/latest/api-reference/index-apis/flush/`

## MVP Normalization Decisions

- Surch uses a single fixed document type token: `_doc`.
- Legacy typed routes are out of scope.
- Full OpenSearch surface is not required. Anything not marked `MUST` or `SHOULD` below may be rejected with a documented `400` rather than silently accepted.
- For create-index payloads, MVP support is limited to `settings.index.number_of_shards`, `settings.index.number_of_replicas`, and `mappings.properties` plus analyzer-related settings required by BR-04. Unknown top-level shapes should be rejected, not ignored.
- For bulk, MVP supports `index`, `create`, and `delete` actions. `update` is `LATER`.
- For document APIs, optimistic concurrency via `if_seq_no` and `if_primary_term` is part of MVP. External versioning via `version` and `version_type` is not.
- `pipeline` and `require_alias` are not MVP requirements.
- `Flush` is explicitly non-blocking for MVP release and may be deferred if BR-03 cannot support it cleanly.

## Index Naming Contract

Create-index requests must reject names that violate the documented OpenSearch restrictions:

- letters must be lowercase
- names must not start with `_` or `-`
- names must not contain spaces or commas
- names must not contain `:`, `"`, `*`, `+`, `/`, `\\`, `|`, `?`, `#`, `>`, or `<`

## Endpoint Compatibility Matrix

| Endpoint | Methods | Success Status | MVP | Request Contract | Required Response Fields | Primary Failure Modes |
|---|---|---|---|---|---|---|
| `/{index}` | `PUT` | `200` | MUST | Optional body with `settings`, `mappings`, `aliases` | `acknowledged`, `shards_acknowledged`, `index` | invalid index name, duplicate index, invalid mapping/settings |
| `/{index}` | `GET` | `200` | SHOULD | No body | object keyed by index name with `aliases`, `mappings`, `settings.index.*` | missing index |
| `/{index}` | `DELETE` | `200` | MUST | No body | `acknowledged` | missing index |
| `/{index}/_mapping` | `GET` | `200` | MUST | No body | object keyed by index name with `mappings.properties` | missing index |
| `/_mapping` | `GET` | `200` | SHOULD | No body | object keyed by index name with `mappings.properties` | missing index, wildcard mismatch |
| `/{index}/_doc/{id}` | `PUT`, `POST` | `201` on create, `200` on overwrite | MUST | JSON source body; explicit document ID | `_index`, `_id`, `_version`, `result`, `_shards`, `_seq_no`, `_primary_term` | malformed JSON, mapping failure, OCC conflict |
| `/{index}/_doc` | `POST` | `201` | MUST | JSON source body; auto-generated ID | `_index`, `_id`, `_version`, `result`, `_shards`, `_seq_no`, `_primary_term` | malformed JSON, mapping failure |
| `/{index}/_create/{id}` | `PUT`, `POST` | `201` | MUST | JSON source body; create-only semantics | `_index`, `_id`, `_version`, `result`, `_shards`, `_seq_no`, `_primary_term` | existing ID conflict |
| `/{index}/_doc/{id}` | `GET` | `200` | MUST | No body | `_index`, `_id`, `_version`, `_seq_no`, `_primary_term`, `found`, optional `_source` | missing index |
| `/{index}/_doc/{id}` | `HEAD` | `200` or `404` | MUST | No body | no body required | missing document, missing index |
| `/{index}/_doc/{id}` | `DELETE` | `200` | MUST | No body | `_index`, `_id`, `_version`, `result`, `_shards`, `_seq_no`, `_primary_term` | missing index, OCC conflict |
| `/_bulk` | `POST`, `PUT` | `200` | MUST | NDJSON body; per-item `_index` or default path index required | `took`, `errors`, `items` | malformed NDJSON, payload too large |
| `/{index}/_bulk` | `POST`, `PUT` | `200` | MUST | NDJSON body; path index becomes default for items without `_index` | `took`, `errors`, `items` | malformed NDJSON, payload too large |
| `/_refresh`, `/{index}/_refresh` | `POST`, `GET` | `200` | SHOULD | No body | `_shards.total`, `_shards.successful`, `_shards.failed` | missing index, wildcard mismatch |
| `/_flush`, `/{index}/_flush` | `POST`, `GET` | `200` | LATER | No body | `_shards.total`, `_shards.successful`, `_shards.failed` | missing index, unsupported implementation |

## Query Parameter Contract

Only the following parameters are part of the MVP compatibility target.

### Create Index

- MUST accept `wait_for_active_shards`
- SHOULD accept `cluster_manager_timeout`
- SHOULD accept `timeout`

### Get Index

- SHOULD accept `flat_settings`
- SHOULD accept `include_defaults`
- LATER: `allow_no_indices`, `expand_wildcards`, `ignore_unavailable`, `local`, `cluster_manager_timeout`

### Index Document And Create-Only Document

- MUST accept `refresh`
- MUST accept `if_seq_no`
- MUST accept `if_primary_term`
- MUST accept `op_type`
- SHOULD accept `routing`
- SHOULD accept `wait_for_active_shards`
- LATER: `version`, `version_type`, `require_alias`, `pipeline`, `timeout`

Rules:

- `POST /{index}/_doc` behaves as `op_type=create`
- `/{index}/_create/{id}` must reject overwrite attempts with `409`
- `PUT /{index}/_doc/{id}` defaults to create-or-overwrite semantics

### Get Document

- MUST accept `_source`
- MUST accept `_source_includes`
- MUST accept `_source_excludes`
- MUST accept `realtime`
- SHOULD accept `routing`
- SHOULD accept `refresh`
- LATER: `preference`, `stored_fields`

### Delete Document

- MUST accept `refresh`
- MUST accept `if_seq_no`
- MUST accept `if_primary_term`
- SHOULD accept `routing`
- SHOULD accept `wait_for_active_shards`
- LATER: `version`, `version_type`, `timeout`

### Bulk

- MUST accept `refresh`
- SHOULD accept `wait_for_active_shards`
- SHOULD accept `routing`
- LATER: `_source`, `_source_includes`, `_source_excludes`, `pipeline`, `require_alias`, `timeout`

### Refresh

- SHOULD accept `ignore_unavailable`
- SHOULD accept `allow_no_indices`
- SHOULD accept `expand_wildcards`

### Flush

- LATER: `force`, `wait_if_ongoing`, `ignore_unavailable`, `allow_no_indices`, `expand_wildcards`

## Request And Response Semantics

### Create Index

- Request body may contain `settings`, `mappings`, and `aliases`.
- MVP implementations must preserve the response keys `acknowledged`, `shards_acknowledged`, and `index`.
- For MVP, aliases are accepted only as part of create-index payload or returned from get-index; standalone alias APIs are out of scope.

### Get Index And Get Mapping

- `GET /{index}` returns an object keyed by concrete index name.
- `GET /{index}/_mapping` returns mapping data keyed by index name, not only the `properties` object.
- Mapping retrieval is the stronger requirement for MVP; get-index remains useful but non-blocking.

### Index Document

- Preserve underscore metadata fields exactly: `_index`, `_id`, `_version`, `_shards`, `_seq_no`, `_primary_term`.
- `result` must be `created` for a new document and `updated` for overwrite.
- Auto-ID creation must return a server-generated `_id` and `201`.

### Get Document

- `GET` for an existing document returns `200` with `found: true` and `_source` unless source filtering removes it.
- `GET` for a missing document in an existing index returns `200` with `found: false` and without `_source`.
- `HEAD` returns `200` when the document exists and `404` when it does not.
- If a document was indexed with custom routing, the same routing value is required for retrieval.

### Delete Document

- Deleting an existing document returns `200` with `result: deleted`.
- Deleting a missing document in an existing index returns `200` with `result: not_found`.
- Deleting from a missing index returns a not-found error envelope, not a `result: not_found` success body.

### Bulk

- Request body must be NDJSON with one action line per line and a source line immediately after `index` or `create` actions.
- Request body must end with a trailing newline.
- Content type must be `application/x-ndjson`.
- `index`, `create`, and `delete` actions are MVP scope.
- Each item response must preserve per-action metadata including `_index`, `_id`, `_version`, `result`, `_shards`, `_seq_no`, `_primary_term`, and per-item `status` where OpenSearch emits it.
- Top-level `errors` must become `true` if any item failed, even when HTTP status remains `200`.
- Partial success is required: one failed item must not abort the entire bulk request.

### Refresh And Flush

- Both route families accept `POST` and `GET`.
- Both return shard-summary objects under `_shards`.
- Prefer per-write `refresh` parameters over explicit refresh calls when the caller only needs visibility control.

## Error Envelope Contract

When an API call fails at request level, Surch should preserve the standard OpenSearch-style envelope:

```json
{
  "error": {
    "type": "...",
    "reason": "..."
  },
  "status": 400
}
```

Minimum failure cases to preserve:

- invalid index name -> `400`
- malformed JSON body -> `400`
- malformed bulk NDJSON -> `400`
- missing index on index-level admin routes -> `404`
- create-only or OCC conflict -> `409`
- unsupported-but-documented-deferred syntax in MVP -> `400` with explicit reason

Bulk is special:

- malformed whole-request NDJSON is a request-level error
- item-level failures stay inside `items[]` and still return top-level HTTP `200`

## Compatibility Traps

- Do not rename underscore fields.
- Do not convert missing-document `GET` into `404`; OpenSearch uses `200` plus `found: false` when the index exists.
- Do not convert missing-document `DELETE` into `404`; OpenSearch uses `200` plus `result: not_found` when the index exists.
- Do not accept pretty-printed multi-line JSON objects inside NDJSON bulk items.
- Do not silently ignore unsupported bulk actions; reject `update` explicitly until implemented.
- `cluster_manager_timeout` is the OpenSearch-era name; avoid backsliding into Elasticsearch-specific wording unless both are intentionally supported.
- Multi-target wildcard behavior exists upstream, but only endpoints marked `SHOULD` or `LATER` need it for MVP.

## MVP Test Inventory

### MUST

1. Create index with valid mappings and settings returns `acknowledged`, `shards_acknowledged`, and `index`.
2. Create index with invalid name returns request-level `400`.
3. Duplicate create returns conflict-style error.
4. Get mapping returns object keyed by index name and preserves `mappings.properties`.
5. Index document with explicit ID returns `201` on create and full underscore metadata.
6. Re-index same ID returns `200` and `result: updated`.
7. `POST /{index}/_doc` auto-generates an ID and returns `201`.
8. `/{index}/_create/{id}` rejects overwrite with `409`.
9. `GET /{index}/_doc/{id}` existing document returns `found: true` and `_source`.
10. `GET /{index}/_doc/{id}` missing document in existing index returns `200` with `found: false`.
11. `HEAD /{index}/_doc/{id}` returns `200` or `404` with no body contract dependency.
12. Delete existing document returns `result: deleted`.
13. Delete missing document in existing index returns `result: not_found`.
14. Bulk with valid NDJSON and mixed success returns HTTP `200`, top-level `errors`, and item-level statuses.
15. Bulk malformed NDJSON returns request-level `400`.
16. Bulk `update` action is rejected explicitly until supported.
17. `refresh=true|false|wait_for` is honored on single-document and bulk writes.

### SHOULD

1. Get index returns aliases, mappings, and settings keyed by index name.
2. Source filtering on get-document works for `_source`, `_source_includes`, and `_source_excludes`.
3. Explicit refresh endpoint returns `_shards` summary.
4. Routing-sensitive document retrieval and deletion behave correctly.

### LATER

1. Flush endpoint returns `_shards` summary.
2. External versioning.
3. Ingest pipeline integration.
4. Alias-only enforcement via `require_alias`.
5. Full wildcard semantics on every admin endpoint.

## Branch Readiness Notes

- This spec is ready to drive BR-03, BR-04, and BR-06 without requiring additional syntax decisions for core MVP routes.
- The only intentional deferral that may affect later implementation planning is `Flush`, which remains non-blocking for MVP.
