use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    response::IntoResponse,
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
async fn root_router_returns_opensearch_bootstrap_fixture() {
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/root/bootstrap_response.json"
    ))
    .expect("response fixture should be valid json");

    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, expected_response);
}

#[tokio::test]
async fn root_router_does_not_accept_unknown_route_as_success() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/unknown")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert!(!response.status().is_success());
}

#[tokio::test]
async fn root_error_envelope_exposes_opensearch_shape() {
    let response = surch_api::OpenSearchError::new(
        StatusCode::BAD_REQUEST,
        "parse_exception",
        "request body is invalid",
    )
    .into_response();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "error": {
                "type": "parse_exception",
                "reason": "request body is invalid"
            },
            "status": 400
        })
    );
}
