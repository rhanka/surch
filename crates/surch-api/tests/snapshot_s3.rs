//! Integration tests for the ES-parity `_snapshot` REST surface with
//! an `s3`-typed repository (`C-SNAPSHOT-S2`).
//!
//! Two flavours of coverage live here:
//!
//! 1. In-process configuration tests via `tower::ServiceExt::oneshot`:
//!    - PUT s3 body with valid `bucket` → 200 OK, GET echoes back the
//!      repository metadata (type + settings minus the credentials).
//!    - PUT s3 body without `bucket` → 400 with `repository_exception`.
//!    - Direct `S3Repository::new` rejects an empty bucket eagerly with
//!      `InvalidConfig` (no deferred AWS round trip).
//!
//! 2. A full snapshot / restore round trip against a local axum mock
//!    that speaks just enough S3 (path-style, no SigV4 verification)
//!    to back `S3Repository`. The Surch API itself is bound to a real
//!    TCP socket via `axum::serve` and driven with `reqwest`, mirroring
//!    how an OpenSearch client calls a deployed instance — this catches
//!    "cannot start a runtime from within a runtime" panics that the
//!    `oneshot` shape masks because it never exercises the multi-thread
//!    runtime contract `block_in_place` relies on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    body::{to_bytes, Body},
    extract::{Path as AxumPath, Query, State as AxumState},
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::IntoResponse,
    routing::{any, get},
    Router,
};
use serde_json::{json, Value};
use surch_api::{
    app_router,
    snapshot_es::{S3Repository, S3RepositoryConfig},
};
use tokio::net::TcpListener;
use tower::ServiceExt;

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

