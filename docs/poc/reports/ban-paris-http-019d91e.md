# BAN Paris HTTP Benchmark Attempt

Date: 2026-05-12
Surch commit under test: `019d91e`
Source file: `demo/data/ban/adresses-75.csv.gz`
Source SHA-256: `348d373b0381d745ce29489404094296f09ee42619e049dae7b38cd41a578d92`
OpenSearch version: `2.17.1`

## Generated Datasets

Generated with `cargo run -p opensearch-oracle --bin ban-to-ndjson`.

| Dataset | Rows | CSV bytes | CSV SHA-256 | NDJSON bytes | NDJSON SHA-256 |
| --- | ---: | ---: | --- | ---: | --- |
| `ban_paris_25000` | 25000 | 4533549 | `427f9071a6a9d03ff0e35324ebabd20fb1ac6212bb9b650e3959d6df7378d172` | 8542789 | `0b5283ba27c8a293e67b7f46dd37da6b34e9efaa2bf3fde9f10f78bf589d94dc` |
| `ban_paris_500` | 500 | 89856 | `1712c150bcd035f35fdf5d742a8ee5e4721eed3e4de756e0028294ea7364e5c1` | 168481 | `e409a0911f720e7767eb57f667778652a724edbe12ee612b95741422706444c4` |
| `ban_paris_100` | 100 | 17434 | `510036cccaf7bf8e1c6a592cf0a2a951c91482737ac1921db2fac60c77b0d1d9` | 33000 | `42366611ef882c4426655ae4be12c7ef3b6353241580a6aa2299e64ca1727b95` |

The generated CSV/NDJSON files are local artifacts under `target/ban-bench/` and
are intentionally not committed.

## Load Attempts

Manual HTTP load path: `DELETE /<index>`, `PUT /<index>`, `POST /_bulk`,
`POST /<index>/_refresh`, `GET /<index>/_count`.

| Dataset | OpenSearch result | Surch result |
| --- | --- | --- |
| `ban_paris_25000` | `_bulk` returned `errors:false`, `_count=25000` | `_bulk` timed out after 120s with no response; `_count=557` after client timeout |
| `ban_paris_500` | `_bulk` returned `errors:false`, `_count=500` | `_bulk` timed out after 120s with no response; `_count=271` after client timeout |
| `ban_paris_100` | `_bulk` returned `errors:false`, `_count=100` | `_bulk` returned `errors:false`, `_count=100` |

## Paris 100 Query Probe

The `ban_paris_100` replay fixture is
`tests/opensearch_compat/oracle/replays/ban_paris_100_http_bench.json`.

Manual query probes after loading `ban_paris_100`:

| Operation | OpenSearch | Surch |
| --- | --- | --- |
| `count_ban_paris_100_addresses` | `_count=100` | `_count=100` |
| `search_ban_paris_100_place_patrice_chereau` | top hit `75103_vaip3v_00001`, `took=718ms` | top hit `75103_vaip3v_00001`, `took=9243ms` |
| `search_ban_paris_100_rue_payenne` | top hit `75103_7205_00001`, total `18`, `took=236ms` | top hit `75103_7205_00001`, total `18`, `took=26020ms` |

The full `ban-http-bench` run for Paris 100 could not be executed in this turn
because the environment rejected the required local-network escalation after the
manual probes. The replay and local NDJSON generation path are committed so the
run can be repeated directly when local endpoint access is available.

## Decision

The Paris attempt changes the readiness assessment: the next performance
blocker is API ingestion and request-time scan cost. Larger Paris samples are
not benchmarkable yet through the current HTTP API because `_bulk` does not
complete within the benchmark timeout and leaves partial data visible after the
client disconnects.
