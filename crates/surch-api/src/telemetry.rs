//! Telemetry initialisation for `surch-api`.
//!
//! Two-step strategy:
//!
//! 1. **Today (this commit)** — a `tracing_subscriber::fmt` subscriber is
//!    installed at boot, gated by the `RUST_LOG` env var via `EnvFilter`
//!    (`info,surch_api=debug` is a sensible default). Application code
//!    annotates hot paths with `#[tracing::instrument]`; events land
//!    on stderr.
//! 2. **Later (round 8+)** — when `OTEL_EXPORTER_OTLP_ENDPOINT` is set,
//!    `init_telemetry()` will additionally install an OpenTelemetry
//!    layer exporting OTLP/gRPC. Today that branch only logs a warning
//!    so operators know the env var was seen but no exporter wired yet.
//!
//! Calling `init_telemetry()` more than once is a no-op: the
//! `try_init` form swallows the "subscriber already set" error so
//! test harnesses can call it from multiple `#[tokio::test]` entry
//! points without panicking.

use std::env;
use std::sync::OnceLock;

use tracing_subscriber::{EnvFilter, fmt};

const OTLP_ENDPOINT_ENV: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";

static INIT_RESULT: OnceLock<InitOutcome> = OnceLock::new();

/// Outcome of the one-shot telemetry initialisation. Stored in a
/// `OnceLock` so the first caller wins and subsequent callers learn
/// what happened without retrying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitOutcome {
    /// `RUST_LOG` subscriber installed; OTLP exporter not requested.
    FmtSubscriber,
    /// `RUST_LOG` subscriber installed; OTLP endpoint was set in env
    /// but the gRPC exporter is not wired in this build — a warning
    /// was logged.
    FmtWithOtlpPending,
    /// `tracing_subscriber::fmt::try_init` returned an error (another
    /// subscriber was already installed). The original subscriber
    /// stays in place.
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

    let result = fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_level(true)
        .try_init();

    if result.is_err() {
        return InitOutcome::AlreadyInitialised;
    }

    match env::var(OTLP_ENDPOINT_ENV) {
        Ok(endpoint) if !endpoint.is_empty() => {
            tracing::warn!(
                target: "surch_api::telemetry",
                otlp_endpoint = %endpoint,
                "{OTLP_ENDPOINT_ENV} is set but the OTLP gRPC exporter is not wired in this build; spans will not be forwarded. Tracked under round 8."
            );
            InitOutcome::FmtWithOtlpPending
        }
        _ => InitOutcome::FmtSubscriber,
    }
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
}
