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
async fn bulk_router_accepts_post_bulk_http_fixture() {
    let request_body =
        include_str!("../../../tests/opensearch_compat/bulk/http_bulk_request.ndjson");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/bulk/http_bulk_response.json"
    ))
    .expect("response fixture should be valid json");

    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["errors"], expected_response["errors"]);
    assert_eq!(response["items"], expected_response["items"]);
}

#[tokio::test]
async fn bulk_router_accepts_body_above_axum_default_limit() {
    const AXUM_DEFAULT_BODY_LIMIT_BYTES: usize = 2_097_152;

    let oversized_label = "r".repeat(AXUM_DEFAULT_BODY_LIMIT_BYTES);
    let request_body = format!(
        "{{\"index\":{{\"_index\":\"ban_demo\",\"_id\":\"large-ban-bulk\"}}}}\n\
         {{\"label\":\"{oversized_label}\"}}\n"
    );
    assert!(request_body.len() > AXUM_DEFAULT_BODY_LIMIT_BYTES);

    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn bulk_router_does_not_accept_unknown_route_as_success() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/unknown")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert!(!response.status().is_success());
}

#[tokio::test]
async fn bulk_router_accepts_index_route_with_default_index() {
    let router = app_router();

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"index":{"_id":"1"}}
{"title":"first item"}
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["errors"], false);
    assert_eq!(response["items"][0]["index"]["_index"], "catalog");

    let search_response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(search_response.status(), StatusCode::OK);
    let search_response = response_json(search_response).await;
    assert_eq!(search_response["hits"]["hits"][0]["_id"], "1");
}

#[tokio::test]
async fn bulk_router_makes_batched_documents_searchable() {
    let router = app_router();

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"index":{"_id":"1"}}
{"title":"alpha road"}
{"index":{"_id":"2"}}
{"title":"beta road"}
{"index":{"_id":"3"}}
{"title":"alpha square"}
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["errors"], false);
    assert_eq!(response["items"].as_array().expect("items array").len(), 3);

    let search_response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"match":{"title":"alpha"}},"size":10}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(search_response.status(), StatusCode::OK);
    let search_response = response_json(search_response).await;
    assert_eq!(search_response["hits"]["total"]["value"], 2);
}

#[tokio::test]
async fn bulk_router_reports_missing_id_as_item_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"index":{"_index":"products"}}
{"title":"Missing id"}
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["errors"], true);
    assert_eq!(response["items"][0]["index"]["_index"], "products");
    assert_eq!(response["items"][0]["index"]["status"], 400);
    assert_eq!(
        response["items"][0]["index"]["error"]["type"],
        "illegal_argument_exception"
    );
    assert_eq!(
        response["items"][0]["index"]["error"]["reason"],
        "missing _id in bulk operation metadata"
    );
}

#[tokio::test]
async fn bulk_router_reports_duplicate_create_as_conflict() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"create":{"_id":"sku-1"}}
{"name":"first"}
{"create":{"_id":"sku-1"}}
{"name":"second"}
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["errors"], true);
    assert_eq!(response["items"][1]["create"]["status"], 409);
    assert_eq!(
        response["items"][1]["create"]["error"]["type"],
        "version_conflict_engine_exception"
    );
}

#[tokio::test]
async fn bulk_router_rejects_non_object_source_with_parse_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"index":{"_index":"products","_id":"sku-1"}}
42
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
