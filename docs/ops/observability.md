# Observability

Surch exposes three classes of operational signals.

## 1. Prometheus metrics (live, scraped)

- Endpoint: `GET /_prometheus_metrics` on every Surch node, plain text
  exposition format.
- Counters / histograms are declared in `crates/surch-api/src/metrics.rs`.
  Notable series:
  - `surch_search_requests_total{index,outcome}`
  - `surch_search_duration_seconds{index}`
  - `surch_bulk_documents_total{index,outcome}`
  - `surch_cache_hits_total{index,kind}` (per-index LRU response cache).
- Scrape job for `prometheus.yml`:

  ```yaml
  - job_name: surch
    metrics_path: /_prometheus_metrics
    static_configs:
      - targets: ["surch.your-cluster:7700"]
  ```

## 2. Structured logs (live, stderr)

- Initialised on boot by `crates/surch-api/src/telemetry.rs::init_telemetry`.
- Subscriber: `tracing_subscriber::fmt` with `EnvFilter` driven by
  `RUST_LOG`.
- Default filter if `RUST_LOG` is unset: `info,surch_api=debug` — keeps
  third-party crates quiet while surfacing every Surch hot-path event.
- Override examples:
  - `RUST_LOG=warn` — only WARN/ERROR everywhere.
  - `RUST_LOG=info,surch_api::search=trace` — drill into one module.
  - `RUST_LOG=off` — silence entirely.

`init_telemetry` is idempotent (`OnceLock` guard), so test harnesses
that spawn multiple Tokio runtimes can call it without panicking.

## 3. Distributed traces (OTLP) — pending

`OTEL_EXPORTER_OTLP_ENDPOINT` is reserved. When set, `init_telemetry`
emits a `WARN` line acknowledging the endpoint and returns
`InitOutcome::FmtWithOtlpPending`. The OTLP/gRPC exporter itself
(via `opentelemetry-otlp` + `tracing-opentelemetry`) is **not wired
in this build** — that's tracked in the wp/c-ops backlog as a
round-8+ deliverable so the dependency surface stays small until a
real collector is provisioned.

Once the exporter ships, the same `init_telemetry` entry point will
add an OpenTelemetry layer alongside the `fmt` subscriber. No
caller-facing API change is expected.

### Roadmap

- Spans on the hot paths (`run_search`, `bulk_index`, `apply_bulk_op`,
  `snapshot::export`, `snapshot::import`) via `#[tracing::instrument]`.
- W3C trace-context propagation through an axum middleware that
  extracts `traceparent` and starts a root span per request.
- Sampler configurable via `OTEL_TRACES_SAMPLER_ARG`
  (`parent_based(trace_id_ratio_based(N))`).

## 4. RSS sampling (offline benches only)

`scripts/bench/rss-sample.sh` polls `pidstat` and emits a JSON envelope
under the `surch.bench.rss.v1` schema; consumed by
`crates/surch-demo::bench_report`. Not part of the live signal set.
