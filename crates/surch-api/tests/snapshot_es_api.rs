//! Integration tests for the ES-parity `_snapshot` REST surface
//! (`C-SNAPSHOT-S1`).
//!
//! Every test drives the public HTTP router via `tower::ServiceExt`,
//! exactly as a real Elasticsearch client would. Repository storage
//! is the on-disk `FsRepository`; each test allocates a per-PID, per-
//! test temporary directory under `std::env::temp_dir()`.

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use serde_json::{json, Value};
use std::path::PathBuf;
use surch_api::app_router;
use tower::ServiceExt;

fn tempdir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "surch-snapshot-es-{}-{}-{}",
        label,
        std::process::id(),
        nanos,
    ));
    std::fs::create_dir_all(&dir).expect("temp dir creation");
    dir
}

async fn read_json(response: axum::response::Response<Body>) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            panic!(
                "body should be json (got {} bytes, err={err}, raw={:?})",
                bytes.len(),
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, json)
}

async fn put(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    read_json(response).await
}

async fn post(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    read_json(response).await
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
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
    read_json(response).await
}

async fn delete_(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(uri)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    read_json(response).await
}

async fn bulk_index(router: &axum::Router, index: &str, docs: &[(String, Value)]) {
    let mut ndjson = String::new();
    for (id, source) in docs {
        ndjson.push_str(
            &serde_json::to_string(&json!({
                "index": { "_index": index, "_id": id }
            }))
            .unwrap(),
        );
        ndjson.push('\n');
        ndjson.push_str(&source.to_string());
        ndjson.push('\n');
    }
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_bulk")
                .header(header::CONTENT_TYPE, "application/x-ndjson")
                .body(Body::from(ndjson))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert!(response.status().is_success());
}

#[tokio::test]
async fn put_fs_repository_then_get_returns_metadata() {
    let router = app_router();
    let dir = tempdir("put-fs");

    let (status, body) = put(
        &router,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": dir.display().to_string() }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT repo body: {body}");
    assert_eq!(body, json!({ "acknowledged": true }));

    let (status, body) = get(&router, "/_snapshot/local").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["local"]["type"], json!("fs"));
    assert_eq!(
        body["local"]["settings"]["location"],
        json!(dir.display().to_string())
    );
}

