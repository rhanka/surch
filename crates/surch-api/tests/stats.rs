//! Integration tests for `GET /_surch/stats` and the matching
//! `surch_index_*_bytes` Prometheus gauges.
//!
//! These tests exercise the wiring end-to-end: a write via `_doc`
//! or `_bulk` should refresh the API-side gauges, and the JSON
//! endpoint should report a coherent breakdown (every component
//! non-negative, `total_bytes == sum(components)`).

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::Value;
use surch_api::app_router;
use tower::ServiceExt;

async fn fetch_stats(router: &axum::Router, query: &str) -> (StatusCode, Value) {
    let uri = if query.is_empty() {
        "/_surch/stats".to_owned()
    } else {
        format!("/_surch/stats?{query}")
    };
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let status = response.status();
    let body_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should be readable");
    let body: Value =
        serde_json::from_slice(&body_bytes).expect("stats body should be JSON: {body_bytes:?}");
    (status, body)
}

async fn index_one(router: &axum::Router, target: &str, id: &str, source: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{target}/_doc/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(source.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::CREATED);
}

/// An index with no documents reports zero memory. The endpoint still
/// exists and the response shape is well-formed.
#[tokio::test]
async fn stats_returns_zero_for_empty_index() {
    let router = app_router();

    // Create an empty index via PUT.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/empty")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert!(
        response.status().is_success(),
        "PUT /empty should succeed, got {}",
        response.status(),
    );

    let (status, body) = fetch_stats(&router, "index=empty").await;
    assert_eq!(status, StatusCode::OK);

    let indices = body
        .get("indices")
        .and_then(Value::as_object)
        .expect("response should carry an indices object");
    let entry = indices
        .get("empty")
        .expect("empty index should appear in stats");
    let memory = entry.get("memory").expect("memory object");
    assert_eq!(
        memory.get("postings_bytes").and_then(Value::as_u64),
        Some(0),
        "empty index postings_bytes should be 0",
    );
    assert_eq!(
        memory.get("total_bytes").and_then(Value::as_u64),
        Some(0),
        "empty index total_bytes should be 0",
    );
    assert_eq!(
        entry.get("doc_count").and_then(Value::as_u64),
        Some(0),
        "empty index doc_count should be 0",
    );
}

/// After indexing 100 documents, `stats` reports non-zero postings AND
/// `total_bytes > stored_fields_bytes alone` (i.e. the inverted index
/// adds bytes on top of the stored payloads).
#[tokio::test]
async fn stats_grows_with_documents() {
    let router = app_router();

    for i in 0..100 {
        index_one(
            &router,
            "people",
            &format!("p{i}"),
            &format!(
                r#"{{"name":"dupont martin {i}","city":"paris","note":"customer #{i} from the rue de la paix"}}"#
            ),
        )
        .await;
    }

    let (status, body) = fetch_stats(&router, "index=people").await;
    assert_eq!(status, StatusCode::OK);

    let memory = body
        .pointer("/indices/people/memory")
        .expect("memory report for 'people'");
    let postings = memory
        .get("postings_bytes")
        .and_then(Value::as_u64)
        .expect("postings_bytes");
    let stored = memory
        .get("stored_fields_bytes")
        .and_then(Value::as_u64)
        .expect("stored_fields_bytes");
    let term_stats = memory
        .get("term_stats_bytes")
        .and_then(Value::as_u64)
        .expect("term_stats_bytes");
    let field_stats = memory
        .get("field_stats_bytes")
        .and_then(Value::as_u64)
        .expect("field_stats_bytes");
    let prefix = memory
        .get("prefix_postings_bytes")
        .and_then(Value::as_u64)
        .expect("prefix_postings_bytes");
    let total = memory
        .get("total_bytes")
        .and_then(Value::as_u64)
        .expect("total_bytes");

    assert!(postings > 0, "postings_bytes should grow with 100 docs");
    // P1 mmap M1 : `stored_fields_bytes` est une gauge RAM. Les blobs
    // `OnDisk` (hot path bulk) retournent 0 — leurs bytes vivent dans
    // le segment file-backed via mmap, pas dans le heap. La gauge ne
    // devient positive qu'après `compact_after_refresh` qui convertit
    // certains blobs en `Compressed` (in-RAM). Le test ne fait pas de
    // `_refresh` ici, donc l'assertion historique > 0 n'est plus tenue ;
    // on borne juste à ≥ 0 (informatif).
    let _ = stored;
    assert!(
        field_stats > 0,
        "field_stats_bytes should grow with 100 docs"
    );
    assert!(
        total >= postings + stored + term_stats + field_stats + prefix,
        "total_bytes should cover every component; got total={total}, components sum={}",
        postings + stored + term_stats + field_stats + prefix,
    );
    assert!(
        total > stored,
        "the inverted index should add bytes on top of stored fields; total={total} stored={stored}",
    );

    let doc_count = body
        .pointer("/indices/people/doc_count")
        .and_then(Value::as_u64)
        .expect("doc_count");
    assert_eq!(doc_count, 100);

    let grand_total = body
        .get("total_bytes")
        .and_then(Value::as_u64)
        .expect("top-level total_bytes");
    assert!(grand_total >= total);
}