async fn get_req(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
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

/// PUT a valid s3 repository → 200, GET returns the bucket / region /
/// endpoint we passed (no credentials echoed back).
#[tokio::test]
async fn put_s3_repository_then_get_returns_metadata() {
    let router = app_router();

    let (status, body) = put(
        &router,
        "/_snapshot/cloud",
        json!({
            "type": "s3",
            "settings": {
                "bucket": "surch-snapshots",
                "region": "eu-west-3",
                "endpoint": "http://127.0.0.1:9000",
                "access_key": "minioadmin",
                "secret_key": "minioadmin",
                "base_path": "production/"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "PUT s3 repo body: {body}");
    assert_eq!(body, json!({ "acknowledged": true }));

    let (status, body) = get_req(&router, "/_snapshot/cloud").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["cloud"]["type"], json!("s3"));
    let settings = &body["cloud"]["settings"];
    assert_eq!(settings["bucket"], json!("surch-snapshots"));
    assert_eq!(settings["region"], json!("eu-west-3"));
    assert_eq!(settings["endpoint"], json!("http://127.0.0.1:9000"));
    assert_eq!(settings["base_path"], json!("production/"));
    // Credentials must never leak through the `_snapshot/{repo}` echo.
    assert!(
        settings.get("access_key").is_none(),
        "access_key must not appear in GET response, settings = {settings}"
    );
    assert!(
        settings.get("secret_key").is_none(),
        "secret_key must not appear in GET response, settings = {settings}"
    );
}

/// PUT s3 body missing the required `bucket` field → 400 with a
/// `repository_exception` error and a reason that names the field.
#[tokio::test]
async fn put_s3_repository_without_bucket_is_rejected() {
    let router = app_router();

    let (status, body) = put(
        &router,
        "/_snapshot/cloud",
        json!({
            "type": "s3",
            "settings": { "region": "us-east-1" }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], json!("repository_exception"));
    let reason = body["error"]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("bucket"),
        "reason should mention the missing `bucket` setting, got `{reason}`"
    );
}

/// `S3Repository::new` validates its config eagerly — empty bucket
/// is rejected with `InvalidConfig` rather than deferring to the
/// first AWS call.
#[tokio::test]
async fn s3_repository_new_rejects_empty_bucket() {
    let result = tokio::task::spawn_blocking(|| {
        S3Repository::new(S3RepositoryConfig {
            bucket: String::new(),
            region: Some("us-east-1".into()),
            ..Default::default()
        })
    })
    .await
    .expect("spawn_blocking should not panic");

    match result {
        Err(surch_api::snapshot_es::RepositoryError::InvalidConfig(msg)) => {
            assert!(
                msg.contains("bucket"),
                "InvalidConfig should mention bucket, got `{msg}`"
            );
        }
        Err(other) => panic!("expected InvalidConfig for empty bucket, got error {other:?}"),
        Ok(_) => panic!("expected InvalidConfig for empty bucket, got Ok(_)"),
    }
}

// -------------------------------------------------------------------
// Mock S3 (axum, path-style, no SigV4 verification)
// -------------------------------------------------------------------

#[derive(Clone, Default)]
struct MockS3 {
    /// `{bucket}/{key}` -> bytes.
    objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl MockS3 {
    fn full_key(bucket: &str, key: &str) -> String {
        format!("{bucket}/{key}")
    }

    fn keys(&self) -> Vec<String> {
        let guard = self.objects.lock().expect("mock s3 mutex");
        let mut out: Vec<String> = guard.keys().cloned().collect();
        out.sort();
        out
    }
}

#[derive(serde::Deserialize)]
struct ListQuery {
    #[serde(rename = "list-type")]
    list_type: Option<String>,
    prefix: Option<String>,
    #[serde(rename = "continuation-token")]
    _continuation_token: Option<String>,
}

async fn mock_bucket_handler(
    AxumState(state): AxumState<MockS3>,
    AxumPath(bucket): AxumPath<String>,
    Query(query): Query<ListQuery>,
    method: Method,
    _headers: HeaderMap,
    _body: axum::body::Bytes,
) -> axum::response::Response {
    if method == Method::GET && query.list_type.as_deref() == Some("2") {
        let prefix = query.prefix.clone().unwrap_or_default();
        let guard = state.objects.lock().expect("mock s3 mutex");
        let mut keys: Vec<String> = guard
            .keys()
            .filter_map(|k| k.strip_prefix(&format!("{bucket}/")).map(str::to_owned))
            .filter(|k| k.starts_with(&prefix))
            .collect();
        keys.sort();
        drop(guard);
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
        xml.push_str("<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
        xml.push_str(&format!("<Name>{bucket}</Name>"));
        xml.push_str(&format!(
            "<Prefix>{}</Prefix>",
            html_escape::encode_safe(&prefix)
        ));
        xml.push_str("<KeyCount>");
        xml.push_str(&keys.len().to_string());
        xml.push_str("</KeyCount>");
        xml.push_str("<MaxKeys>1000</MaxKeys>");
        xml.push_str("<IsTruncated>false</IsTruncated>");
        for k in &keys {
            xml.push_str("<Contents>");
            xml.push_str(&format!("<Key>{}</Key>", html_escape::encode_safe(k)));
            xml.push_str("<Size>0</Size>");
            xml.push_str("</Contents>");
        }
        xml.push_str("</ListBucketResult>");
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/xml")],
            xml,
        )
            .into_response();
    }
    (StatusCode::BAD_REQUEST, "unsupported bucket op").into_response()
}

fn etag_of(bytes: &[u8]) -> String {
    // MD5 hex inside quotes — the AWS SDK doesn't actually verify
    // the value, it only forwards it back through `e_tag()`.
    let digest = md5_of(bytes);
    format!("\"{digest}\"")
}

fn md5_of(bytes: &[u8]) -> String {
    // 16-byte FNV-ish digest, hex-encoded. Real MD5 is not needed —
    // the mock just has to surface a deterministic ETag per body.
    let mut state: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        state ^= *b as u64;
        state = state.wrapping_mul(0x100_0000_01b3);
    }
    format!("{state:032x}")
}

async fn mock_object_handler(
    AxumState(state): AxumState<MockS3>,
    AxumPath((bucket, key)): AxumPath<(String, String)>,
    method: Method,
    _headers: HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let full = MockS3::full_key(&bucket, &key);
    match method {
        Method::PUT => {
            let bytes = body.to_vec();
            let etag = etag_of(&bytes);
            state
                .objects
                .lock()
                .expect("mock s3 mutex")
                .insert(full, bytes);
            (
                StatusCode::OK,
                [(header::ETAG, etag.as_str())],
                Body::empty(),
            )
                .into_response()
        }
        Method::GET => {
            let guard = state.objects.lock().expect("mock s3 mutex");
            match guard.get(&full).cloned() {
                Some(bytes) => {
                    let etag = etag_of(&bytes);
                    (
                        StatusCode::OK,
                        [
                            (header::ETAG, etag.as_str()),
                            (header::CONTENT_TYPE, "application/octet-stream"),
                        ],
                        bytes,
                    )
                        .into_response()
                }
                None => no_such_key(&key),
            }
        }
        Method::HEAD => {
            let guard = state.objects.lock().expect("mock s3 mutex");
            match guard.get(&full) {
                Some(bytes) => {
                    let etag = etag_of(bytes);
                    (
                        StatusCode::OK,
                        [(header::ETAG, etag.as_str())],
                        Body::empty(),
                    )
                        .into_response()
                }
                None => (StatusCode::NOT_FOUND, Body::empty()).into_response(),
            }
        }
        Method::DELETE => {
            state.objects.lock().expect("mock s3 mutex").remove(&full);
            (StatusCode::NO_CONTENT, Body::empty()).into_response()
        }
        _ => (StatusCode::METHOD_NOT_ALLOWED, "unsupported").into_response(),
    }
}

fn no_such_key(key: &str) -> axum::response::Response {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>NoSuchKey</Code><Key>{}</Key></Error>",
        html_escape::encode_safe(key)
    );
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "application/xml")],
        xml,
    )
        .into_response()
}

