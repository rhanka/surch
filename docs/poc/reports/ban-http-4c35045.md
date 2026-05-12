# BAN HTTP Benchmark Report

Date: 2026-05-12
Surch commit under test: `4c35045`
Dataset: `tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson`
Oracle: `tests/opensearch_compat/oracle/replays/ban_tiny_http_bench.json`
Report JSON: `docs/poc/reports/ban-http-4c35045.json`

## Scope

This is a 3-document smoke benchmark. It proves that the benchmark harness can
load Surch and OpenSearch through the same HTTP sequence, validate the replay
oracle before timing, run warmup, and emit raw latency samples. It is not a
production performance claim.

Configuration:

- Surch URL: `http://127.0.0.1:7700`
- OpenSearch URL: `http://127.0.0.1:9200`
- OpenSearch version: `2.17.1`
- Warmup: 100 requests per replay operation
- Iterations: 1000 requests per measured operation
- Timeout: 30 seconds
- HTTP client: same Rust HTTP/1.1 keep-alive client for both engines

## Results

Setup latencies are single samples and should be treated as smoke data.

| Operation | Surch us | OpenSearch us |
| --- | ---: | ---: |
| create_index | 342 | 343383 |
| bulk_ingest | 914 | 203207 |
| refresh | 206 | 126797 |

Query latencies are client-observed microseconds over 1000 measured iterations.

| Operation | Surch p50 | Surch p95 | Surch p99 | OpenSearch p50 | OpenSearch p95 | OpenSearch p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| count_ban_tiny_addresses | 119 | 240 | 326 | 3037 | 6754 | 9252 |
| search_ban_tiny_by_label | 267 | 493 | 2951 | 4467 | 8527 | 12424 |
| search_ban_tiny_by_address_fields | 341 | 557 | 1341 | 3928 | 7214 | 10035 |

All measured operations returned HTTP 200, zero benchmark-level errors, and the
expected hit counts/top-hit IDs.

## Compatibility Note

The broader `ban_tiny_search.json` replay still includes
`future_fuzzy_label_typo`. During this real HTTP run, Surch returned the expected
`67482_0003_00007` hit for `Ale des Erables`, while OpenSearch 2.17.1 returned
zero hits. The symmetric benchmark therefore uses `ban_tiny_http_bench.json`,
which excludes this known fuzzy gap and measures only the currently compatible
overlap.

This gap must be classified against MatchID query logs before any shadow UAT
claim if fuzzy address typo matching is part of the workload.
