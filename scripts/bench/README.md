# BAN OpenSearch Bench Scripts

Shell-only lifecycle helpers for a local single-node OpenSearch used by the BAN demo.

```sh
scripts/bench/opensearch-start.sh
scripts/bench/opensearch-wait.sh
scripts/bench/opensearch-cleanup.sh
scripts/bench/opensearch-stop.sh
```

Defaults:

- `OPENSEARCH_URL=http://127.0.0.1:9200`
- `OPENSEARCH_PORT=9200`
- `OPENSEARCH_IMAGE=opensearchproject/opensearch:2.17.1`
- `OPENSEARCH_HEAP=512m`
- `OPENSEARCH_CONTAINER_NAME=surch-ban-opensearch`
- `OPENSEARCH_BAN_INDEX=ban_addresses`

The container runs with `discovery.type=single-node`, a fixed heap, and OpenSearch security disabled for local demo use.

Safety notes:

- `opensearch-stop.sh` only stops/removes a dedicated container name prefixed with `surch-` or `surch_`.
- `opensearch-cleanup.sh` only deletes a dedicated BAN/Surch index name prefixed with `ban-`, `ban_`, `surch-`, or `surch_`.
- No Python is used.
