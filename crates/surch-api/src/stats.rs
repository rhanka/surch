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
    let (state_docs_overhead, state_id_maps) =
        state.index_state_memory_bytes(index).unwrap_or((0, 0));
    let disk_segment_bytes = state.index_disk_segment_bytes(index).unwrap_or(0);
    let disk_segment_peak_bytes = state.index_disk_segment_peak_bytes(index).unwrap_or(0);
    set_gauges(
        index,
        doc_count,
        &usage,
        state_docs_overhead,
        state_id_maps,
        disk_segment_bytes,
        disk_segment_peak_bytes,
    );
    refresh_process_memory_gauges();
}

/// Process-wide RSS accounting (#17b). Reads `/proc/self/status` to
/// surface VmRSS / VmAnon / VmData / VmHWM as Prometheus gauges. Combined
/// with the per-index `surch_index_*` gauges this isolates the gap
/// between "what Surch thinks it stores" (~2.8 GiB after inc1) and "what
/// the kernel reports resident" (~8.0 GiB). Zero allocations on the
/// scrape path — the file is short and parsed line by line. Linux-only.
#[cfg(target_os = "linux")]
fn refresh_process_memory_gauges() {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return;
    };
    for line in status.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        // `rest` looks like "    123456 kB"; jemalloc and the kernel both
        // expose this field in kibibytes. `split_whitespace` already skips
        // the leading run of whitespace, no trim needed.
        let kib_str = rest.split_whitespace().next().unwrap_or("0");
        let Ok(kib) = kib_str.parse::<u64>() else {
            continue;
        };
        let bytes = kib.saturating_mul(1024) as f64;
        match key {
            "VmRSS" => metrics::gauge!("surch_process_rss_bytes").set(bytes),
            "VmHWM" => metrics::gauge!("surch_process_rss_peak_bytes").set(bytes),
            "VmData" => metrics::gauge!("surch_process_data_bytes").set(bytes),
            "RssAnon" => metrics::gauge!("surch_process_rss_anon_bytes").set(bytes),
            "RssFile" => metrics::gauge!("surch_process_rss_file_bytes").set(bytes),
            "VmSize" => metrics::gauge!("surch_process_vsize_bytes").set(bytes),
            _ => {}
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn refresh_process_memory_gauges() {}

/// Drop the gauges for `index`. Called when an index is deleted so the
/// scrape body does not keep advertising stale totals.
pub fn clear_memory_gauges(index: &str) {
    set_gauges(index, 0, &MemoryUsage::default(), 0, 0, 0, 0);
}

fn set_gauges(
    index: &str,
    doc_count: u64,
    usage: &MemoryUsage,
    state_documents_overhead: u64,
    state_id_maps: u64,
    disk_segment_bytes: u64,
    disk_segment_peak_bytes: u64,
) {
    let label = [("index", index.to_owned())];
    // P1 mmap M1 + axe disque #19 : taille on-disk effective du segment
    // `source.dat`. `_bytes` est la mesure instantanée (0 après refresh
    // car `compact_after_refresh` truncate). `_peak_bytes` retient le
    // pic depuis la creation — c'est la vraie mesure pour le scoreboard
    // axe disque (puisque le scrape arrive APRES `_refresh`).
    metrics::gauge!("surch_index_disk_segment_bytes", &label).set(disk_segment_bytes as f64);
    metrics::gauge!("surch_index_disk_segment_peak_bytes", &label)
        .set(disk_segment_peak_bytes as f64);
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
    // #17c: capacity slack on per-term Vec<Posting>/Vec<u32> — bytes allocated
    // but unused after the FST build (size-class rounding).
    metrics::gauge!("surch_index_postings_capacity_slack_bytes", &label)
        .set(usage.postings_capacity_slack_bytes as f64);
    // #17c walker complet : PostingsBuilder retenu Lot 1.5. Premier suspect
    // du gap heap ~4 GiB inexpliqué sur deces 1.36M (cf scoreboard
    // 2026-06-10-mesured.md). Walk les BTreeMap imbriqués + Vec capacity.
    metrics::gauge!("surch_index_postings_builder_bytes", &label)
        .set(usage.postings_builder_bytes as f64);
    metrics::gauge!("surch_index_total_bytes", &label).set(usage.total_bytes() as f64);
    metrics::gauge!("surch_index_doc_count", &label).set(doc_count as f64);
    // #17b: api-side state overhead — the `documents` BTreeMap node + Arc
    // header + key strings, and the id maps. The _source payload is already
    // reported via `surch_index_stored_fields_bytes`, so this gauge is the
    // ON-TOP-OF cost.
    metrics::gauge!("surch_state_documents_overhead_bytes", &label)
        .set(state_documents_overhead as f64);
    metrics::gauge!("surch_state_id_maps_bytes", &label).set(state_id_maps as f64);
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
    // #17c: capacity slack on Vec<Posting>/Vec<u32> per-term channels.
    pub postings_capacity_slack_bytes: u64,
    // #17c walker complet: PostingsBuilder retenu Lot 1.5.
    pub postings_builder_bytes: u64,
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
            postings_capacity_slack_bytes: value.postings_capacity_slack_bytes,
            postings_builder_bytes: value.postings_builder_bytes,
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
