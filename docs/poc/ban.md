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

4. Open the browser. When the page sees an active CSV dataset, it loads the BAN
   dataset into Surch and OpenSearch automatically through the fixed
   `ban_addresses` index. **Charger BAN** remains available as a manual reload
   action. If the page still reports `ban_tiny`, it is showing the local
   fallback dataset rather than the downloaded BAN extract.

5. Type an address, select one of the BAN suggestions, then click **Comparer**.
   Comparing without selecting a suggestion is outside the intended UAT path.
   With both engines running, both comparison columns should report `ok`; if
   OpenSearch is stopped or unreachable, only the OpenSearch column should show
   the upstream error.

The demo/benchmark zone is explicitly Rust, TypeScript, and shell only. Do not
add Python scripts, Python notebooks, or Python one-off tooling here.

## Reproducible Surch/OpenSearch Benchmark Plan

The next publishable benchmark must be symmetric HTTP-to-HTTP. The current
`ban-bench` command remains useful for Surch regression smoke tests, but it is
an in-process Axum router benchmark and must not be mixed with OpenSearch HTTP
numbers.

### Prerequisites

- Rust release build available from this workspace; no npm install, UI load, or
  SvelteKit dependency change is part of the benchmark.
- Docker or Podman for the dedicated local OpenSearch node managed by
  `scripts/bench/`.
- Shell tools only: `bash`, `curl`, `git`, `date`, and standard Unix inspection
  commands such as `uname`. Do not add Python scripts, notebooks, or one-off
  Python tooling.
- Surch HTTP server started with the release profile:

  ```bash
  SURCH_PORT=7700 cargo run -p surch-api --release
  ```

- OpenSearch started and checked through the existing shell helpers:

  ```bash
  scripts/bench/opensearch-start.sh
  scripts/bench/opensearch-wait.sh
  ```

- Dataset and oracle pinned in the run record:

  ```bash
  DATASET=tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson
  ORACLE=tests/opensearch_compat/oracle/replays/ban_tiny_search.json
  SURCH_URL=http://127.0.0.1:7700
  OPENSEARCH_URL=http://127.0.0.1:9200
  ```

### Current Smoke Command

Use the existing local Surch-only command only as a regression signal:

```bash
cargo run -p surch-demo --release -- ban-bench --iterations 1000
```

The report must keep the label `Surch in-process axum router` and must state
that OpenSearch is not measured by that command.

### Manual HTTP Parity Smoke

Before the benchmark harness exists, use this shell-only sequence to prove that
both engines can be loaded and queried through comparable HTTP paths:

```bash
export DATASET=tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson
export SURCH_URL=http://127.0.0.1:7700
export OPENSEARCH_URL=http://127.0.0.1:9200

OPENSEARCH_BAN_INDEX=ban_tiny scripts/bench/opensearch-cleanup.sh
curl -fsS -X DELETE "$SURCH_URL/ban_tiny" >/dev/null || true

curl -fsS -X PUT "$SURCH_URL/ban_tiny" -H 'Content-Type: application/json' -d '{}'
curl -fsS -X PUT "$OPENSEARCH_URL/ban_tiny" -H 'Content-Type: application/json' -d '{}'

curl -fsS -X POST "$SURCH_URL/_bulk" \
  -H 'Content-Type: application/x-ndjson' \
  --data-binary "@$DATASET"
curl -fsS -X POST "$OPENSEARCH_URL/_bulk" \
  -H 'Content-Type: application/x-ndjson' \
  --data-binary "@$DATASET"

curl -fsS -X POST "$SURCH_URL/ban_tiny/_refresh"
curl -fsS -X POST "$OPENSEARCH_URL/ban_tiny/_refresh"

curl -fsS "$SURCH_URL/ban_tiny/_count"
curl -fsS "$OPENSEARCH_URL/ban_tiny/_count"

curl -fsS -X POST "$SURCH_URL/ban_tiny/_search" \
  -H 'Content-Type: application/json' \
  -d '{"query":{"match":{"label":"Rue de Rivoli"}}}'
curl -fsS -X POST "$OPENSEARCH_URL/ban_tiny/_search" \
  -H 'Content-Type: application/json' \
  -d '{"query":{"match":{"label":"Rue de Rivoli"}}}'

curl -fsS -X POST "$SURCH_URL/ban_tiny/_search" \
  -H 'Content-Type: application/json' \
  -d '{"query":{"bool":{"must":[{"match":{"street_name":"Cours de l'\''Intendance"}},{"match":{"postcode":"33000"}}]}}}'
curl -fsS -X POST "$OPENSEARCH_URL/ban_tiny/_search" \
  -H 'Content-Type: application/json' \
  -d '{"query":{"bool":{"must":[{"match":{"street_name":"Cours de l'\''Intendance"}},{"match":{"postcode":"33000"}}]}}}'

curl -fsS -X POST "$SURCH_URL/ban_tiny/_search" \
  -H 'Content-Type: application/json' \
  -d '{"query":{"fuzzy":{"label":{"value":"Ale des Erables","fuzziness":2}}}}'
curl -fsS -X POST "$OPENSEARCH_URL/ban_tiny/_search" \
  -H 'Content-Type: application/json' \
  -d '{"query":{"fuzzy":{"label":{"value":"Ale des Erables","fuzziness":2}}}}'
```

