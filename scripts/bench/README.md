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
- `ban-http-smoke.sh` resets only the dedicated `ban_tiny` smoke index on Surch and OpenSearch.
- No Python is used.

## BAN HTTP Smoke

`ban-http-smoke.sh` is a manual shell/curl parity smoke for the committed
3-document BAN fixture. It does not produce benchmark timings and must not be
treated as the future measured `ban-http-bench` result.

Prerequisites:

- Surch API running on `http://127.0.0.1:7700`, for example:

  ```sh
  SURCH_PORT=7700 cargo run -p surch-api --release
  ```

- OpenSearch running on `http://127.0.0.1:9200`, for example:

  ```sh
  scripts/bench/opensearch-start.sh
  scripts/bench/opensearch-wait.sh
  ```

Run:

```sh
scripts/bench/ban-http-smoke.sh
```

Environment variables:

- `SURCH_URL`: Surch API base URL; default `http://127.0.0.1:7700`.
- `OPENSEARCH_URL`: OpenSearch base URL; default `http://127.0.0.1:9200`.
- `BAN_HTTP_DATASET`: NDJSON fixture path; default
  `tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson`.
- `BAN_HTTP_TIMEOUT`: per-request curl timeout in seconds; default `10`.

The smoke checks both engines, deletes/recreates only `ban_tiny`, loads the
NDJSON through `_bulk`, refreshes `ban_tiny`, and verifies `_count == 3`.

## Symmetric HTTP Benchmark Runbook

These scripts manage only the OpenSearch side. For a publishable Surch vs
OpenSearch benchmark, Surch must also run as an HTTP server; do not compare
OpenSearch HTTP timings with the current in-process `ban-bench` timings.

Prerequisites:

- release Surch API server in a separate terminal:

  ```sh
  SURCH_PORT=7700 cargo run -p surch-api --release
  ```

- local OpenSearch node:

  ```sh
  scripts/bench/opensearch-start.sh
  scripts/bench/opensearch-wait.sh
  ```

- pinned BAN smoke fixture and oracle:

  ```sh
  DATASET=tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson
  ORACLE=tests/opensearch_compat/oracle/replays/ban_tiny_http_bench.json
  SURCH_URL=http://127.0.0.1:7700
  OPENSEARCH_URL=http://127.0.0.1:9200
  ```

Before each measured run, reset the benchmark index on both engines:

```sh
OPENSEARCH_BAN_INDEX=ban_tiny scripts/bench/opensearch-cleanup.sh
curl -fsS -X DELETE "$SURCH_URL/ban_tiny" >/dev/null || true
```

The Rust-only benchmark command is implemented in `surch-demo` and executes the
same HTTP load/refresh/oracle/query sequence against Surch and OpenSearch. Use
`--dry-run` only when you want to print the plan without sending HTTP requests.

```sh
cargo run -p surch-demo --release -- ban-http-bench \
  --surch-url "$SURCH_URL" \
  --opensearch-url "$OPENSEARCH_URL" \
  --dataset "$DATASET" \
  --oracle "$ORACLE" \
  --warmup 100 \
  --timeout-seconds 30 \
  --iterations 1000 \
  --report docs/poc/reports/ban-http-$(git rev-parse --short HEAD).json
```

Publication guardrails:

- same dataset bytes, index name, query bodies, warmup, iterations, timeout,
  and persistent HTTP/1.1 client code for both engines;
- reject the run if oracle validation fails for either engine;
- report ingestion status/latency/docs/s/bytes/s and query
  status/p50/p95/p99/raw samples/error counts/total hits/top-hit IDs per
  operation;
- label `ban_tiny` as a 3-document smoke benchmark;
- publish side-by-side per-operation numbers only, not a single global ratio.

For bounded official BAN samples, generate local NDJSON under `target/ban-bench`
with the Rust converter and keep the data out of git:

```sh
mkdir -p target/ban-bench
gzip -dc demo/data/ban/adresses-75.csv.gz | sed -n '1,101p' \
  > target/ban-bench/ban-paris-100.csv
cargo run -p opensearch-oracle --bin ban-to-ndjson -- \
  --input target/ban-bench/ban-paris-100.csv \
  --output target/ban-bench/ban-paris-100.ndjson \
  --index ban_paris_100 \
  --limit 100
```
