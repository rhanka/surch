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
async fn index_router_creates_index_with_opensearch_acknowledgement() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "acknowledged": true,
            "shards_acknowledged": true,
            "index": "products"
        })
    );
}

#[tokio::test]
async fn index_router_deletes_index_with_opensearch_acknowledgement() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/products")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "acknowledged": true
        })
    );
}

#[tokio::test]
async fn index_router_refreshes_index_with_shards_response() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_refresh")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "_shards": {
                "total": 1,
                "successful": 1,
                "failed": 0
            }
        })
    );
}
