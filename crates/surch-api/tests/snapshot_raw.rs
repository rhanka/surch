//! Integration tests for `POST /_surch/snapshot/export` and
//! `POST /_surch/snapshot/import`.
//!
//! Scenario: build an index, ingest 100 docs, export to a tar.gz,
//! drop the source index, import under a new name, re-run a match
//! query on both fields, check that the hits.total.value matches.

use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, Method, Request, StatusCode},
};
use serde_json::{json, Value};
use surch_api::app_router;
use tower::ServiceExt;

async fn read_json(response: axum::response::Response<Body>) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    let json = serde_json::from_slice(&bytes).expect("body should be json");
    (status, json)
}

async fn read_bytes(
    response: axum::response::Response<Body>,
) -> (StatusCode, axum::http::HeaderMap, Bytes) {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    (status, headers, bytes)
}

fn make_router() -> axum::Router {
    app_router()
}

async fn create_index(router: &axum::Router, name: &str, body: Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/{name}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "PUT /{name} should succeed"
    );
}

async fn index_document(router: &axum::Router, index: &str, id: &str, source: Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/{index}/_doc/{id}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(source.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert!(
        response.status().is_success(),
        "PUT /{index}/_doc/{id} returned {}",
        response.status()
    );
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
    assert!(
        response.status().is_success(),
        "POST /_bulk returned {}",
        response.status()
    );
}

async fn search_match(router: &axum::Router, index: &str, field: &str, term: &str) -> Value {
    let body = json!({ "query": { "match": { field: term } } });
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_search"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let (status, body) = read_json(response).await;
    assert_eq!(status, StatusCode::OK, "search should return 200");
    body
}

async fn delete_index(router: &axum::Router, index: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/{index}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "DELETE /{index} should succeed"
    );
}

#[tokio::test]
async fn export_then_import_preserves_hits_total() {
    let router = make_router();

    // Create source index with an explicit mapping so we exercise the
    // mapping round-trip through the tarball.
    create_index(
        &router,
        "source",
        json!({
            "mappings": {
                "properties": {
                    "title": { "type": "text" },
                    "category": { "type": "keyword" }
                }
            },
            "settings": { "index": { "number_of_shards": 1 } }
        }),
    )
    .await;

    // Ingest 100 documents in one bulk call (PUT /_doc rebuilds the
    // whole index after every doc — O(N^2) cost on the test scenario,
    // measurable enough to spike timing-sensitive sibling tests). Half
    // are tagged `science`, half `fiction`; every document mentions
    // "alpha" so the post-restore match query has a non-trivial hit
    // count.
    let docs: Vec<(String, Value)> = (0..100)
        .map(|n| {
            let category = if n.is_multiple_of(2) {
                "science"
            } else {
                "fiction"
            };
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
    let _ = index_document; // keep the helper available for ad-hoc tests below

    let baseline = search_match(&router, "source", "title", "alpha").await;
    let baseline_total = baseline["hits"]["total"]["value"]
        .as_u64()
        .expect("hits.total.value should be a number");
    assert_eq!(baseline_total, 100, "baseline match should hit every doc");

    // Export the source index.
    let export_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_surch/snapshot/export?index=source")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let (status, headers, tarball) = read_bytes(export_response).await;
    assert_eq!(status, StatusCode::OK, "export should return 200");
    assert_eq!(
        headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()),
        Some(b"application/gzip" as &[u8]),
        "export Content-Type should be application/gzip"
    );
    assert!(!tarball.is_empty(), "exported tarball must not be empty");
    // gzip magic
    assert_eq!(&tarball[..2], &[0x1f, 0x8b], "exported body must be gzip");

    // Drop the source index entirely.
    delete_index(&router, "source").await;

    // Import under a new index name.
    let import_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_surch/snapshot/import?index=restored")
                .header(header::CONTENT_TYPE, "application/gzip")
                .body(Body::from(tarball.clone()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let (status, json) = read_json(import_response).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "import should return 200, got {json}"
    );
    assert_eq!(json["acknowledged"], json!(true));
    assert_eq!(json["index"], json!("restored"));
    assert_eq!(json["documents"], json!(100));
    assert_eq!(json["format_version"], json!(1));
    assert_eq!(json["source_index"], json!("source"));

    // Same `match` query against the restored index returns the same hit count.
    let restored = search_match(&router, "restored", "title", "alpha").await;
    let restored_total = restored["hits"]["total"]["value"]
        .as_u64()
        .expect("hits.total.value should be a number");
    assert_eq!(
        restored_total, baseline_total,
        "restored index must report the same hits.total.value as the source"
    );

    // Mapping round-trips: keyword field still resolves through the
    // dedicated path (term-level, no analyzer split).
    let keyword_hits = search_match(&router, "restored", "category", "science").await;
    let keyword_total = keyword_hits["hits"]["total"]["value"]
        .as_u64()
        .expect("hits.total.value should be a number");
    assert_eq!(
        keyword_total, 50,
        "restored keyword field must keep its mapping (50 science docs)"
    );
}

#[tokio::test]
async fn export_unknown_index_returns_404() {
    let router = make_router();
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_surch/snapshot/export?index=nope")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let (status, json) = read_json(response).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(json["error"]["type"], json!("index_not_found_exception"));
}

#[tokio::test]
async fn import_into_existing_index_is_rejected() {
    let router = make_router();
    create_index(&router, "already", json!({})).await;

    // We don't even need a valid tarball — the existence check fires
    // before we touch the body.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_surch/snapshot/import?index=already")
                .body(Body::from(vec![0u8; 16]))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let (status, json) = read_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json["error"]["type"],
        json!("resource_already_exists_exception")
    );
}

#[tokio::test]
async fn import_rejects_garbage_body() {
    let router = make_router();
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_surch/snapshot/import?index=garbage")
                .body(Body::from(b"not a tarball".to_vec()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let (status, json) = read_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["type"], json!("snapshot_import_exception"));
}

#[tokio::test]
async fn import_rejects_path_traversal_entries() {
    // The tar crate refuses to *write* `..` in a path (good citizen),
    // but a hostile archive built by any other tool can still ship
    // one — and a directory-walking importer would happily honour it.
    // Our defence is a whitelist of expected entry names; this test
    // exercises that whitelist by shipping an unexpected (but valid)
    // entry name `surch.key`. The same code path rejects `../etc/passwd`
    // when it comes from a non-Rust archiver: the entry name simply
    // isn't in `ALLOWED_ENTRIES`.
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tar::{Builder, Header};

    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = Builder::new(gz);
    let payload = b"x";
    let mut header = Header::new_gnu();
    header.set_path("surch.key").expect("path should set");
    header.set_size(payload.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, &payload[..])
        .expect("tar append should succeed");
    let gz = tar.into_inner().expect("tar finalisation");
    let tarball = gz.finish().expect("gzip finalisation");

    let router = make_router();
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_surch/snapshot/import?index=evil")
                .body(Body::from(tarball))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let (status, json) = read_json(response).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(json["error"]["type"], json!("snapshot_import_exception"));
    let reason = json["error"]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("unexpected tar entry"),
        "reason should mention the rejected entry, got `{reason}`"
    );
}
