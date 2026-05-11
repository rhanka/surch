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
async fn cat_indices_returns_empty_array_when_cluster_has_no_indices() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cat/indices")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, serde_json::json!([]));
}

#[tokio::test]
async fn cat_indices_lists_each_index_with_doc_counts() {
    let router = app_router();
    create_index(&router, "products").await;
    create_index(&router, "users").await;
    index_doc(&router, "products", "sku-1", r#"{"name":"desk"}"#).await;
    index_doc(&router, "products", "sku-2", r#"{"name":"chair"}"#).await;
    index_doc(&router, "users", "u-1", r#"{"name":"alice"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cat/indices")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let rows = body.as_array().expect("rows array");
    assert_eq!(rows.len(), 2);

    let products = rows
        .iter()
        .find(|row| row["index"] == "products")
        .expect("products row");
    assert_eq!(products["health"], "green");
    assert_eq!(products["status"], "open");
    assert_eq!(products["docs.count"], "2");
    assert_eq!(products["pri"], "1");
    assert_eq!(products["rep"], "0");

    let users = rows
        .iter()
        .find(|row| row["index"] == "users")
        .expect("users row");
    assert_eq!(users["docs.count"], "1");
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

async fn post_aliases(router: &Router, body: &'static str) {
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
}

#[tokio::test]
async fn cat_health_returns_single_green_row_with_shard_count() {
    let router = app_router();
    create_index(&router, "products").await;
    create_index(&router, "users").await;

    let (status, body) = get_json(&router, "/_cat/health").await;
    assert_eq!(status, StatusCode::OK);
    let rows = body.as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cluster"], "surch-cluster");
    assert_eq!(rows[0]["status"], "green");
    assert_eq!(rows[0]["shards"], "2");
    assert_eq!(rows[0]["pri"], "2");
    assert_eq!(rows[0]["unassign"], "0");
}

#[tokio::test]
async fn cat_aliases_lists_one_row_per_alias_index_pair() {
    let router = app_router();
    create_index(&router, "logs_2025").await;
    create_index(&router, "logs_2026").await;
    post_aliases(
        &router,
        r#"{"actions":[
            {"add":{"index":"logs_2025","alias":"logs"}},
            {"add":{"index":"logs_2026","alias":"logs"}}
        ]}"#,
    )
    .await;

    let (status, body) = get_json(&router, "/_cat/aliases").await;
    assert_eq!(status, StatusCode::OK);
    let rows = body.as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    let aliases: Vec<&str> = rows
        .iter()
        .map(|row| row["alias"].as_str().expect("alias"))
        .collect();
    assert_eq!(aliases, vec!["logs", "logs"]);
    let mut indices: Vec<String> = rows
        .iter()
        .map(|row| row["index"].as_str().expect("index").to_owned())
        .collect();
    indices.sort();
    assert_eq!(indices, vec!["logs_2025", "logs_2026"]);
}

#[tokio::test]
async fn cat_aliases_by_name_filters_to_matching_alias() {
    let router = app_router();
    create_index(&router, "products").await;
    create_index(&router, "users").await;
    post_aliases(
        &router,
        r#"{"actions":[
            {"add":{"index":"products","alias":"shop"}},
            {"add":{"index":"users","alias":"people"}}
        ]}"#,
    )
    .await;

    let (status, body) = get_json(&router, "/_cat/aliases/shop").await;
    assert_eq!(status, StatusCode::OK);
    let rows = body.as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["alias"], "shop");
    assert_eq!(rows[0]["index"], "products");
}

#[tokio::test]
async fn cat_aliases_by_name_returns_404_when_unknown() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cat/aliases/missing")
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

#[tokio::test]
async fn cat_count_returns_cluster_wide_total_when_no_index() {
    let router = app_router();
    create_index(&router, "products").await;
    index_doc(&router, "products", "1", r#"{"name":"a"}"#).await;
    index_doc(&router, "products", "2", r#"{"name":"b"}"#).await;
    create_index(&router, "users").await;
    index_doc(&router, "users", "u1", r#"{"name":"alice"}"#).await;

    let (status, body) = get_json(&router, "/_cat/count").await;
    assert_eq!(status, StatusCode::OK);
    let rows = body.as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["count"], "3");
}

#[tokio::test]
async fn cat_count_for_specific_index_returns_local_total() {
    let router = app_router();
    create_index(&router, "products").await;
    index_doc(&router, "products", "1", r#"{"name":"a"}"#).await;
    index_doc(&router, "products", "2", r#"{"name":"b"}"#).await;
    create_index(&router, "users").await;
    index_doc(&router, "users", "u1", r#"{"name":"alice"}"#).await;

    let (status, body) = get_json(&router, "/_cat/count/products").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["count"], "2");
}

#[tokio::test]
async fn cat_count_through_alias_sums_pointed_indices() {
    let router = app_router();
    create_index(&router, "logs_a").await;
    create_index(&router, "logs_b").await;
    index_doc(&router, "logs_a", "1", r#"{"e":1}"#).await;
    index_doc(&router, "logs_b", "1", r#"{"e":2}"#).await;
    index_doc(&router, "logs_b", "2", r#"{"e":3}"#).await;
    post_aliases(
        &router,
        r#"{"actions":[
            {"add":{"index":"logs_a","alias":"logs"}},
            {"add":{"index":"logs_b","alias":"logs"}}
        ]}"#,
    )
    .await;

    let (status, body) = get_json(&router, "/_cat/count/logs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body[0]["count"], "3");
}

#[tokio::test]
async fn cat_count_returns_404_for_missing_target() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_cat/count/nope")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
