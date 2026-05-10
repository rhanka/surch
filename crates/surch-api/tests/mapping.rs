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
async fn mapping_router_returns_index_mapping_payload() {
    let mapping_body = r#"{"mappings":{"properties":{"title":{"type":"text"}}}}"#;
    let router = app_router();

    let create_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .header("content-type", "application/json")
                .body(Body::from(mapping_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(create_response.status(), StatusCode::OK);

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/products/_mapping")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body,
        serde_json::json!({
            "products": {
                "mappings": {
                    "properties": {
                        "title": { "type": "text" }
                    }
                }
            }
        })
    );
}

#[tokio::test]
async fn mapping_router_returns_all_mappings() {
    let create_products = r#"{"mappings":{"properties":{"title":{"type":"text"}}}}"#;
    let create_inventory = r#"{"mappings":{"properties":{"price":{"type":"integer"}}}}"#;

    let router = app_router();

    let products = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .header("content-type", "application/json")
                .body(Body::from(create_products))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(products.status(), StatusCode::OK);

    let inventory = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/inventory")
                .header("content-type", "application/json")
                .body(Body::from(create_inventory))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(inventory.status(), StatusCode::OK);

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_mapping")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body["products"]["mappings"]["properties"]["title"]["type"],
        serde_json::json!("text")
    );
    assert_eq!(
        body["inventory"]["mappings"]["properties"]["price"]["type"],
        serde_json::json!("integer")
    );
}

#[tokio::test]
async fn mapping_router_index_not_found() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/missing/_mapping")
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
