//! Telemetry initialisation for `surch-api`.
//!
//! Two-layer strategy, both gated by the same idempotent entry point
//! `init_telemetry()`:
//!
//! 1. **`tracing_subscriber::fmt`** is always installed; its filter is
//!    driven by `RUST_LOG` via `EnvFilter`. The default when the env
//!    var is unset is `info,surch_api=debug`, which keeps third-party
//!    crates quiet while surfacing every Surch hot-path event.
//! 2. **OpenTelemetry layer** is installed on top of the `fmt` layer
//!    when `OTEL_EXPORTER_OTLP_ENDPOINT` is set and non-empty. The
//!    layer batches spans and forwards them via OTLP/gRPC (tonic) to
//!    the configured collector. Configuration knobs honoured:
//!    - `OTEL_EXPORTER_OTLP_ENDPOINT` — collector URL, e.g.
//!      `http://otel-collector:4317`.
//!    - `OTEL_TRACES_SAMPLER_ARG` — float in `[0.0, 1.0]`, default
//!      `1.0`. Wrapped in `Sampler::ParentBased(Sampler::TraceIdRatioBased(_))`
//!      so child spans inherit the parent decision and only root
//!      spans get sampled at the configured ratio.
//!    - `OTEL_SERVICE_NAME` — overrides the default `surch-api`
//!      service.name resource attribute.
//!
//! Calling `init_telemetry()` more than once is a no-op: the first
//! caller wins and the outcome is cached in a `OnceLock` so test
//! harnesses can call it from multiple `#[tokio::test]` entry points
//! without panicking.

use std::env;
use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider, Tracer as SdkTracer};
use opentelemetry_sdk::Resource;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

const OTLP_ENDPOINT_ENV: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const OTLP_SAMPLER_ARG_ENV: &str = "OTEL_TRACES_SAMPLER_ARG";
const OTLP_SERVICE_NAME_ENV: &str = "OTEL_SERVICE_NAME";
const DEFAULT_SERVICE_NAME: &str = "surch-api";
const OTLP_EXPORT_TIMEOUT: Duration = Duration::from_secs(3);

static INIT_RESULT: OnceLock<InitOutcome> = OnceLock::new();

/// Outcome of the one-shot telemetry initialisation. Stored in a
/// `OnceLock` so the first caller wins and subsequent callers learn
/// what happened without retrying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitOutcome {
    /// `fmt` subscriber installed; OTLP exporter not requested
    /// (`OTEL_EXPORTER_OTLP_ENDPOINT` unset or empty).
    FmtSubscriber,
    /// `fmt` subscriber + OTLP/gRPC exporter installed.
    FmtWithOtlp,
    /// `OTEL_EXPORTER_OTLP_ENDPOINT` was set but building the OTLP
    /// exporter failed (invalid URL, missing TLS root, etc.). The
    /// `fmt` subscriber is still installed and the failure was logged
    /// at WARN.
    FmtWithOtlpFailed,
    /// `try_init` returned an error (another subscriber was already
    /// installed). The original subscriber stays in place.
    AlreadyInitialised,
}

/// Install the global `tracing` subscriber.
///
/// Idempotent: only the first call has an effect. Safe to call from
/// `main` or from any `#[tokio::test]` that needs spans.
pub fn init_telemetry() -> InitOutcome {
    *INIT_RESULT.get_or_init(install_subscriber)
}

fn install_subscriber() -> InitOutcome {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,surch_api=debug"));

    // Attach the filter directly to the fmt layer so it doesn't appear
    // as a top-level subscriber-layer (which composes awkwardly with
    // the OTel layer's generic `S` parameter). The OTel layer below
    // is therefore unfiltered — every span/event matched by the
    // application-level filter reaches the exporter.
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_filter(filter);

    let endpoint = env::var(OTLP_ENDPOINT_ENV).ok().filter(|s| !s.is_empty());

    let Some(endpoint) = endpoint else {
        let result = tracing_subscriber::registry().with(fmt_layer).try_init();
        return match result {
            Ok(()) => InitOutcome::FmtSubscriber,
            Err(_) => InitOutcome::AlreadyInitialised,
        };
    };

    match build_otel_tracer(&endpoint) {
        Ok((tracer, provider)) => {
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            let result = tracing_subscriber::registry()
                .with(fmt_layer)
                .with(otel_layer)
                .try_init();
            match result {
                Ok(()) => {
                    // Keep the global provider alive so spans flush on
                    // shutdown. The SDK owns batching internally.
                    global::set_tracer_provider(provider);
                    tracing::info!(
                        target: "surch_api::telemetry",
                        otlp_endpoint = %endpoint,
                        "OTLP/gRPC tracer installed"
                    );
                    InitOutcome::FmtWithOtlp
                }
                Err(_) => InitOutcome::AlreadyInitialised,
            }
        }
        Err(err) => {
            // Exporter construction failed — fall back to fmt only so
            // the process keeps starting. Pre-init failure is emitted
            // to stderr because the global subscriber isn't installed
            // yet at this point.
            eprintln!(
                "surch_api::telemetry: failed to build OTLP exporter for {endpoint}: {err}; \
                 continuing with fmt subscriber only"
            );
            let result = tracing_subscriber::registry().with(fmt_layer).try_init();
            if result.is_err() {
                return InitOutcome::AlreadyInitialised;
            }
            InitOutcome::FmtWithOtlpFailed
        }
    }
}

fn build_otel_tracer(
    endpoint: &str,
) -> Result<(SdkTracer, SdkTracerProvider), Box<dyn std::error::Error + Send + Sync>> {
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(OTLP_EXPORT_TIMEOUT)
        .build()?;

    let ratio = env::var(OTLP_SAMPLER_ARG_ENV)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|r| r.clamp(0.0, 1.0))
        .unwrap_or(1.0);

    let sampler = Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(ratio)));

    let service_name = env::var(OTLP_SERVICE_NAME_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned());

    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new("service.name", service_name),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ])
        .build();

    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_sampler(sampler)
        .with_batch_exporter(exporter)
        .build();

    let tracer = provider.tracer(DEFAULT_SERVICE_NAME);

    Ok((tracer, provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_telemetry_is_idempotent() {
        let first = init_telemetry();
        let second = init_telemetry();
        assert_eq!(first, second);
    }

    /// When `OTEL_EXPORTER_OTLP_ENDPOINT` points at an unreachable
    /// endpoint, the tracer builder still returns successfully: tonic
    /// connects lazily on the first export attempt, so building does
    /// not block on the TCP handshake. The builder *does* need a Tokio
    /// reactor present (hyper-util grabs the current handle on
    /// construction), so the test runs under `tokio::test`.
    #[tokio::test]
    async fn build_otel_tracer_with_loopback_endpoint_succeeds() {
        let (_tracer, provider) =
            build_otel_tracer("http://127.0.0.1:1").expect("builder should not fail eagerly");
        // Drop the provider explicitly: forcing a flush would block on
        // the network, so we just rely on Drop to abort the background
        // exporter.
        drop(provider);
    }
}
