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

#[tokio::test]
async fn cluster_health_returns_green_status_with_zero_indices() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cluster/health")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["cluster_name"], "surch-cluster");
    assert_eq!(body["status"], "green");
    assert_eq!(body["timed_out"], false);
    assert_eq!(body["number_of_nodes"], 1);
    assert_eq!(body["active_primary_shards"], 0);
    assert_eq!(body["active_shards"], 0);
    assert_eq!(body["unassigned_shards"], 0);
    assert!(body.get("indices").is_none());
}

#[tokio::test]
async fn cluster_health_counts_indices_as_active_primary_shards() {
    let router = app_router();
    create_index(&router, "products").await;
    create_index(&router, "users").await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cluster/health")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["active_primary_shards"], 2);
    assert_eq!(body["active_shards"], 2);
}

#[tokio::test]
async fn cluster_health_index_endpoint_returns_indices_breakdown() {
    let router = app_router();
    create_index(&router, "products").await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cluster/health/products")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], "green");
    assert_eq!(body["indices"]["products"]["status"], "green");
    assert_eq!(body["indices"]["products"]["active_primary_shards"], 1);
}

#[tokio::test]
async fn cluster_health_returns_404_for_missing_index() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cluster/health/missing")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(response).await["error"]["type"],
        "index_not_found_exception"
    );
}
