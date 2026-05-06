use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use surch_api::app_router;
use tower::ServiceExt;

async fn response_json(response: axum::response::Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");

    serde_json::from_slice(&body).expect("response body should be json")
}

#[tokio::test]
async fn bulk_router_accepts_post_bulk_http_fixture() {
    let request_body =
        include_str!("../../../tests/opensearch_compat/bulk/http_bulk_request.ndjson");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/bulk/http_bulk_response.json"
    ))
    .expect("response fixture should be valid json");

    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, expected_response);
}

#[tokio::test]
async fn bulk_router_does_not_accept_unknown_route_as_success() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/unknown")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert!(!response.status().is_success());
}
