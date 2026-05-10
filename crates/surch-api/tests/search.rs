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
async fn search_router_match_all_returns_document_indexed_by_doc_api() {
    let router = app_router();

    let index_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"desk"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(index_response.status(), StatusCode::CREATED);

    let search_response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(search_response.status(), StatusCode::OK);

    let search_body = response_json(search_response).await;
    assert_eq!(
        search_body["hits"]["hits"]
            .as_array()
            .expect("hits should be an array"),
        &[serde_json::json!({
            "_index": "products",
            "_id": "sku-1",
        })]
    );
}

#[tokio::test]
async fn search_router_term_returns_exact_text_match_indexed_by_doc_api() {
    let router = app_router();

    let desk_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"desk"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(desk_response.status(), StatusCode::CREATED);

    let desktop_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-2")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"desktop"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(desktop_response.status(), StatusCode::CREATED);

    let search_response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"term":{"name":"desk"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(search_response.status(), StatusCode::OK);

    let search_body = response_json(search_response).await;
    assert_eq!(search_body["hits"]["total"]["value"], 1);
    assert_eq!(
        search_body["hits"]["hits"]
            .as_array()
            .expect("hits should be an array"),
        &[serde_json::json!({
            "_index": "products",
            "_id": "sku-1",
        })]
    );
}

#[tokio::test]
async fn search_router_match_phrase_matches_normalized_contiguous_tokens() {
    let router = app_router();

    let standing_desk_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Compact Standing Desk"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(standing_desk_response.status(), StatusCode::CREATED);

    let reversed_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-2")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"desk standing"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(reversed_response.status(), StatusCode::CREATED);

    let non_contiguous_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-3")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"standing adjustable desk"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(non_contiguous_response.status(), StatusCode::CREATED);

    let search_response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"match_phrase":{"name":"standing desk"}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(search_response.status(), StatusCode::OK);

    let search_body = response_json(search_response).await;
    assert_eq!(search_body["hits"]["total"]["value"], 1);
    assert_eq!(
        search_body["hits"]["hits"]
            .as_array()
            .expect("hits should be an array"),
        &[serde_json::json!({
            "_index": "products",
            "_id": "sku-1",
        })]
    );
}

#[tokio::test]
async fn search_router_accepts_match_all_fixture() {
    let request_body =
        include_str!("../../../tests/opensearch_compat/search/match_all_request.json");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/search/bootstrap_response.json"
    ))
    .expect("response fixture should be valid json");
    let router = app_router();

    let create_index = router
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
    assert!(create_index.status().is_success());

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["timed_out"], expected_response["timed_out"]);
    assert_eq!(response["_shards"], expected_response["_shards"]);
    assert_eq!(response["hits"], expected_response["hits"]);
}

#[tokio::test]
async fn search_router_accepts_empty_object_fixture() {
    let request_body = include_str!("../../../tests/opensearch_compat/search/empty_request.json");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/search/bootstrap_response.json"
    ))
    .expect("response fixture should be valid json");
    let router = app_router();

    let create_index = router
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
    assert!(create_index.status().is_success());

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let response = response_json(response).await;
    assert_eq!(response["timed_out"], expected_response["timed_out"]);
    assert_eq!(response["_shards"], expected_response["_shards"]);
    assert_eq!(response["hits"], expected_response["hits"]);
}

#[tokio::test]
async fn search_router_rejects_invalid_index_name_with_open_search_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/Products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}}}"#))
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
                "reason": "invalid index name"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_unknown_query_with_opensearch_error() {
    let router = app_router();
    let create_index = router
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
    assert!(create_index.status().is_success());

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"range":{"price":{"gte":10}}}}"#))
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
                "reason": "unsupported search query `range`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_term_query_object_without_value_with_opensearch_error() {
    let router = app_router();
    let create_index = router
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
    assert!(create_index.status().is_success());

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"term":{"name":{"query":"desk"}}}}"#,
                ))
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
                "reason": "term field query object must contain `value`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_invalid_pagination_with_opensearch_error() {
    let router = app_router();
    let create_index = router
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
    assert!(create_index.status().is_success());

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"from":-1,"size":10}"#))
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
                "reason": "search `from` must be a non-negative integer"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_missing_index() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/missing/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}}}"#))
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
