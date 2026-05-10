use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use surch_api::app_router;
use tower::ServiceExt;

async fn response_json(response: axum::response::Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be json")
}

async fn index_doc(router: &Router, index: &str, id: &str, source: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_doc/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(source.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::CREATED);
}

async fn post_mget(router: &Router, path: &str, body: &'static str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

#[tokio::test]
async fn mget_returns_docs_with_source_for_global_endpoint() {
    let router = app_router();
    index_doc(&router, "products", "sku-1", r#"{"name":"desk"}"#).await;
    index_doc(&router, "products", "sku-2", r#"{"name":"chair"}"#).await;

    let body = post_mget(
        &router,
        "/_mget",
        r#"{"docs":[{"_index":"products","_id":"sku-1"},{"_index":"products","_id":"sku-2"}]}"#,
    )
    .await;

    let docs = body["docs"].as_array().expect("docs array");
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0]["_index"], "products");
    assert_eq!(docs[0]["_id"], "sku-1");
    assert_eq!(docs[0]["found"], true);
    assert_eq!(docs[0]["_source"], serde_json::json!({"name": "desk"}));
    assert_eq!(docs[1]["_id"], "sku-2");
    assert_eq!(docs[1]["_source"], serde_json::json!({"name": "chair"}));
}

#[tokio::test]
async fn mget_marks_missing_documents_as_not_found() {
    let router = app_router();
    index_doc(&router, "products", "sku-1", r#"{"name":"desk"}"#).await;

    let body = post_mget(
        &router,
        "/products/_mget",
        r#"{"ids":["sku-1","sku-missing"]}"#,
    )
    .await;

    let docs = body["docs"].as_array().expect("docs array");
    assert_eq!(docs.len(), 2);
    assert_eq!(docs[0]["found"], true);
    assert_eq!(docs[1]["found"], false);
    assert!(docs[1].get("_source").is_none());
}

#[tokio::test]
async fn mget_applies_root_source_filter_to_each_doc() {
    let router = app_router();
    index_doc(
        &router,
        "products",
        "sku-1",
        r#"{"name":"desk","price":42,"sku":"S1"}"#,
    )
    .await;

    let body = post_mget(
        &router,
        "/products/_mget",
        r#"{"ids":["sku-1"],"_source":["name","sku"]}"#,
    )
    .await;

    assert_eq!(
        body["docs"][0]["_source"],
        serde_json::json!({"name": "desk", "sku": "S1"})
    );
}

#[tokio::test]
async fn mget_per_doc_source_filter_overrides_root_filter() {
    let router = app_router();
    index_doc(
        &router,
        "products",
        "sku-1",
        r#"{"name":"desk","price":42}"#,
    )
    .await;

    let body = post_mget(
        &router,
        "/products/_mget",
        r#"{"docs":[{"_id":"sku-1","_source":false}],"_source":["name"]}"#,
    )
    .await;

    assert!(body["docs"][0].get("_source").is_none());
    assert_eq!(body["docs"][0]["found"], true);
}

#[tokio::test]
async fn mget_rejects_request_without_docs_or_ids_with_opensearch_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_mget")
                .header("content-type", "application/json")
                .body(Body::from(r#"{}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "error": {
                "type": "parsing_exception",
                "reason": "_mget request must contain `docs` or `ids`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn mget_rejects_ids_form_without_index_in_path() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_mget")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"ids":["sku-1"]}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "error": {
                "type": "parsing_exception",
                "reason": "_mget `ids` requires an index in the request path"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn mget_rejects_doc_without_index_when_no_default() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_mget")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"docs":[{"_id":"sku-1"}]}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "error": {
                "type": "parsing_exception",
                "reason": "_mget item is missing `_index`"
            },
            "status": 400
        })
    );
}