/// Plan segments S5 (`docs/paper/design-segments-pic-borne-2026-07-05.md`
/// §"Dimensionnement S5"): `postings_directory_bytes` — the per-term
/// CSR/directory metadata (`offsets`/`block_offsets`/`segment_descriptors`/
/// `block_directory`/`block_dir_offsets`) identified as the strongest
/// candidate for the ~295 MiB gap between jemalloc `allocated` and the sum
/// of every other gauge measured on the 1,36 M matchID corpus — must be
/// non-zero once the index carries a meaningful number of DISTINCT terms,
/// and must be part of `total_bytes` (end-to-end wiring: `state.rs` ->
/// `surch-index::memory` -> this endpoint's JSON).
#[tokio::test]
async fn postings_directory_bytes_grows_with_distinct_term_count() {
    let router = app_router();

    // 200 docs, each contributing its OWN distinct term — mirrors, at test
    // scale, the high term cardinality an `edge_ngram`/`autocomplete`
    // analyzer or an `index_prefixes` field produces on the real corpus.
    for i in 0..200 {
        index_one(
            &router,
            "terms",
            &format!("d{i}"),
            &format!(r#"{{"tag":"uniqueterm{i}"}}"#),
        )
        .await;
    }

    let (status, body) = fetch_stats(&router, "index=terms").await;
    assert_eq!(status, StatusCode::OK);
    let memory = body
        .pointer("/indices/terms/memory")
        .expect("memory report for 'terms'");
    let directory = memory
        .get("postings_directory_bytes")
        .and_then(Value::as_u64)
        .expect("postings_directory_bytes");
    let total = memory
        .get("total_bytes")
        .and_then(Value::as_u64)
        .expect("total_bytes");
    assert!(
        directory > 0,
        "200 distinct terms should carry non-zero per-term CSR/directory metadata"
    );
    assert!(
        directory <= total,
        "postings_directory_bytes must be part of total_bytes: directory={directory} total={total}"
    );
}

/// Unknown index → 200 with an empty `indices` object. We deliberately
/// chose this shape (rather than 404) so dashboards that always scrape
/// `?index=<name>` stay green when an index has not been created yet.
#[tokio::test]
async fn stats_unknown_index_is_empty() {
    let router = app_router();
    let (status, body) = fetch_stats(&router, "index=does_not_exist").await;
    assert_eq!(status, StatusCode::OK);
    let indices = body
        .get("indices")
        .and_then(Value::as_object)
        .expect("indices object");
    assert!(
        indices.is_empty(),
        "unknown index should yield an empty `indices` object, got {indices:?}",
    );
    assert_eq!(body.get("total_bytes").and_then(Value::as_u64), Some(0));
}

/// The Prometheus scrape body advertises the per-index gauges as soon
/// as a write touches the index. This guards the wiring between
/// `apply_document_writes` and `stats::refresh_memory_gauges`.
#[tokio::test]
async fn prometheus_exposes_memory_gauges_after_indexing() {
    let router = app_router();
    index_one(
        &router,
        "products",
        "sku-1",
        r#"{"name":"a moderately long description for one product"}"#,
    )
    .await;

    let scrape = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_prometheus_metrics")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(scrape.status(), StatusCode::OK);

    let body_bytes = to_bytes(scrape.into_body(), usize::MAX)
        .await
        .expect("scrape body should be readable");
    let body = String::from_utf8(body_bytes.to_vec()).expect("UTF-8 scrape body");

    for metric in [
        "surch_index_postings_bytes",
        "surch_index_stored_fields_bytes",
        "surch_index_field_stats_bytes",
        "surch_index_total_bytes",
        "surch_index_doc_count",
        // Plan segments S5: per-term CSR/directory metadata gauge (the
        // identified candidate for the ~295 MiB non-gaugé gap — see
        // `surch_index::memory::MemoryUsage::postings_directory_bytes`).
        "surch_index_postings_directory_bytes",
    ] {
        assert!(
            body.contains(metric),
            "scrape body should mention `{metric}`, got:\n{body}",
        );
    }
    assert!(
        body.contains("index=\"products\""),
        "scrape body should label gauges with the index name, got:\n{body}",
    );
}
