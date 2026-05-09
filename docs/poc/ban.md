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

## Demo With Official BAN Data

The BAN demo has three distinct concerns:

1. **Official BAN autocomplete**: the SvelteKit UI can read an official BAN CSV
   extract from `adresse.data.gouv.fr` and serve address suggestions from the
   demo repository layer.
2. **Active engine loading**: Surch and OpenSearch must be loaded through the
   fixed BAN engine operations before their search responses are compared. This
   is the active runtime path; it is separate from autocomplete suggestions.
3. **Comparative benchmark**: the current numbers are not publishable as a
   global Surch/OpenSearch performance ratio. Surch is still measured through an
   in-process in-memory router for the local bench, while OpenSearch would run
   as an HTTP engine. Treat timings as smoke/regression signals only until the
   runtime paths are symmetric.

The SvelteKit demo knows the official BAN CSV source published on data.gouv.fr:

- Dataset: `Base Adresse Nationale`
- CSV directory: `https://adresse.data.gouv.fr/data/ban/adresses/latest/csv`
- Default demo file: `adresses-75.csv.gz`
- Full national file: `adresses-france.csv.gz`

Download the default Paris dataset without committing it:

```bash
cd demo
npm run ban:download
BAN_CSV_PATH=data/ban/adresses-75.csv.gz npm run dev
```

Download the full national BAN only when the machine has enough disk and memory:

```bash
cd demo
npm run ban:download:france
BAN_CSV_PATH=data/ban/adresses-france.csv.gz BAN_SAMPLE_LIMIT=25000 npm run dev
```

The downloaded files live under `demo/data/ban/`, which is git-ignored.
The backend parser caps loaded rows through `BAN_SAMPLE_LIMIT` so the demo can
remain responsive while Surch is still in an in-memory bootstrap mode.

## Local BAN UAT Flow

Expected local flow for the SvelteKit BAN demo:

1. Download the BAN extract:

   ```bash
   cd demo
   npm run ban:download
   ```

2. Start the local Surch API and a local OpenSearch node. The demo defaults are
   `SURCH_URL=http://127.0.0.1:7700` and
   `OPENSEARCH_URL=http://127.0.0.1:9200`; override them in the demo environment
   if either service runs elsewhere. OpenSearch must be running and reachable
   before loading/comparing if the OpenSearch column is expected to show `ok`.

   ```bash
   scripts/bench/opensearch-start.sh
   scripts/bench/opensearch-wait.sh
   ```

3. Start the demo with the downloaded BAN file:

   ```bash
   BAN_CSV_PATH=data/ban/adresses-75.csv.gz npm run dev
   ```

4. In the browser, click **Charger BAN**. This loads the active BAN dataset into
   Surch and OpenSearch through the fixed `ban_addresses` index. If the page
   still reports `ban_tiny`, it is showing the local fallback dataset rather
   than the downloaded BAN extract.

5. Type an address, select one of the BAN suggestions, then click **Comparer**.
   Comparing without selecting a suggestion is outside the intended UAT path.
   With both engines running, both comparison columns should report `ok`; if
   OpenSearch is stopped or unreachable, only the OpenSearch column should show
   the upstream error.

The demo/benchmark zone is explicitly Rust, TypeScript, and shell only. Do not
add Python scripts, Python notebooks, or Python one-off tooling here.

## Reproducible Surch/OpenSearch Benchmark Plan

The next publishable benchmark must use the same data, the same queries, and the
same validation rules for both engines:

- Data: load the same BAN fixture or sampled official BAN CSV into Surch and
  OpenSearch. Record the exact file, sample limit, Surch commit, Rust profile,
  OpenSearch version or image digest, heap, host OS, CPU, and memory.
- Queries: replay the same `_count` and `_search` requests for match, bool, and
  fuzzy cases. Do not add engine-specific query shortcuts.
- Oracle validation: before measuring, verify response status, total hits, and
  expected top-hit IDs against the replay oracle. Reject the run if validation
  fails.
- Warmup: run an unmeasured warmup pass after index load and refresh so first-use
  effects do not dominate the reported sample.
- Measurements: report ingestion duration, docs/s, error count, and query
  latency p50 and p95 at minimum. Keep min/max or p99 as secondary diagnostics.
- Publication rule: publish raw per-operation measurements and methodology only.
  Do not extrapolate from `ban_tiny`, do not claim production throughput, and do
  not publish a global Surch/OpenSearch ratio until both engines are measured
  through comparable runtime paths.

## Current Scope

This PoC is intentionally small. It proves the HTTP compatibility path, oracle
replay path, BAN fixture loading, and fuzzy query behavior on deterministic
data. It does not yet claim full OpenSearch feature coverage, distributed
execution, disk-backed indexing, or production-grade scoring parity for this API
path.
