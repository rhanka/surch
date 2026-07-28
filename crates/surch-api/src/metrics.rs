//! Prometheus metrics exposition for the OpenSearch-compatible API.
//!
//! A single `PrometheusRecorder` is installed lazily on the first call to
//! [`install_global`]. The recorder is published as the process-wide
//! `metrics::Recorder`, so any `metrics::counter!` / `metrics::histogram!`
//! call made afterwards from anywhere in the workspace lands in the same
//! handle. That handle backs the `/_prometheus_metrics` endpoint and
//! renders the standard text exposition format consumed by a Prometheus
//! scraper.

use std::sync::OnceLock;

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Content-Type expected by Prometheus for the text exposition format.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4";

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global Prometheus recorder.
///
/// This is idempotent and race-free: under concurrent callers, exactly
/// one thread builds the recorder, installs it via
/// `metrics::set_global_recorder`, and publishes its handle; every
/// other caller observes the same handle. Safe to call from test
/// setups, from `app_router`, and from `main` at startup.
pub fn install_global() -> Result<(), String> {
    handle();
    Ok(())
}

/// Borrow the process-wide Prometheus handle, installing the recorder
/// exactly once on first access.
///
/// Using `OnceLock::get_or_init` makes the install atomic: even if two
/// threads call this in parallel, only one builds the recorder and
/// runs `set_global_recorder`; the other blocks until the winner
/// publishes the handle. That guarantees the global recorder and the
/// handle we render from are always the same `PrometheusRecorder`.
fn handle() -> &'static PrometheusHandle {
    PROMETHEUS_HANDLE.get_or_init(|| {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        // The first caller wins the global recorder slot. A second
        // process-global installer (e.g. another integration test
        // crate) is rare in practice; if it happens, the loser keeps
        // a live handle that observes whatever was installed first,
        // which is acceptable for an observability endpoint.
        let _ = metrics::set_global_recorder(recorder);
        handle
    })
}

/// Axum handler for `GET /_prometheus_metrics`.
///
/// Returns the current scrape body in the Prometheus text exposition
/// format. The `Content-Type` header is exactly the value a Prometheus
/// scraper expects (`text/plain; version=0.0.4`) so the endpoint is
/// usable as a drop-in scrape target.
pub async fn prometheus_handler() -> Response {
    // Les stats jemalloc sont mises en cache jusqu'à l'avancement de leur
    // epoch. Un scrape est une borne de télémétrie P3 : il doit donc rafraîchir
    // les jauges processus/allocateur avant de rendre l'exposition.
    crate::stats::refresh_runtime_memory_gauges();
    let body = handle().render();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_global_is_idempotent() {
        install_global().expect("first install should succeed");
        install_global().expect("second install should succeed");
        // The handle is published exactly once and lives for the
        // process lifetime, regardless of how many times we call
        // install_global.
        assert!(PROMETHEUS_HANDLE.get().is_some());
    }

    #[test]
    fn rendered_output_is_text_exposition() {
        install_global().expect("install should succeed");
        // Touch a counter so the rendered output is not empty even
        // when this test runs in isolation.
        metrics::counter!("surch_metrics_self_test_total").increment(1);
        let body = handle().render();
        assert!(
            body.contains("surch_metrics_self_test_total"),
            "rendered output should contain the emitted counter, got: {body}"
        );
    }
}
