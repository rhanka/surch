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

async fn post_msearch(router: &Router, path: &str, body: String) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header("content-type", "application/x-ndjson")
                .body(Body::from(body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

#[tokio::test]
async fn msearch_returns_one_response_per_query_pair() {
    let router = app_router();
    index_doc(&router, "products", "sku-1", r#"{"name":"desk"}"#).await;
    index_doc(&router, "products", "sku-2", r#"{"name":"chair"}"#).await;

    let body = "{\"index\":\"products\"}\n\
                {\"query\":{\"match_all\":{}}}\n\
                {\"index\":\"products\"}\n\
                {\"query\":{\"term\":{\"name\":\"desk\"}}}\n"
        .to_owned();

    let response = post_msearch(&router, "/_msearch", body).await;
    let responses = response["responses"].as_array().expect("responses array");

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["status"], 200);
    assert_eq!(responses[0]["hits"]["total"]["value"], 2);
    assert_eq!(responses[1]["status"], 200);
    assert_eq!(responses[1]["hits"]["total"]["value"], 1);
    assert_eq!(responses[1]["hits"]["hits"][0]["_id"], "sku-1");
}

#[tokio::test]
async fn msearch_uses_path_index_when_header_omits_index() {
    let router = app_router();
    index_doc(&router, "products", "sku-1", r#"{"name":"desk"}"#).await;

    let body = "{}\n\
                {\"query\":{\"match_all\":{}}}\n"
        .to_owned();

    let response = post_msearch(&router, "/products/_msearch", body).await;
    let responses = response["responses"].as_array().expect("responses array");

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["status"], 200);
    assert_eq!(responses[0]["hits"]["total"]["value"], 1);
}

#[tokio::test]
async fn msearch_reports_missing_index_per_pair_without_failing_the_batch() {
    let router = app_router();
    index_doc(&router, "products", "sku-1", r#"{"name":"desk"}"#).await;

    let body = "{\"index\":\"products\"}\n\
                {\"query\":{\"match_all\":{}}}\n\
                {\"index\":\"missing\"}\n\
                {\"query\":{\"match_all\":{}}}\n"
        .to_owned();

    let response = post_msearch(&router, "/_msearch", body).await;
    let responses = response["responses"].as_array().expect("responses array");

    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["status"], 200);
    assert_eq!(responses[1]["status"], 404);
    assert_eq!(responses[1]["error"]["type"], "index_not_found_exception");
}

#[tokio::test]
async fn msearch_reports_invalid_query_body_per_pair() {
    let router = app_router();
    index_doc(&router, "products", "sku-1", r#"{"name":"desk"}"#).await;

    let body = "{\"index\":\"products\"}\n\
                {\"query\":{\"range\":{\"price\":{\"gte\":10}}}}\n"
        .to_owned();

    let response = post_msearch(&router, "/_msearch", body).await;
    let responses = response["responses"].as_array().expect("responses array");

    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["status"], 400);
    assert_eq!(responses[0]["error"]["type"], "parsing_exception");
}

#[tokio::test]
async fn msearch_rejects_empty_body_with_opensearch_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_msearch")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(""))
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
                "reason": "_msearch request must contain at least one search"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn msearch_rejects_unpaired_lines_with_opensearch_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_msearch")
                .header("content-type", "application/x-ndjson")
                .body(Body::from("{\"index\":\"products\"}\n"))
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
                "reason": "_msearch NDJSON must contain header/body pairs"
            },
            "status": 400
        })
    );
}