#[tokio::test]
async fn put_unknown_repository_type_is_rejected() {
    // `gcs` is intentionally unwired — `s3` (incl. R2 / MinIO / GCS
    // interop endpoint) is the only non-fs backend in C-SNAPSHOT-S2.
    let router = app_router();
    let (status, body) = put(
        &router,
        "/_snapshot/cloud",
        json!({
            "type": "gcs",
            "settings": { "bucket": "irrelevant" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], json!("repository_exception"));
    let reason = body["error"]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("gcs") && reason.contains("not supported"),
        "reason should mention unsupported type, got `{reason}`"
    );
}

#[tokio::test]
async fn snapshot_take_and_get_reports_success() {
    let router = app_router();
    let dir = tempdir("take");

    // Register repository.
    let (status, _) = put(
        &router,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": dir.display().to_string() }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Build a 100-doc index.
    let (status, _) = put(
        &router,
        "/source",
        json!({
            "mappings": {
                "properties": {
                    "title": { "type": "text" },
                    "category": { "type": "keyword" }
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let docs: Vec<(String, Value)> = (0..100)
        .map(|n| {
            let category = if n % 2 == 0 { "science" } else { "fiction" };
            (
                format!("doc-{n}"),
                json!({
                    "title": format!("alpha beta gamma {n}"),
                    "category": category,
                }),
            )
        })
        .collect();
    bulk_index(&router, "source", &docs).await;

    // Take the snapshot.
    let (status, body) = put(
        &router,
        "/_snapshot/local/snap-1",
        json!({ "indices": "source", "include_global_state": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "take body: {body}");
    assert_eq!(body["snapshot"]["snapshot"], json!("snap-1"));
    assert_eq!(body["snapshot"]["state"], json!("SUCCESS"));
    assert_eq!(body["snapshot"]["indices"], json!(["source"]));

    // Fetch metadata.
    let (status, body) = get(&router, "/_snapshot/local/snap-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["snapshots"][0]["snapshot"], json!("snap-1"));
    assert_eq!(body["snapshots"][0]["state"], json!("SUCCESS"));
}

#[tokio::test]
async fn snapshot_get_all_returns_every_snapshot_in_repository() {
    let router = app_router();
    let dir = tempdir("get-all");

    let (status, _) = put(
        &router,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": dir.display().to_string() }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = put(&router, "/idx", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    bulk_index(
        &router,
        "idx",
        &[("doc-1".into(), json!({ "title": "alpha", "category": "x" }))],
    )
    .await;

    let (status, body) = put(
        &router,
        "/_snapshot/local/snap-a",
        json!({ "indices": "idx" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "snap-a take body: {body}");

    let (status, body) = put(
        &router,
        "/_snapshot/local/snap-b",
        json!({ "indices": "idx" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "snap-b take body: {body}");

    let (status, body) = get(&router, "/_snapshot/local/_all").await;
    assert_eq!(status, StatusCode::OK, "get all body: {body}");
    assert_eq!(body["snapshots"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["snapshots"][0]["snapshot"], json!("snap-a"));
    assert_eq!(body["snapshots"][0]["state"], json!("SUCCESS"));
    assert_eq!(body["snapshots"][1]["snapshot"], json!("snap-b"));
    assert_eq!(body["snapshots"][1]["state"], json!("SUCCESS"));
}

#[tokio::test]
async fn snapshot_delete_then_get_returns_404() {
    let router = app_router();
    let dir = tempdir("delete");

    let (status, _) = put(
        &router,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": dir.display().to_string() }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = put(&router, "/idx", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    bulk_index(
        &router,
        "idx",
        &[("doc-1".into(), json!({ "title": "alpha", "category": "x" }))],
    )
    .await;

    let (status, _) = put(
        &router,
        "/_snapshot/local/snap-2",
        json!({ "indices": "idx" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = delete_(&router, "/_snapshot/local/snap-2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "acknowledged": true }));

    let (status, body) = get(&router, "/_snapshot/local/snap-2").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["type"], json!("snapshot_missing_exception"));
}

#[tokio::test]
async fn snapshot_restore_round_trip_brings_docs_back() {
    let router = app_router();
    let dir = tempdir("restore");

    let (status, _) = put(
        &router,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": dir.display().to_string() }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = put(
        &router,
        "/source",
        json!({
            "mappings": {
                "properties": {
                    "title": { "type": "text" },
                    "category": { "type": "keyword" }
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let docs: Vec<(String, Value)> = (0..100)
        .map(|n| {
            let category = if n % 2 == 0 { "science" } else { "fiction" };
            (
                format!("doc-{n}"),
                json!({
                    "title": format!("alpha beta gamma {n}"),
                    "category": category,
                }),
            )
        })
        .collect();
    bulk_index(&router, "source", &docs).await;

    let (status, _) = put(
        &router,
        "/_snapshot/local/snap-3",
        json!({ "indices": "source" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Drop the live index entirely.
    let (status, _) = delete_(&router, "/source").await;
    assert_eq!(status, StatusCode::OK);

    // Restore from the snapshot.
    let (status, body) = post(
        &router,
        "/_snapshot/local/snap-3/_restore",
        json!({ "indices": "source" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "restore body: {body}");
    assert_eq!(body["snapshot"]["indices"], json!(["source"]));

    // Same match query → same hit count.
    let (status, body) = post(
        &router,
        "/source/_search",
        json!({ "query": { "match": { "title": "alpha" } } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let total = body["hits"]["total"]["value"]
        .as_u64()
        .expect("hits.total.value should be number");
    assert_eq!(total, 100, "every doc must come back after restore");

    // Keyword mapping survives the round trip.
    let (status, body) = post(
        &router,
        "/source/_search",
        json!({ "query": { "match": { "category": "science" } } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let total = body["hits"]["total"]["value"]
        .as_u64()
        .expect("hits.total.value should be number");
    assert_eq!(total, 50, "keyword mapping must round-trip");
}

#[tokio::test]
async fn snapshot_restore_over_existing_index_returns_snapshot_exception() {
    let router = app_router();
    let dir = tempdir("restore-conflict");

    let (status, _) = put(
        &router,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": dir.display().to_string() }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = put(&router, "/source", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    bulk_index(
        &router,
        "source",
        &[("doc-1".into(), json!({ "title": "alpha" }))],
    )
    .await;

    let (status, _) = put(
        &router,
        "/_snapshot/local/snap-conflict",
        json!({ "indices": "source" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = post(
        &router,
        "/_snapshot/local/snap-conflict/_restore",
        json!({ "indices": "source" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], json!("snapshot_exception"));
    let reason = body["error"]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("cannot restore index [source]") && reason.contains("already exists"),
        "reason should explain the existing-index restore conflict, got `{reason}`"
    );
}

#[tokio::test]
async fn snapshot_take_when_repository_missing_returns_404() {
    let router = app_router();

    let (status, _) = put(&router, "/whatever", json!({})).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = put(
        &router,
        "/_snapshot/never_registered/snap-x",
        json!({ "indices": "whatever" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["type"], json!("repository_missing_exception"));
}

#[tokio::test]
async fn double_take_with_same_name_is_rejected() {
    let router = app_router();
    let dir = tempdir("double");

    let (status, _) = put(
        &router,
        "/_snapshot/local",
        json!({
            "type": "fs",
            "settings": { "location": dir.display().to_string() }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = put(&router, "/idx", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    bulk_index(
        &router,
        "idx",
        &[("doc-1".into(), json!({ "title": "alpha", "category": "x" }))],
    )
    .await;

    let (status, _) = put(
        &router,
        "/_snapshot/local/snap-dup",
        json!({ "indices": "idx" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = put(
        &router,
        "/_snapshot/local/snap-dup",
        json!({ "indices": "idx" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body["error"]["type"],
        json!("invalid_snapshot_name_exception")
    );
}
