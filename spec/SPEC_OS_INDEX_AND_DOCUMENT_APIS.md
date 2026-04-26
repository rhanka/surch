# SPEC - OpenSearch Index And Document APIs

## Purpose

Capture the MVP-compatible OpenSearch syntax for index management and document APIs that Surch must reproduce.

## Sources

- OpenSearch Create Index API
- OpenSearch Delete Index API
- OpenSearch Get Index API
- OpenSearch Get Mapping API
- OpenSearch Index Document API
- OpenSearch Get Document API
- OpenSearch Delete Document API
- OpenSearch Bulk API
- OpenSearch Refresh API
- OpenSearch Flush API

## Endpoint Compatibility Matrix

| Endpoint | Methods | Required Path or Query Params | Expected Body | Critical Response Fields | Error Focus | MVP Priority |
|---|---|---|---|---|---|---|
| `/{index}` | `PUT` | path `index` | optional `settings`, `mappings`, `aliases` | `acknowledged`, `shards_acknowledged`, `index` | invalid name, mapping error, already exists | MUST |
| `/{index}` | `DELETE` | path `index` | none | `acknowledged` | missing index, wildcard handling | MUST |
| `/{index}` | `GET` | path `index` | none | `aliases`, `mappings`, `settings.index.*` | missing index | SHOULD |
| `/{index}/_mapping`, `/_mapping` | `GET` | optional or required path `index` depending on route | none | `mappings.properties` | missing index | MUST |
| `/{index}/_doc/{id}` | `PUT`, `POST` | path `index`, path `id` except auto-id route | document JSON | `_index`, `_id`, `_version`, `_seq_no`, `_primary_term`, `result`, `_shards` | version conflict, invalid payload | MUST |
| `/{index}/_doc/{id}` | `GET`, `HEAD` | path `index`, path `id` | none | `_index`, `_id`, `_version`, `found`, `_source`, `_seq_no`, `_primary_term` | missing doc, missing index | MUST |
| `/{index}/_doc/{id}` | `DELETE` | path `index`, path `id` | none | `_index`, `_id`, `_version`, `_seq_no`, `_primary_term`, `result`, `_shards` | missing doc, version conflict | MUST |
| `/_bulk`, `/{index}/_bulk` | `POST`, `PUT` | optional path `index` | NDJSON action lines plus source lines | `errors`, `items`, `took` | malformed NDJSON, oversized payload | MUST |
| `/_refresh`, `/{index}/_refresh` | `POST`, `GET` | optional path `index` | none | `_shards.total`, `_shards.successful`, `_shards.failed` | missing index | SHOULD |
| `/_flush`, `/{index}/_flush` | `POST`, `GET` | optional path `index` | none | `_shards.total`, `_shards.successful`, `_shards.failed` | missing index | LATER |

## Compatibility Traps

- Preserve underscore response fields exactly: `_index`, `_id`, `_version`, `_seq_no`, `_primary_term`, `_shards`, `_source`
- Bulk payload is NDJSON, not regular JSON
- Bulk should usually return HTTP `200` even when `errors=true` at item level
- `POST /{index}/_doc` may auto-generate IDs; `PUT /{index}/_doc/{id}` must use the given ID
- `op_type=create` and `/{index}/_create/{id}` must reject overwrite attempts
- `cluster_manager_timeout` may appear in OpenSearch where Elasticsearch historically used `master_timeout`

## Required Integration Scenarios

1. Create index with mappings succeeds
2. Duplicate create returns already-exists error
3. Get index returns settings and mappings
4. Get mapping returns `properties`
5. Delete missing index returns not-found error
6. Index document returns underscore fields and `created`
7. Create-on-existing-ID returns conflict
8. Get existing document returns `found=true`
9. Get missing document returns correct missing shape
10. Delete existing document returns `deleted`
11. Bulk valid NDJSON returns `errors=false` or item-level mix
12. Bulk malformed NDJSON returns parse error
13. Refresh returns shard summary
14. Flush returns shard summary or documented defer behavior

## MVP Notes

- `Get Index` is `SHOULD`, not a release blocker if mapping retrieval is already present and behavior is explicitly documented
- `Flush` is `LATER` if the storage contract is not strong enough yet, but its deferral must be explicit in branch and release notes
