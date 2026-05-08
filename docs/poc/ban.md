# Surch BAN PoC

This proof of concept demonstrates a small OpenSearch-compatible flow on a tiny
Base Adresse Nationale fixture:

- create an index;
- ingest three BAN address documents through `_bulk`;
- refresh the index;
- run `_count`;
- run `match`, `bool.must`, and fuzzy search queries.

The fixture is stored in `tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson`.
The replay oracle is stored in `tests/opensearch_compat/oracle/replays/ban_tiny_search.json`.

## Run the PoC

```bash
cargo run -p surch-demo -- ban-poc
```

Expected stable identifiers:

```text
Surch BAN PoC
dataset: ban_tiny
documents: 3
count: 3
match label: 75101_0001_00001
bool address: 33063_0002_00010B
fuzzy label: 67482_0003_00007
oracle: tests/opensearch_compat/oracle/replays/ban_tiny_search.json
```

## Run the Local Bench

```bash
cargo run -p surch-demo --release -- ban-bench --iterations 1000
```

The bench reports:

- `load_ban_tiny`: create index, bulk ingest, refresh;
- `count_match_all`: `_count` on the loaded in-memory index;
- `search_match_label`: `match` query on `label`;
- `search_bool_address`: `bool.must` query on `street_name` and `postcode`;
- `search_fuzzy_label`: fuzzy query with edit distance 2 on `label`.

These are local in-memory router measurements. They are useful for tracking
relative regressions on the same host and build, not for claiming production
cluster throughput.

## Current Scope

This PoC is intentionally small. It proves the HTTP compatibility path, oracle
replay path, BAN fixture loading, and fuzzy query behavior on deterministic
data. It does not yet claim full OpenSearch feature coverage, distributed
execution, disk-backed indexing, or production-grade scoring parity for this API
path.
