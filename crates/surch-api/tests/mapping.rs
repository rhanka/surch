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
async fn mapping_router_accepts_legacy_doc_type_wrapper() {
    let legacy_body = r#"{"mappings":{"_doc":{"properties":{"title":{"type":"text"}}}}}"#;

    let router = app_router();
    let create_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/legacy")
                .header("content-type", "application/json")
                .body(Body::from(legacy_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(create_response.status(), StatusCode::OK);

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/legacy/_mapping")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body["legacy"]["mappings"]["properties"]["title"]["type"],
        "text"
    );
}

#[tokio::test]
async fn mapping_router_rejects_invalid_mappings_type() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/invalid-mappings")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"mappings": "not-object"}"#))
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
                "reason": "index request `mappings` must be an object"
            },
            "status": 400
        })
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

async fn create_products_index(router: &axum::Router, body: &'static str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert!(response.status().is_success());
}

#[tokio::test]
async fn put_mapping_adds_new_field_to_existing_index() {
    let router = app_router();
    create_products_index(
        &router,
        r#"{"mappings":{"properties":{"name":{"type":"text"}}}}"#,
    )
    .await;

    let put_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products/_mapping")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"properties":{"price":{"type":"long"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(put_response.status(), StatusCode::OK);
    assert_eq!(
        response_json(put_response).await,
        serde_json::json!({"acknowledged": true})
    );

    let get_response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/products/_mapping")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let body = response_json(get_response).await;
    assert_eq!(
        body["products"]["mappings"]["properties"]["name"]["type"],
        "text"
    );
    assert_eq!(
        body["products"]["mappings"]["properties"]["price"]["type"],
        "long"
    );
}

#[tokio::test]
async fn put_mapping_accepts_raw_properties_body() {
    let router = app_router();
    create_products_index(&router, "").await;

    let put_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products/_mapping")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"sku":{"type":"keyword"}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(put_response.status(), StatusCode::OK);

    let get_response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/products/_mapping")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(
        response_json(get_response).await["products"]["mappings"]["properties"]["sku"]["type"],
        "keyword"
    );
}

#[tokio::test]
async fn put_mapping_rejects_type_conflict_with_opensearch_error() {
    let router = app_router();
    create_products_index(
        &router,
        r#"{"mappings":{"properties":{"price":{"type":"long"}}}}"#,
    )
    .await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products/_mapping")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"properties":{"price":{"type":"keyword"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "illegal_argument_exception");
    assert!(body["error"]["reason"]
        .as_str()
        .expect("reason string")
        .contains("price"));
}

#[tokio::test]
async fn put_mapping_returns_404_for_missing_index() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/missing/_mapping")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"properties":{"a":{"type":"text"}}}"#))
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
async fn put_mapping_keeps_documents_searchable_via_added_field() {
    let router = app_router();
    create_products_index(&router, "").await;

    let index_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"desk","price":42}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(index_response.status(), StatusCode::CREATED);

    let put_mapping = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products/_mapping")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"properties":{"category":{"type":"keyword"}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(put_mapping.status(), StatusCode::OK);

    let search = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"range":{"price":{"gte":10}}},"_source":false}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(search.status(), StatusCode::OK);
    assert_eq!(response_json(search).await["hits"]["total"]["value"], 1);
}
