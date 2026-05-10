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
async fn index_router_rejects_duplicate_index_creation() {
    let router = app_router();

    let response = router
        .clone()
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
    assert_eq!(response_json(response).await["index"], "products");

    let duplicate = router
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(duplicate).await,
        serde_json::json!({
            "error": {
                "type": "resource_already_exists_exception",
                "reason": "index [products] already exists"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn index_router_delete_missing_index_returns_not_found() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/missing")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "error": {
                "type": "index_not_found_exception",
                "reason": "index [missing] missing"
            },
            "status": 404
        })
    );
}

#[tokio::test]
async fn index_router_deletes_index_with_opensearch_acknowledgement() {
    let router = app_router();

    let create_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert!(create_response.status().is_success());

    let response = router
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
