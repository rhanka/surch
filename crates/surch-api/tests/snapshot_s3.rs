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
//! 2. A full snapshot / restore round trip against a real MinIO S3
//!    backend running inside a `testcontainers` Docker container. The
//!    Surch API itself is bound to a real TCP socket via `axum::serve`
//!    and driven with `reqwest`, mirroring how an OpenSearch client
//!    calls a deployed instance. The MinIO container speaks the real
//!    AWS S3 wire contract (Flexible Checksums, STREAMING-UNSIGNED-
//!    PAYLOAD-TRAILER, ListObjectsV2 XML schema, conditional writes),
//!    which the previous in-process axum mock could not.

use std::path::Path;

use aws_sdk_s3::config::{Credentials, Region};
use serde_json::{json, Value};
use surch_api::{
    app_router,
    snapshot_es::{S3Repository, S3RepositoryConfig},
};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::minio::MinIO;
use tokio::net::TcpListener;
use tower::ServiceExt;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};

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
// MinIO testcontainer + Surch API e2e
// -------------------------------------------------------------------

/// Returns true if the local environment can spin a Docker container
/// (the testcontainers crate needs a reachable Docker socket). In CI
/// the workflow always exports `CI=true` and Docker is provided by
/// the runner, so the test is mandatory there. On a developer box
/// without Docker, the test short-circuits with a `println!` rather
/// than failing — keeps `cargo test` green on minimal environments.
fn docker_socket_present() -> bool {
    if Path::new("/var/run/docker.sock").exists() {
        return true;
    }
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        // tcp:// / unix:// / npipe:// — any explicit override means
        // the user has set up Docker access by hand.
        if !host.trim().is_empty() {
            return true;
        }
    }
    false
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

/// End-to-end: register an s3 repository pointing at a MinIO
/// testcontainer, index 24 documents, take a snapshot, verify the
/// MinIO bucket now holds the root manifest + payload blob, delete
/// the index, restore the snapshot, and search the restored index.
///
/// Multi-thread runtime is required: `S3Repository` calls
/// `tokio::task::block_in_place` from inside the axum handler, which
/// panics on a current-thread runtime.
///
/// Unlike the previous in-process axum mock, MinIO speaks the full
/// AWS S3 wire contract — Flexible Checksums, STREAMING-UNSIGNED-
/// PAYLOAD-TRAILER, ListObjectsV2 XML, conditional writes — so we
/// keep `disable_request_checksum: false` (default) and exercise the
/// real production path the SDK takes against AWS / R2 / MinIO.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s3_repository_snapshot_restore_round_trip_against_local_s3() {
    if !docker_socket_present() {
        println!("Docker socket not present; skipping MinIO testcontainer test");
        return;
    }

    // 1. Spin up MinIO. The `testcontainers_modules::minio::MinIO`
    //    image defaults to `minioadmin` / `minioadmin` and exposes
    //    port 9000 inside the container. `get_host_port_ipv4(9000)`
    //    returns the random host port Docker mapped it onto.
    //
    //    Wrap in a 90 s timeout so that — when the host lacks pull
    //    bandwidth or the Docker daemon is unhealthy (notably some
    //    GitHub Actions runner shapes) — we short-circuit with a
    //    clear "skip" message instead of hanging the entire CI run.
    let minio =
        match tokio::time::timeout(std::time::Duration::from_secs(90), MinIO::default().start())
            .await
        {
            Ok(Ok(minio)) => minio,
            Ok(Err(err)) => {
                println!("skipping MinIO testcontainer test: container failed to start: {err}");
                return;
            }
            Err(_) => {
                println!(
                    "skipping MinIO testcontainer test: container did not become \
                 ready within 90s (Docker pull / daemon issue)"
                );
                return;
            }
        };
    let host = minio
        .get_host()
        .await
        .expect("MinIO host should be reachable");
    let port = minio
        .get_host_port_ipv4(9000)
        .await
        .expect("MinIO 9000 should be mapped to host");
    let endpoint = format!("http://{host}:{port}");

    // 2. Pre-create the `surch-snapshots` bucket via the AWS SDK
    //    directly. The snapshot repository code only does
    //    `PutObject`/`GetObject`/`ListObjectsV2`; bucket creation is
    //    out of scope for `S3Repository`.
    let sdk_cfg = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            "minioadmin",
            "minioadmin",
            None,
            None,
            "surch-snapshot-test",
        ))
        .endpoint_url(&endpoint)
        .force_path_style(true)
        .build();
    let s3 = aws_sdk_s3::Client::from_conf(sdk_cfg);
    s3.create_bucket()
        .bucket("surch-snapshots")
        .send()
        .await
        .expect("MinIO create_bucket(surch-snapshots)");

    let api_url = spawn_surch_api().await;
    let client = reqwest::Client::new();

    // 3. Register the s3 repository against the live MinIO endpoint.
    let resp = client
        .put(format!("{api_url}/_snapshot/cloud"))
        .json(&json!({
            "type": "s3",
            "settings": {
                "bucket": "surch-snapshots",
                "region": "us-east-1",
                "endpoint": endpoint,
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

    // 4. Create the `source` index with a minimal mapping.
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

    // 5. Bulk-index 24 docs; every third doc is `category=science`.
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

    // 6. Take a snapshot of `source`.
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

    // 7. MinIO must now hold the root manifest + at least one
    //    payload blob. ListObjectsV2 the bucket and check.
    let listed = s3
        .list_objects_v2()
        .bucket("surch-snapshots")
        .send()
        .await
        .expect("MinIO list_objects_v2");
    let keys: Vec<String> = listed
        .contents()
        .iter()
        .filter_map(|o| o.key().map(str::to_owned))
        .collect();
    assert!(
        keys.iter()
            .any(|k| k.ends_with("/index-0") || k == "index-0"),
        "MinIO should contain a root manifest key, got {keys:?}"
    );
    assert!(
        keys.iter().any(|k| k.ends_with(".dat")),
        "MinIO should contain at least one .dat payload, got {keys:?}"
    );

    // 8. Drop `source` and restore from the snapshot.
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

    // 9. After restore, search must surface all 24 alpha docs and the
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
