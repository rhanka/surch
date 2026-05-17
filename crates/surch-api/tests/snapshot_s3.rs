//! Integration tests for the ES-parity `_snapshot` REST surface with
//! an `s3`-typed repository (`C-SNAPSHOT-S2`).
//!
//! The tests in this file exercise the *registration / parse* path —
//! `PUT /_snapshot/{repo}` with an `{"type":"s3"}` body and the
//! follow-up `GET /_snapshot/{repo}` metadata read — which is the
//! piece that has to work for every existing ES client (`Curator`,
//! `elasticsearch-py`, the Kibana repository UI) before any take or
//! restore can be attempted. We deliberately do **not** drive a real
//! take/restore against a live S3 endpoint from this binary: the AWS
//! SDK speaks SigV4 + the AWS XML/JSON dialects, and a fully
//! protocol-compliant in-process mock would be more code than the
//! production `S3Repository` it tries to exercise. The end-to-end
//! flavour is left to the MinIO CI step in `docs/ops/snapshot-es-api.md`
//! — flagged "MinIO e2e pending" in the work-package report.
//!
//! What is covered in-process:
//!  1. PUT s3 body with valid `bucket` → 200 OK, GET echoes the
//!     repository metadata (type + settings minus the credentials).
//!  2. PUT s3 body without `bucket` → 400 with a `repository_exception`
//!     error and a reason string that names the missing field.
//!  3. Direct `S3Repository::new` validates the same configuration
//!     surface and rejects an empty bucket with `InvalidConfig`.

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use serde_json::{json, Value};
use surch_api::{
    app_router,
    snapshot_es::{S3Repository, S3RepositoryConfig},
};
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

    let (status, body) = get(&router, "/_snapshot/cloud").await;
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
///
/// This is the same guarantee the registration path leans on: a
/// misconfigured `_snapshot` PUT must fail *at registration time*,
/// not when an operator triggers a take.
#[tokio::test]
async fn s3_repository_new_rejects_empty_bucket() {
    // `tokio::task::spawn_blocking` mirrors how the axum handler
    // builds the repository (S3Repository::new starts a current-thread
    // runtime internally; calling it from the async test task without
    // spawn_blocking would panic with "Cannot start a runtime from
    // within a runtime").
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