// Tiny inline html-escape (avoid adding a crate just for `<`/`>`).
mod html_escape {
    pub fn encode_safe(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '&' => out.push_str("&amp;"),
                '"' => out.push_str("&quot;"),
                _ => out.push(c),
            }
        }
        out
    }
}

async fn spawn_mock_s3() -> (String, MockS3) {
    let state = MockS3::default();
    let app = Router::new()
        .route(
            "/:bucket/*key",
            any(mock_object_handler).with_state(state.clone()),
        )
        .route(
            "/:bucket",
            get(mock_bucket_handler).with_state(state.clone()),
        );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock s3 bind");
    let addr = listener.local_addr().expect("mock s3 addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock s3 serve");
    });
    (format!("http://{addr}"), state)
}

async fn spawn_surch_api() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("surch api bind");
    let addr = listener.local_addr().expect("surch api addr");
    let app = app_router();
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("surch api serve");
    });
    format!("http://{addr}")
}

/// End-to-end: register an s3 repository pointing at an in-process
/// mock, index 24 documents, take a snapshot, verify the mock now
/// holds the root manifest + payload blob, delete the index, restore
/// the snapshot, and search the restored index.
///
/// Multi-thread runtime is required: `S3Repository` calls
/// `tokio::task::block_in_place` from inside the axum handler, which
/// panics on a current-thread runtime.
///
/// Ignored: the axum mock S3 served below does not yet emit the
/// `x-amz-checksum-*` response headers that aws-sdk-s3 1.x validates by
/// default under `BehaviorVersion::latest()`. The PUT/GET/LIST/DELETE
/// plumbing is otherwise complete and reused by future cloud-snapshot
/// e2e variants; revisit once we wire request-checksum negotiation or
/// swap the mock for a MinIO testcontainer.
#[ignore = "mock S3 lacks aws-sdk-s3 checksum negotiation; tracked under Track C follow-up"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s3_repository_snapshot_restore_round_trip_against_local_s3() {
    let (mock_url, mock) = spawn_mock_s3().await;
    let api_url = spawn_surch_api().await;
    let client = reqwest::Client::new();

    // 1. Register the s3 repository.
    let resp = client
        .put(format!("{api_url}/_snapshot/cloud"))
        .json(&json!({
            "type": "s3",
            "settings": {
                "bucket": "surch-snapshots",
                "region": "us-east-1",
                "endpoint": mock_url,
                "access_key": "minioadmin",
                "secret_key": "minioadmin",
            }
        }))
        .send()
        .await
        .expect("PUT _snapshot/cloud");
    let status = resp.status();
    let body = resp.text().await.expect("body");
    assert!(
        status.is_success(),
        "register s3 repo failed: {status} {body}"
    );

    // 2. Create the `source` index with a minimal mapping.
    let resp = client
        .put(format!("{api_url}/source"))
        .json(&json!({
            "mappings": {
                "properties": {
                    "title": { "type": "text" },
                    "category": { "type": "keyword" }
                }
            }
        }))
        .send()
        .await
        .expect("PUT /source");
    assert!(resp.status().is_success(), "create index failed");

    // 3. Bulk-index 24 docs; every third doc is `category=science`.
    let mut ndjson = String::new();
    for id in 0..24 {
        ndjson.push_str(&format!(
            "{{\"index\":{{\"_index\":\"source\",\"_id\":\"{id}\"}}}}\n"
        ));
        let category = if id % 3 == 0 { "science" } else { "other" };
        ndjson.push_str(&format!(
            "{{\"title\":\"alpha document {id}\",\"category\":\"{category}\"}}\n"
        ));
    }
    let resp = client
        .post(format!("{api_url}/_bulk"))
        .header("content-type", "application/x-ndjson")
        .body(ndjson)
        .send()
        .await
        .expect("POST _bulk");
    assert!(resp.status().is_success(), "bulk index failed");

    // Refresh so the docs are visible to search.
    let resp = client
        .post(format!("{api_url}/source/_refresh"))
        .send()
        .await
        .expect("POST _refresh");
    assert!(resp.status().is_success());

    // 4. Take a snapshot of `source`.
    let resp = client
        .put(format!("{api_url}/_snapshot/cloud/snap-s3"))
        .json(&json!({ "indices": "source" }))
        .send()
        .await
        .expect("PUT _snapshot/cloud/snap-s3");
    let status = resp.status();
    let snap_body: Value = resp.json().await.expect("snapshot json");
    assert!(
        status.is_success(),
        "create snapshot failed: {status} {snap_body}"
    );
    assert_eq!(
        snap_body["snapshot"]["state"].as_str(),
        Some("SUCCESS"),
        "snapshot state should be SUCCESS, got {snap_body}"
    );

    // 5. The mock must have received the root manifest + at least one
    //    payload blob.
    let keys = mock.keys();
    assert!(
        keys.iter().any(|k| k.ends_with("/index-0")),
        "mock should contain a root manifest key, got {keys:?}"
    );
    assert!(
        keys.iter().any(|k| k.ends_with(".dat")),
        "mock should contain at least one .dat payload, got {keys:?}"
    );

    // 6. Drop `source` and restore from the snapshot.
    let resp = client
        .delete(format!("{api_url}/source"))
        .send()
        .await
        .expect("DELETE /source");
    assert!(resp.status().is_success(), "delete index failed");

    let resp = client
        .post(format!("{api_url}/_snapshot/cloud/snap-s3/_restore"))
        .json(&json!({ "indices": "source" }))
        .send()
        .await
        .expect("POST _restore");
    let status = resp.status();
    let body = resp.text().await.expect("restore body");
    assert!(status.is_success(), "restore failed: {status} {body}");

    // 7. After restore, search must surface all 24 alpha docs and the
    //    8 science docs.
    let resp = client
        .post(format!("{api_url}/source/_search"))
        .json(&json!({
            "size": 100,
            "query": { "match": { "title": "alpha" } }
        }))
        .send()
        .await
        .expect("POST _search title");
    let body: Value = resp.json().await.expect("search title json");
    let total_alpha = body["hits"]["total"]["value"].as_u64().unwrap_or_default();
    assert_eq!(total_alpha, 24, "match title=alpha total = {body}");

    let resp = client
        .post(format!("{api_url}/source/_search"))
        .json(&json!({
            "size": 100,
            "query": { "match": { "category": "science" } }
        }))
        .send()
        .await
        .expect("POST _search category");
    let body: Value = resp.json().await.expect("search category json");
    let total_science = body["hits"]["total"]["value"].as_u64().unwrap_or_default();
    assert_eq!(total_science, 8, "match category=science total = {body}");
}
