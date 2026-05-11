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

async fn post_aliases(router: &Router, body: &'static str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_aliases")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn get_json(router: &Router, path: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let status = response.status();
    (status, response_json(response).await)
}

#[tokio::test]
async fn aliases_action_add_creates_alias_pointing_at_index() {
    let router = app_router();
    create_index(&router, "products_v1").await;

    let body = post_aliases(
        &router,
        r#"{"actions":[{"add":{"index":"products_v1","alias":"products"}}]}"#,
    )
    .await;
    assert_eq!(body, serde_json::json!({"acknowledged": true}));

    let (status, response) = get_json(&router, "/_alias").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response,
        serde_json::json!({
            "products_v1": {"aliases": {"products": {}}}
        })
    );
}

#[tokio::test]
async fn aliases_action_supports_atomic_add_and_remove() {
    let router = app_router();
    create_index(&router, "products_v0").await;
    create_index(&router, "products_v1").await;
    post_aliases(
        &router,
        r#"{"actions":[{"add":{"index":"products_v0","alias":"products"}}]}"#,
    )
    .await;

    post_aliases(
        &router,
        r#"{"actions":[
            {"add":{"index":"products_v1","alias":"products"}},
            {"remove":{"index":"products_v0","alias":"products"}}
        ]}"#,
    )
    .await;

    let (status, response) = get_json(&router, "/_alias/products").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response,
        serde_json::json!({
            "products_v1": {"aliases": {"products": {}}}
        })
    );
}

#[tokio::test]
async fn aliases_action_rejects_add_on_missing_index() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_aliases")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"actions":[{"add":{"index":"missing","alias":"foo"}}]}"#,
                ))
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
async fn aliases_rejects_empty_actions_with_opensearch_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_aliases")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"actions":[]}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"]["reason"],
        "_aliases `actions` must not be empty"
    );
}

#[tokio::test]
async fn put_index_alias_creates_alias_via_shortcut() {
    let router = app_router();
    create_index(&router, "logs_2026").await;

    let put_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/logs_2026/_alias/logs")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(put_response.status(), StatusCode::OK);

    let (status, response) = get_json(&router, "/logs_2026/_alias").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response,
        serde_json::json!({
            "logs_2026": {"aliases": {"logs": {}}}
        })
    );
}

#[tokio::test]
async fn delete_index_alias_removes_alias_via_shortcut() {
    let router = app_router();
    create_index(&router, "metrics").await;
    post_aliases(
        &router,
        r#"{"actions":[{"add":{"index":"metrics","alias":"current_metrics"}}]}"#,
    )
    .await;

    let delete_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/metrics/_alias/current_metrics")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let (status, response) = get_json(&router, "/_alias").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response,
        serde_json::json!({
            "metrics": {"aliases": {}}
        })
    );
}

#[tokio::test]
async fn delete_index_drops_associated_aliases() {
    let router = app_router();
    create_index(&router, "ephemeral").await;
    post_aliases(
        &router,
        r#"{"actions":[{"add":{"index":"ephemeral","alias":"alias_a"}}]}"#,
    )
    .await;

    let delete_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/ephemeral")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(delete_response.status(), StatusCode::OK);

    let (status, response) = get_json(&router, "/_alias").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response, serde_json::json!({}));
}

async fn index_doc(router: &Router, index: &str, id: &str, source: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_doc/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(source.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn search_through_alias_returns_documents_from_pointed_index() {
    let router = app_router();
    create_index(&router, "products_v1").await;
    index_doc(&router, "products_v1", "sku-1", r#"{"name":"desk"}"#).await;
    post_aliases(
        &router,
        r#"{"actions":[{"add":{"index":"products_v1","alias":"products"}}]}"#,
    )
    .await;

    let search_response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}},"_source":false}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(search_response.status(), StatusCode::OK);
    let body = response_json(search_response).await;
    assert_eq!(body["hits"]["total"]["value"], 1);
    assert_eq!(body["hits"]["hits"][0]["_index"], "products_v1");
    assert_eq!(body["hits"]["hits"][0]["_id"], "sku-1");
}

#[tokio::test]
async fn search_through_alias_fans_out_over_multiple_indices() {
    let router = app_router();
    create_index(&router, "logs_2025").await;
    create_index(&router, "logs_2026").await;
    index_doc(&router, "logs_2025", "a", r#"{"message":"old"}"#).await;
    index_doc(&router, "logs_2026", "b", r#"{"message":"recent"}"#).await;
    post_aliases(
        &router,
        r#"{"actions":[
            {"add":{"index":"logs_2025","alias":"logs"}},
            {"add":{"index":"logs_2026","alias":"logs"}}
        ]}"#,
    )
    .await;

    let body = response_json(
        router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/logs/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":{"match_all":{}},"_source":false}"#))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond"),
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 2);
    let mut hit_indices: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_index"].as_str().map(str::to_owned).expect("index"))
        .collect();
    hit_indices.sort();
    assert_eq!(hit_indices, vec!["logs_2025", "logs_2026"]);
}

#[tokio::test]
async fn count_through_alias_sums_documents_across_pointed_indices() {
    let router = app_router();
    create_index(&router, "logs_2025").await;
    create_index(&router, "logs_2026").await;
    index_doc(&router, "logs_2025", "a", r#"{"level":"info"}"#).await;
    index_doc(&router, "logs_2025", "b", r#"{"level":"warn"}"#).await;
    index_doc(&router, "logs_2026", "c", r#"{"level":"info"}"#).await;
    post_aliases(
        &router,
        r#"{"actions":[
            {"add":{"index":"logs_2025","alias":"logs"}},
            {"add":{"index":"logs_2026","alias":"logs"}}
        ]}"#,
    )
    .await;

    let body = response_json(
        router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/logs/_count")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":{"match_all":{}}}"#))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond"),
    )
    .await;
    assert_eq!(body["count"], 3);
}

#[tokio::test]
async fn get_alias_by_name_returns_404_when_unknown() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_alias/missing_alias")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(response).await["error"]["type"],
        "aliases_not_found_exception"
    );
}
