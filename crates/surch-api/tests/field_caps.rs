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

async fn create_index(router: &Router, name: &str, mapping_body: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/{name}"))
                .header("content-type", "application/json")
                .body(Body::from(mapping_body.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert!(response.status().is_success());
}

async fn post_field_caps(router: &Router, path: &str, body: &'static str) -> serde_json::Value {
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
async fn field_caps_returns_all_fields_when_no_filter_provided() {
    let router = app_router();
    create_index(
        &router,
        "products",
        r#"{"mappings":{"properties":{"name":{"type":"text"},"price":{"type":"long"}}}}"#,
    )
    .await;

    let body = post_field_caps(&router, "/products/_field_caps", "").await;

    assert_eq!(body["indices"], serde_json::json!(["products"]));
    assert_eq!(body["fields"]["name"]["text"]["type"], "text");
    assert_eq!(body["fields"]["name"]["text"]["searchable"], true);
    assert_eq!(body["fields"]["name"]["text"]["aggregatable"], false);
    assert_eq!(body["fields"]["price"]["long"]["type"], "long");
    assert_eq!(body["fields"]["price"]["long"]["aggregatable"], true);
}

#[tokio::test]
async fn field_caps_filters_to_requested_fields_only() {
    let router = app_router();
    create_index(
        &router,
        "products",
        r#"{"mappings":{"properties":{"name":{"type":"text"},"price":{"type":"long"},"sku":{"type":"keyword"}}}}"#,
    )
    .await;

    let body = post_field_caps(
        &router,
        "/products/_field_caps",
        r#"{"fields":["name","sku"]}"#,
    )
    .await;

    let fields = body["fields"].as_object().expect("fields object");
    assert!(fields.contains_key("name"));
    assert!(fields.contains_key("sku"));
    assert!(!fields.contains_key("price"));
    assert_eq!(body["fields"]["sku"]["keyword"]["aggregatable"], true);
}

#[tokio::test]
async fn field_caps_wildcard_returns_all_fields_when_combined_with_others() {
    let router = app_router();
    create_index(
        &router,
        "products",
        r#"{"mappings":{"properties":{"name":{"type":"text"},"price":{"type":"long"}}}}"#,
    )
    .await;

    let body = post_field_caps(&router, "/products/_field_caps", r#"{"fields":["*"]}"#).await;

    let fields = body["fields"].as_object().expect("fields object");
    assert_eq!(fields.len(), 2);
}

#[tokio::test]
async fn field_caps_aggregates_fields_across_all_indices() {
    let router = app_router();
    create_index(
        &router,
        "products",
        r#"{"mappings":{"properties":{"name":{"type":"text"}}}}"#,
    )
    .await;
    create_index(
        &router,
        "people",
        r#"{"mappings":{"properties":{"age":{"type":"integer"}}}}"#,
    )
    .await;

    let body = post_field_caps(&router, "/_field_caps", "").await;

    let mut indices = body["indices"]
        .as_array()
        .expect("indices array")
        .iter()
        .map(|value| value.as_str().expect("index name").to_owned())
        .collect::<Vec<_>>();
    indices.sort();
    assert_eq!(indices, vec!["people", "products"]);
    assert_eq!(body["fields"]["name"]["text"]["type"], "text");
    assert_eq!(body["fields"]["age"]["integer"]["type"], "integer");
}

#[tokio::test]
async fn field_caps_returns_404_for_missing_index() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/missing/_field_caps")
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

#[tokio::test]
async fn field_caps_rejects_non_string_fields_with_opensearch_error() {
    let router = app_router();
    create_index(&router, "products", "").await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_field_caps")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"fields":[42]}"#))
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
                "reason": "_field_caps `fields` entries must be strings"
            },
            "status": 400
        })
    );
}