The expected top-hit identifiers are:

- count: `3`;
- match label: `75101_0001_00001`;
- bool address: `33063_0002_00010B`;
- fuzzy label: `67482_0003_00007`.

### Target Benchmark Command

Add a Rust-only HTTP harness before publication. The target CLI should be:

```bash
cargo run -p surch-demo --release -- ban-http-bench \
  --surch-url "$SURCH_URL" \
  --opensearch-url "$OPENSEARCH_URL" \
  --dataset "$DATASET" \
  --oracle "$ORACLE" \
  --warmup 100 \
  --iterations 1000 \
  --report docs/poc/reports/ban-http-$(git rev-parse --short HEAD).json
```

The command must load both engines, refresh both indexes, validate both engines
against the oracle before timing, run the same warmup and measured request
sequence through the same HTTP client code, and write raw per-operation samples
plus a human-readable summary.

### Metrics

Record these fields for every publishable run:

- run metadata: UTC timestamp, Surch commit, dirty-worktree flag, Rust version,
  Cargo profile, host OS/kernel, CPU model/count, memory, OpenSearch image or
  digest, OpenSearch heap, dataset path, dataset byte size, document count, and
  sample limit;
- ingestion: create-index duration, bulk duration, refresh duration, total load
  duration, docs/s, bytes/s, HTTP status, and bulk item error count per engine;
- queries: client-observed latency min, p50, p95, p99, max, sample count,
  timeout count, HTTP error count, OpenSearch `took` when present, total hits,
  top-hit ID, and max score recorded separately from latency;
- validation: oracle status for each request, expected top-hit ID, actual
  top-hit ID, expected hit count, actual hit count, and explicit failure reason
  when a sample is rejected.

### Guardrails

- Same dataset bytes, same index name, same query bodies, same warmup count,
  same measured iteration count, same timeout, and same concurrency for both
  engines.
- Reset the benchmark index before each measured run. Do not benchmark on a
  reused index.
- Use loopback HTTP for both engines. Do not compare Surch in-process timings
  with OpenSearch HTTP timings.
- Reject the full run if either engine fails an oracle check, returns a non-2xx
  response, reports bulk item errors, or times out during validation.
- Keep `ban_tiny` results labeled as a 3-document smoke benchmark. Use a pinned
  official BAN sample before making any public performance statement.
- Publish raw per-operation tables and methodology. Avoid a single global
  Surch/OpenSearch ratio or production throughput claim.

### Criteria Before Publication

- The Rust-only HTTP harness exists, is documented, and has tests for argument
  validation, oracle rejection, report serialization, and failed upstream HTTP
  responses.
- `cargo test --workspace` passes after the harness change.
- Surch and OpenSearch both pass the replay oracle before timing on `ban_tiny`.
- The official BAN sample run records the exact CSV/GZ file, sample limit, and
  generated NDJSON checksum.
- At least five measured runs are captured on the same host. If p95 latency for
  any operation varies by more than 15% across runs, the report must explain the
  variance instead of publishing a headline comparison.
- The report includes the guardrails above and explicitly separates
  compatibility/parity findings from performance findings.

### Next Tasks

1. Add `ban-http-bench` as a Rust-only CLI command in `crates/surch-demo` or a
   future benchmark crate, without touching UI loading or npm dependencies.
2. Implement oracle validation before measurement and fail closed on any mismatch.
3. Emit JSON plus a compact Markdown summary under `docs/poc/reports/`.
4. Run the manual HTTP parity smoke on `ban_tiny`.
5. Run the harness on `ban_tiny`, then on the pinned Paris BAN sample.
6. Review the generated report for methodology, caveats, and reproducibility
   before publishing.

## Current Scope

This PoC is intentionally small. It proves the HTTP compatibility path, oracle
replay path, BAN fixture loading, and fuzzy query behavior on deterministic
data. It does not yet claim full OpenSearch feature coverage, distributed
execution, disk-backed indexing, or production-grade scoring parity for this API
path.
