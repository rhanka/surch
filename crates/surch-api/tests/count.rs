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
async fn count_router_accepts_match_all_fixture() {
    let request_body =
        include_str!("../../../tests/opensearch_compat/count/match_all_request.json");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/count/bootstrap_response.json"
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
                .uri("/products/_count")
                .header("content-type", "application/json")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, expected_response);
}

#[tokio::test]
async fn count_router_accepts_empty_object_fixture() {
    let request_body = include_str!("../../../tests/opensearch_compat/count/empty_request.json");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/count/bootstrap_response.json"
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
                .uri("/products/_count")
                .header("content-type", "application/json")
                .body(Body::from(request_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await, expected_response);
}

#[tokio::test]
async fn count_router_term_returns_matching_document_count() {
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

    let chair_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-2")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"chair"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(chair_response.status(), StatusCode::CREATED);

    let standing_desk_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-3")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"standing desk"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(standing_desk_response.status(), StatusCode::CREATED);

    let desktop_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-4")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"desktop"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(desktop_response.status(), StatusCode::CREATED);

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_count")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"term":{"name":"desk"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "count": 2,
            "_shards": {
                "total": 1,
                "successful": 1,
                "skipped": 0,
                "failed": 0
            }
        })
    );
}

#[tokio::test]
async fn count_router_bool_must_returns_documents_matching_all_clauses() {
    let router = app_router();

    for (id, body) in [
        ("sku-1", r#"{"name":"desk","category":"furniture"}"#),
        ("sku-2", r#"{"name":"desk","category":"electronics"}"#),
        ("sku-3", r#"{"name":"chair","category":"furniture"}"#),
        ("sku-4", r#"{"name":"lamp","category":"lighting"}"#),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/products/_doc/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_count")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"bool":{"must":[{"term":{"name":"desk"}},{"term":{"category":"furniture"}}]}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "count": 1,
            "_shards": {
                "total": 1,
                "successful": 1,
                "skipped": 0,
                "failed": 0
            }
        })
    );
}

#[tokio::test]
async fn count_router_rejects_invalid_bool_must_with_opensearch_error() {
    for request_body in [
        r#"{"query":{"bool":{"must":[]}}}"#,
        r#"{"query":{"bool":{"must":{"term":{"name":"desk"}}}}}"#,
    ] {
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
                    .uri("/products/_count")
                    .header("content-type", "application/json")
                    .body(Body::from(request_body))
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
                    "reason": "bool.must must be a non-empty array"
                },
                "status": 400
            })
        );
    }
}

#[tokio::test]
async fn count_router_rejects_unknown_query_with_opensearch_error() {
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
                .uri("/products/_count")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"regexp":{"name":"des.*"}}}"#))
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
                "reason": "unsupported count query `regexp`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn count_router_rejects_invalid_term_query_with_opensearch_error() {
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
                .uri("/products/_count")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"term":{}}}"#))
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
                "reason": "term query must contain exactly one field"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn count_router_rejects_missing_index() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/missing/_count")
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

#[tokio::test]
async fn count_router_range_query_counts_documents_within_numeric_bounds() {
    let router = app_router();

    for (id, body) in [
        ("sku-1", r#"{"price":5}"#),
        ("sku-2", r#"{"price":15}"#),
        ("sku-3", r#"{"price":25}"#),
        ("sku-4", r#"{"price":50}"#),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/products/_doc/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_count")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"range":{"price":{"gt":5,"lte":25}}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["count"], 2);
}

#[tokio::test]
async fn count_router_multi_match_and_operator_requires_all_tokens_in_one_field() {
    let router = app_router();

    for (id, body) in [
        (
            "sku-same-field",
            r#"{"name":"rust search guide","description":"engine internals"}"#,
        ),
        (
            "sku-split-fields",
            r#"{"name":"rust","description":"search guide"}"#,
        ),
        (
            "sku-one-token",
            r#"{"name":"rust systems","description":"engine internals"}"#,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/products/_doc/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_count")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"multi_match":{"query":"rust search","fields":["name","description"],"operator":"AND"}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["count"], 1);
}
