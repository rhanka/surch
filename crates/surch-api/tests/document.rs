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
async fn document_router_indexes_object_fixture_with_put() {
    let request_body = include_str!("../../../tests/opensearch_compat/document/index_request.json");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/document/bootstrap_response.json"
    ))
    .expect("response fixture should be valid json");

    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response_json(response).await, expected_response);
}

#[tokio::test]
async fn document_router_indexes_object_fixture_with_post() {
    let request_body = include_str!("../../../tests/opensearch_compat/document/index_request.json");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/document/bootstrap_response.json"
    ))
    .expect("response fixture should be valid json");

    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response_json(response).await, expected_response);
}

#[tokio::test]
async fn document_router_rejects_non_object_source_with_opensearch_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(r#"["not", "an", "object"]"#))
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
                "reason": "document source must be an object"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn document_router_rejects_invalid_json_with_opensearch_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"desk""#))
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
                "reason": "EOF while parsing an object at line 1 column 14"
            },
            "status": 400
        })
    );
}
