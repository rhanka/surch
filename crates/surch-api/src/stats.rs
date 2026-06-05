//! `GET /_surch/stats` — RAM accounting endpoint plus the Prometheus
//! gauges that mirror it.
//!
//! Surch keeps every index in memory; sizing a cluster (notably the
//! matchID INSEE indexer with ~1.3 M docs) depends on knowing how
//! many bytes the postings, the prefix-postings side table, the
//! field-length stats, the term block metas, and the stored `_source`
//! payloads consume.
//!
//! The numbers are refreshed at indexing time only (`_bulk`, `_doc`,
//! delete, snapshot import) so the hot search path is untouched.

use std::collections::BTreeMap;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surch_index::memory::MemoryUsage;

use crate::state::AppState;

/// Prometheus gauge: per-field-component byte counts and total. Refreshed
/// after every write to the relevant index. The `index` label is bounded
/// by the number of physical indices in the cluster; there is no
/// per-document or per-query cardinality blow-up.
pub fn refresh_memory_gauges(state: &AppState, index: &str) {
    let usage = state.index_memory_usage(index).unwrap_or_default();
    let doc_count = state.index_doc_count(index).unwrap_or(0);
    set_gauges(index, doc_count, &usage);
}

/// Drop the gauges for `index`. Called when an index is deleted so the
/// scrape body does not keep advertising stale totals.
pub fn clear_memory_gauges(index: &str) {
    set_gauges(index, 0, &MemoryUsage::default());
}

fn set_gauges(index: &str, doc_count: u64, usage: &MemoryUsage) {
    let label = [("index", index.to_owned())];
    metrics::gauge!("surch_index_postings_bytes", &label).set(usage.postings_bytes as f64);
    metrics::gauge!("surch_index_prefix_postings_bytes", &label)
        .set(usage.prefix_postings_bytes as f64);
    metrics::gauge!("surch_index_stored_fields_bytes", &label)
        .set(usage.stored_fields_bytes as f64);
    metrics::gauge!("surch_index_field_stats_bytes", &label).set(usage.field_stats_bytes as f64);
    metrics::gauge!("surch_index_term_stats_bytes", &label).set(usage.term_stats_bytes as f64);
    // #17: break out the previously-unaccounted RSS portion.
    metrics::gauge!("surch_index_fst_bytes", &label).set(usage.fst_bytes as f64);
    metrics::gauge!("surch_index_roaring_bytes", &label).set(usage.roaring_bytes as f64);
    metrics::gauge!("surch_index_block_metas_bytes", &label).set(usage.block_metas_bytes as f64);
    metrics::gauge!("surch_index_total_bytes", &label).set(usage.total_bytes() as f64);
    metrics::gauge!("surch_index_doc_count", &label).set(doc_count as f64);
}

/// `?index=<name>` query string for `GET /_surch/stats`.
#[derive(Debug, Deserialize, Default)]
pub struct StatsParams {
    /// Optional filter; when supplied, the response only contains the
    /// named index. An unknown name returns an empty `indices` object.
    pub index: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IndexStats {
    pub doc_count: u64,
    pub memory: MemoryReport,
}

#[derive(Debug, Serialize, Default)]
pub struct MemoryReport {
    pub postings_bytes: u64,
    pub prefix_postings_bytes: u64,
    pub stored_fields_bytes: u64,
    pub field_stats_bytes: u64,
    pub term_stats_bytes: u64,
    // #17: previously-unaccounted RSS components.
    pub fst_bytes: u64,
    pub roaring_bytes: u64,
    pub block_metas_bytes: u64,
    pub total_bytes: u64,
}

impl From<MemoryUsage> for MemoryReport {
    fn from(value: MemoryUsage) -> Self {
        Self {
            postings_bytes: value.postings_bytes,
            prefix_postings_bytes: value.prefix_postings_bytes,
            stored_fields_bytes: value.stored_fields_bytes,
            field_stats_bytes: value.field_stats_bytes,
            term_stats_bytes: value.term_stats_bytes,
            fst_bytes: value.fst_bytes,
            roaring_bytes: value.roaring_bytes,
            block_metas_bytes: value.block_metas_bytes,
            total_bytes: value.total_bytes(),
        }
    }
}

/// Axum handler for `GET /_surch/stats[?index=<name>]`.
pub async fn stats_handler(
    State(state): State<AppState>,
    Query(params): Query<StatsParams>,
) -> impl IntoResponse {
    let names: Vec<String> = match params.index {
        Some(name) if state.index_exists(&name) => vec![name],
        Some(_) => Vec::new(),
        None => state.index_names(),
    };

    let mut indices: BTreeMap<String, Value> = BTreeMap::new();
    let mut grand_total: u64 = 0;

    for name in names {
        let usage = state.index_memory_usage(&name).unwrap_or_default();
        let doc_count = state.index_doc_count(&name).unwrap_or(0);
        let report = MemoryReport::from(usage);
        grand_total = grand_total.saturating_add(report.total_bytes);
        indices.insert(
            name,
            json!({
                "doc_count": doc_count,
                "memory": report,
            }),
        );
    }

    let body = json!({
        "indices": indices,
        "total_bytes": grand_total,
    });

    (StatusCode::OK, Json(body))
}
