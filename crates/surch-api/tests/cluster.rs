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

/// L6.2: ES 8.6.1 `TransportClusterHealthAction` treats a missing index as
/// RED; the wait can never be satisfied, so the call answers after
/// `timeout` with `timed_out: true` and HTTP 408
/// (`ClusterHealthResponse.status()`).
#[tokio::test]
async fn cluster_health_times_out_red_408_for_missing_index() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cluster/health/missing?wait_for_status=green&timeout=0s")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    let body = response_json(response).await;
    assert_eq!(body["status"], "red");
    assert_eq!(body["timed_out"], true);
}

#[tokio::test]
async fn cluster_health_waits_for_an_index_created_during_the_wait() {
    let router = app_router();
    let waiter = {
        let router = router.clone();
        tokio::spawn(async move {
            router
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri("/_cluster/health/late?wait_for_status=green&timeout=10s")
                        .body(Body::empty())
                        .expect("request should build"),
                )
                .await
                .expect("router should respond")
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    create_index(&router, "late").await;
    let response = waiter.await.expect("waiter joins");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["status"], "green");
    assert_eq!(body["timed_out"], false);
    assert_eq!(body["indices"]["late"]["status"], "green");
}

#[tokio::test]
async fn cluster_health_rejects_unknown_status_and_bad_timeout() {
    for uri in [
        "/_cluster/health?wait_for_status=blue",
        "/_cluster/health?timeout=10",
    ] {
        let response = app_router()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
    }
}
