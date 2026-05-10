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

async fn create_index(router: &Router, name: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/{name}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert!(response.status().is_success());
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

#[tokio::test]
async fn cat_indices_returns_empty_array_when_cluster_has_no_indices() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cat/indices")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, serde_json::json!([]));
}

#[tokio::test]
async fn cat_indices_lists_each_index_with_doc_counts() {
    let router = app_router();
    create_index(&router, "products").await;
    create_index(&router, "users").await;
    index_doc(&router, "products", "sku-1", r#"{"name":"desk"}"#).await;
    index_doc(&router, "products", "sku-2", r#"{"name":"chair"}"#).await;
    index_doc(&router, "users", "u-1", r#"{"name":"alice"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cat/indices")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let rows = body.as_array().expect("rows array");
    assert_eq!(rows.len(), 2);

    let products = rows
        .iter()
        .find(|row| row["index"] == "products")
        .expect("products row");
    assert_eq!(products["health"], "green");
    assert_eq!(products["status"], "open");
    assert_eq!(products["docs.count"], "2");
    assert_eq!(products["pri"], "1");
    assert_eq!(products["rep"], "0");

    let users = rows
        .iter()
        .find(|row| row["index"] == "users")
        .expect("users row");
    assert_eq!(users["docs.count"], "1");
}
