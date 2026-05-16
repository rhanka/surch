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
            "_source": {"name": "desk"},
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
            "_source": {"name": "desk"},
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
    let hits = search_body["hits"]["hits"]
        .as_array()
        .expect("hits should be an array");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["_index"], "products");
    assert_eq!(hits[0]["_id"], "sku-1");
    assert_eq!(
        hits[0]["_source"],
        serde_json::json!({"name": "Compact Standing Desk"})
    );
    assert!(
        hits[0]["_score"]
            .as_f64()
            .expect("match_phrase should expose a numeric score")
            > 0.0
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
                "reason": "unsupported search query `regexp`"
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

async fn index_product(router: &axum::Router, id: &str, source: &str) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/products/_doc/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(source.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn search_router_omits_source_when_disabled() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk","price":42}"#).await;

    let response = router
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

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body["hits"]["hits"]
            .as_array()
            .expect("hits should be an array"),
        &[serde_json::json!({
            "_index": "products",
            "_id": "sku-1",
        })]
    );
}

#[tokio::test]
async fn search_router_includes_only_selected_fields_when_source_array() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk","price":42,"sku":"S1"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"match_all":{}},"_source":["name","sku"]}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body["hits"]["hits"][0]["_source"],
        serde_json::json!({"name": "desk", "sku": "S1"})
    );
}

#[tokio::test]
async fn search_router_excludes_listed_fields_when_source_object() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk","price":42,"sku":"S1"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"match_all":{}},"_source":{"excludes":["price"]}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body["hits"]["hits"][0]["_source"],
        serde_json::json!({"name": "desk", "sku": "S1"})
    );
}

#[tokio::test]
async fn search_router_rejects_invalid_source_filter_with_opensearch_error() {
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
                .body(Body::from(r#"{"query":{"match_all":{}},"_source":42}"#))
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
                "reason": "`_source` must be a boolean, string, array, or object"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_track_total_hits_false_omits_total_object() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"match_all":{}},"track_total_hits":false}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert!(body["hits"].get("total").is_none());
    assert_eq!(body["hits"]["hits"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn search_router_track_total_hits_int_caps_value_with_gte_relation() {
    let router = app_router();
    for index in 0..5 {
        index_product(
            &router,
            &format!("sku-{index}"),
            &format!(r#"{{"name":"item-{index}"}}"#),
        )
        .await;
    }

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"match_all":{}},"track_total_hits":2,"size":0}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["hits"]["total"]["value"], 2);
    assert_eq!(body["hits"]["total"]["relation"], "gte");
}

#[tokio::test]
async fn search_router_track_total_hits_int_returns_eq_when_total_within_limit() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"match_all":{}},"track_total_hits":10}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["hits"]["total"]["value"], 1);
    assert_eq!(body["hits"]["total"]["relation"], "eq");
}

#[tokio::test]
async fn search_router_rejects_invalid_track_total_hits_with_opensearch_error() {
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
                    r#"{"query":{"match_all":{}},"track_total_hits":"yes"}"#,
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
                "reason": "`track_total_hits` must be a boolean or non-negative integer"
            },
            "status": 400
        })
    );
}

async fn search_with_body(router: &axum::Router, body: &'static str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
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
async fn search_router_returns_highlight_fragments_for_requested_match_field() {
    let router = app_router();
    index_product(
        &router,
        "sku-1",
        r#"{"description":"Rust search engine with safe indexing"}"#,
    )
    .await;
    index_product(&router, "sku-2", r#"{"description":"cooking guide"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"description":"rust search"}},"highlight":{"fields":{"description":{}}},"_source":false}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["_id"], "sku-1");
    assert_eq!(
        hits[0]["highlight"]["description"],
        serde_json::json!(["<em>Rust</em> <em>search</em> engine with safe indexing"])
    );
}

#[tokio::test]
async fn search_router_highlight_with_custom_pre_and_post_tags() {
    let router = app_router();
    index_product(
        &router,
        "sku-1",
        r#"{"description":"Rust search engine with safe indexing"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"description":"rust search"}},"highlight":{"pre_tags":["<mark>"],"post_tags":["</mark>"],"fields":{"description":{}}},"_source":false}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["_id"], "sku-1");
    assert_eq!(
        hits[0]["highlight"]["description"],
        serde_json::json!(["<mark>Rust</mark> <mark>search</mark> engine with safe indexing"])
    );
}

#[tokio::test]
async fn search_router_highlight_returns_multiple_fragments_when_fragment_options_are_set() {
    let router = app_router();
    index_product(
        &router,
        "sku-1",
        r#"{"description":"Rust search in practice, rust search is everywhere and reliable"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"description":"rust search"}},"highlight":{"fields":{"description":{}}, "fragment_size":24, "number_of_fragments":2},"_source":false}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 1);
    let fragments = hits[0]["highlight"]["description"]
        .as_array()
        .expect("highlight array");
    assert_eq!(fragments.len(), 2);
    for fragment in fragments {
        let fragment = fragment.as_str().expect("fragment");
        assert!(fragment.contains("<em>rust</em>") || fragment.contains("<em>Rust</em>"));
        assert!(fragment.contains("<em>search</em>"));
    }
}

#[tokio::test]
async fn search_router_highlight_limits_fragments_to_number_of_fragments() {
    let router = app_router();
    index_product(
        &router,
        "sku-1",
        r#"{"description":"Rust search engine is fast. Rust search engine is safe. Rust search engine is simple."}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"description":"rust search"}},"highlight":{"fields":{"description":{}}, "fragment_size":20, "number_of_fragments":1},"_source":false}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    let fragments = hits[0]["highlight"]["description"]
        .as_array()
        .expect("highlight array");
    assert_eq!(fragments.len(), 1);
    let fragment = fragments[0].as_str().expect("fragment");
    assert!(fragment.contains("<em>Rust</em>"));
    assert!(fragment.contains("<em>search</em>"));
}

#[tokio::test]
async fn search_router_highlight_fragment_options_preserve_utf8_boundaries() {
    let router = app_router();
    index_product(
        &router,
        "sku-1",
        r#"{"description":"caf\u00e9 rust search keeps accents intact"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"description":"rust search"}},"highlight":{"fields":{"description":{}}, "fragment_size":14, "number_of_fragments":1},"_source":false}"#,
    )
    .await;

    let fragments = body["hits"]["hits"][0]["highlight"]["description"]
        .as_array()
        .expect("highlight array");
    assert_eq!(fragments.len(), 1);
    let fragment = fragments[0].as_str().expect("fragment");
    assert!(fragment.contains("<em>rust</em>"));
    assert!(fragment.contains("<em>search</em>"));
}

#[tokio::test]
async fn search_router_rejects_highlight_fragment_size_type_with_opensearch_error() {
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
                    r#"{"highlight":{"fields":{"description":{}},"fragment_size":"20"},"_source":false}"#,
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
                "reason": "`highlight.fragment_size` must be a positive integer",
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_highlight_number_of_fragments_as_negative_with_opensearch_error() {
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
                    r#"{"highlight":{"fields":{"description":{}},"number_of_fragments":0},"_source":false}"#,
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
                "reason": "`highlight.number_of_fragments` must be greater than zero",
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_highlight_pre_tags_with_non_string_elements() {
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
                    r#"{"highlight":{"pre_tags":[42],"post_tags":["</mark>"],"fields":{"description":{}}},"_source":false}"#,
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
                "reason": "`highlight.pre_tags` entries must be strings"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_highlight_fields_array_with_opensearch_error() {
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
                    r#"{"highlight":{"pre_tags":["<mark>"],"post_tags":["</mark>"],"fields":[]}}"#,
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
                "reason": "`highlight.fields` must be an object"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_sorts_documents_by_field_ascending_by_default() {
    let router = app_router();
    index_product(&router, "sku-3", r#"{"name":"Cap","price":15}"#).await;
    index_product(&router, "sku-1", r#"{"name":"Cap","price":5}"#).await;
    index_product(&router, "sku-2", r#"{"name":"Cap","price":10}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"sort":[{"price":"asc"}],"_source":["price"]}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    let prices: Vec<i64> = hits
        .iter()
        .map(|hit| hit["_source"]["price"].as_i64().expect("price"))
        .collect();
    assert_eq!(prices, vec![5, 10, 15]);
}

#[tokio::test]
async fn search_router_sorts_documents_by_field_descending() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"alpha"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"charlie"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"bravo"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"sort":[{"name":{"order":"desc"}}],"_source":["name"]}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    let names: Vec<String> = hits
        .iter()
        .map(|hit| {
            hit["_source"]["name"]
                .as_str()
                .map(str::to_owned)
                .expect("name")
        })
        .collect();
    assert_eq!(names, vec!["charlie", "bravo", "alpha"]);
}

#[tokio::test]
async fn search_router_sorts_with_secondary_clause_breaking_ties() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"category":"a","price":20}"#).await;
    index_product(&router, "sku-2", r#"{"category":"a","price":10}"#).await;
    index_product(&router, "sku-3", r#"{"category":"b","price":5}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"sort":[{"category":"asc"},{"price":"desc"}],"_source":["category","price"]}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    let extracted: Vec<(String, i64)> = hits
        .iter()
        .map(|hit| {
            (
                hit["_source"]["category"]
                    .as_str()
                    .map(str::to_owned)
                    .expect("category"),
                hit["_source"]["price"].as_i64().expect("price"),
            )
        })
        .collect();
    assert_eq!(
        extracted,
        vec![
            ("a".to_owned(), 20),
            ("a".to_owned(), 10),
            ("b".to_owned(), 5),
        ]
    );
}

#[tokio::test]
async fn search_router_sorts_pushes_missing_field_documents_to_the_end() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"price":5}"#).await;
    index_product(&router, "sku-2", r#"{"name":"only name"}"#).await;
    index_product(&router, "sku-3", r#"{"price":10}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"sort":[{"price":"asc"}],"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1", "sku-3", "sku-2"]);
}

#[tokio::test]
async fn search_router_rejects_unknown_sort_order_with_opensearch_error() {
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
                    r#"{"query":{"match_all":{}},"sort":[{"price":"sideways"}]}"#,
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
                "reason": "unknown `sort` order `sideways`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_range_query_filters_numeric_field_with_inclusive_bounds() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"a","price":5}"#).await;
    index_product(&router, "sku-2", r#"{"name":"b","price":10}"#).await;
    index_product(&router, "sku-3", r#"{"name":"c","price":20}"#).await;
    index_product(&router, "sku-4", r#"{"name":"d","price":50}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"range":{"price":{"gte":10,"lte":20}}},"sort":[{"price":"asc"}],"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-2", "sku-3"]);
}

#[tokio::test]
async fn search_router_range_query_excludes_strict_bounds_for_text_field() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"sku":"alpha"}"#).await;
    index_product(&router, "sku-2", r#"{"sku":"bravo"}"#).await;
    index_product(&router, "sku-3", r#"{"sku":"charlie"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"range":{"sku":{"gt":"alpha","lt":"charlie"}}},"_source":["sku"]}"#,
    )
    .await;

    let skus: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| {
            hit["_source"]["sku"]
                .as_str()
                .map(str::to_owned)
                .expect("sku")
        })
        .collect();
    assert_eq!(skus, vec!["bravo"]);
}

#[tokio::test]
async fn search_router_range_query_combines_with_bool_must() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"category":"a","price":5}"#).await;
    index_product(&router, "sku-2", r#"{"category":"a","price":50}"#).await;
    index_product(&router, "sku-3", r#"{"category":"b","price":50}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"term":{"category":"a"}},{"range":{"price":{"gte":10}}}]}},"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-2"]);
}

#[tokio::test]
async fn search_router_rejects_empty_range_with_opensearch_error() {
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
                .body(Body::from(r#"{"query":{"range":{"price":{}}}}"#))
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
                "reason": "range query must contain at least one of `gt`, `gte`, `lt`, `lte`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_exists_query_matches_documents_with_non_null_field() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk","price":42}"#).await;
    index_product(&router, "sku-2", r#"{"name":"chair"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"lamp","price":null}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"exists":{"field":"price"}},"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn search_router_terms_query_matches_any_listed_value() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"chair"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"lamp"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"terms":{"name":["desk","lamp"]}},"sort":[{"name":"asc"}],"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1", "sku-3"]);
}

#[tokio::test]
async fn search_router_rejects_empty_terms_array_with_opensearch_error() {
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
                .body(Body::from(r#"{"query":{"terms":{"name":[]}}}"#))
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
                "reason": "terms query value array must not be empty"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_exists_without_field_with_opensearch_error() {
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
                .body(Body::from(r#"{"query":{"exists":{}}}"#))
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
                "reason": "exists query must contain `field`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_prefix_query_matches_token_prefix() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desktop"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"deck chair"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"lamp"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"prefix":{"name":"des"}},"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn search_router_wildcard_query_matches_star_and_question_patterns() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"desktop"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"chair"}"#).await;
    index_product(&router, "sku-4", r#"{"name":"dusk"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"wildcard":{"name":"d?s*"}},"sort":[{"name":"asc"}],"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1", "sku-2", "sku-4"]);
}

#[tokio::test]
async fn search_router_wildcard_query_accepts_value_wrapper() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"chair"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"wildcard":{"name":{"value":"*esk"}}},"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn search_router_rejects_empty_wildcard_with_opensearch_error() {
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
                .body(Body::from(r#"{"query":{"wildcard":{"name":""}}}"#))
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
                "reason": "wildcard query value must not be empty"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_multi_match_matches_query_in_any_listed_field() {
    let router = app_router();
    index_product(
        &router,
        "sku-1",
        r#"{"name":"Rust Search","description":"engine internals"}"#,
    )
    .await;
    index_product(
        &router,
        "sku-2",
        r#"{"name":"Cooking","description":"rust prevention manual"}"#,
    )
    .await;
    index_product(
        &router,
        "sku-3",
        r#"{"name":"Kayak","description":"river adventures"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"multi_match":{"query":"rust","fields":["name","description"]}},"sort":[{"name":"asc"}],"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-2", "sku-1"]);
}

#[tokio::test]
async fn search_router_multi_match_returns_any_field_containing_query_token() {
    let router = app_router();
    index_product(
        &router,
        "sku-rust-name",
        r#"{"name":"rust","description":"cookbook"}"#,
    )
    .await;
    index_product(
        &router,
        "sku-search-description",
        r#"{"name":"manual","description":"search engine"}"#,
    )
    .await;
    index_product(
        &router,
        "sku-no-overlap",
        r#"{"name":"chair","description":"furniture"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"multi_match":{"query":"rust search","fields":["name","description"]}},"_source":false}"#,
    )
    .await;

    let mut ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["sku-rust-name", "sku-search-description"]);
}

#[tokio::test]
async fn search_router_multi_match_and_operator_requires_all_tokens_in_one_field() {
    let router = app_router();
    index_product(
        &router,
        "sku-same-field",
        r#"{"name":"rust search guide","description":"engine internals"}"#,
    )
    .await;
    index_product(
        &router,
        "sku-split-fields",
        r#"{"name":"rust","description":"search guide"}"#,
    )
    .await;
    index_product(
        &router,
        "sku-one-token",
        r#"{"name":"rust systems","description":"engine internals"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"multi_match":{"query":"rust search","fields":["name","description"],"operator":"AND"}},"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-same-field"]);
}

#[tokio::test]
async fn search_router_rejects_multi_match_without_fields_with_opensearch_error() {
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
                .body(Body::from(r#"{"query":{"multi_match":{"query":"rust"}}}"#))
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
                "reason": "multi_match query must contain `fields`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn search_router_rejects_multi_match_unknown_field_with_opensearch_error() {
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
                    r#"{"query":{"multi_match":{"query":"rust","fields":["name"],"bogus":1}}}"#,
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
                "reason": "unsupported multi_match field `bogus`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn match_query_orders_results_by_bm25_score_descending() {
    let router = app_router();
    // sku-1 mentions the matching phrase once in a short body — high tf/dl ratio.
    index_product(&router, "sku-1", r#"{"description":"rust search engine"}"#).await;
    // sku-2 does not mention the phrase at all.
    index_product(
        &router,
        "sku-2",
        r#"{"description":"practical relevance with inverted indexes"}"#,
    )
    .await;
    // sku-3 mentions the phrase in a much longer body — lower tf/dl ratio.
    index_product(
        &router,
        "sku-3",
        r#"{"description":"comprehensive guide to the rust search ecosystem including indexing storage and query parsing internals"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"description":"rust search"}},"_source":false}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 2);
    let ids: Vec<String> = hits
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids[0], "sku-1");
    assert_eq!(ids[1], "sku-3");

    let first = hits[0]["_score"].as_f64().expect("score");
    let second = hits[1]["_score"].as_f64().expect("score");
    assert!(first > 0.0);
    assert!(first > second);
    assert_eq!(
        body["hits"]["max_score"].as_f64().expect("max_score"),
        first
    );
}

#[tokio::test]
async fn match_query_uses_index_analyzer_for_keyword_fields() {
    let router = app_router();
    let create_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mappings":{"properties":{"sku":{"type":"keyword"}}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(create_response.status(), StatusCode::OK);

    index_product(&router, "sku-1", r#"{"sku":"alpha road"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"sku":"alpha"}},"_source":false}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 0);
    assert_eq!(body["hits"]["hits"].as_array().expect("hits").len(), 0);
}

#[tokio::test]
async fn bool_must_mixed_match_and_range_uses_index_analyzer_for_keyword_fields() {
    let router = app_router();
    let create_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mappings":{"properties":{"sku":{"type":"keyword"},"price":{"type":"integer"}}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(create_response.status(), StatusCode::OK);

    index_product(&router, "sku-1", r#"{"sku":"alpha road","price":10}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"match":{"sku":"alpha"}},{"range":{"price":{"gte":1}}}]}},"_source":false}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 0);
    assert_eq!(body["hits"]["hits"].as_array().expect("hits").len(), 0);
}

#[tokio::test]
async fn bool_must_mixed_term_and_range_uses_index_analyzer_for_keyword_fields() {
    let router = app_router();
    let create_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mappings":{"properties":{"sku":{"type":"keyword"},"price":{"type":"integer"}}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(create_response.status(), StatusCode::OK);

    index_product(&router, "sku-1", r#"{"sku":"alpha road","price":10}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"term":{"sku":"alpha"}},{"range":{"price":{"gte":1}}}]}},"_source":false}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 0);
    assert_eq!(body["hits"]["hits"].as_array().expect("hits").len(), 0);
}

#[tokio::test]
async fn match_all_query_keeps_max_score_null_and_omits_score_field() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"chair"}"#).await;

    let body = search_with_body(&router, r#"{"query":{"match_all":{}},"_source":false}"#).await;

    assert!(body["hits"]["max_score"].is_null());
    let hits = body["hits"]["hits"].as_array().expect("hits");
    for hit in hits {
        assert!(hit.get("_score").is_none());
    }
}

#[tokio::test]
async fn sort_clause_overrides_default_score_order() {
    let router = app_router();
    index_product(&router, "sku-strong", r#"{"name":"rust rust rust"}"#).await;
    index_product(&router, "sku-weak", r#"{"name":"rust"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":"rust"}},"sort":[{"name":"asc"}],"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-weak", "sku-strong"]);
}

#[tokio::test]
async fn score_sort_clause_orders_by_score() {
    let router = app_router();
    index_product(&router, "sku-strong", r#"{"name":"rust rust rust"}"#).await;
    index_product(&router, "sku-weak", r#"{"name":"rust"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":"rust"}},"sort":[{"_score":"asc"}],"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-weak", "sku-strong"]);
}

#[tokio::test]
async fn match_query_respects_norms_disabled_for_text_fields() {
    let router = app_router();
    let create_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/products")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"mappings":{"properties":{"body":{"type":"text","norms":false}}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(create_response.status(), StatusCode::OK);

    index_product(&router, "short", r#"{"body":"rust"}"#).await;
    index_product(
        &router,
        "long",
        r#"{"body":"rust storage engine search parser"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"body":"rust"}},"sort":[{"_id":"asc"}],"_source":false}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 2);
    let scores = hits
        .iter()
        .map(|hit| hit["_score"].as_f64().expect("score"))
        .collect::<Vec<_>>();
    assert!(
        (scores[0] - scores[1]).abs() < f64::EPSILON,
        "norms disabled should make equal term frequency scores equal: {scores:?}"
    );
}

#[tokio::test]
async fn match_query_defaults_to_or_operator_across_query_tokens() {
    let router = app_router();
    index_product(&router, "sku-rust", r#"{"description":"rust internals"}"#).await;
    index_product(
        &router,
        "sku-search",
        r#"{"description":"search engine indexing"}"#,
    )
    .await;
    index_product(&router, "sku-noop", r#"{"description":"cooking guide"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"description":"rust search"}},"_source":false}"#,
    )
    .await;

    let mut ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["sku-rust", "sku-search"]);
}

#[tokio::test]
async fn match_query_with_and_operator_requires_all_query_tokens() {
    let router = app_router();
    index_product(
        &router,
        "sku-both",
        r#"{"description":"rust search engine"}"#,
    )
    .await;
    index_product(&router, "sku-rust", r#"{"description":"rust internals"}"#).await;
    index_product(&router, "sku-search", r#"{"description":"search engine"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"description":{"query":"rust search","operator":"AND"}}},"_source":false}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-both"]);
}

// ──────────────────────────────────────────────────────────────────────
// A3 — bool.filter / bool.should / minimum_should_match / clause boost.
// Mirrors the matchID deces-backend wire shapes from
// `docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
// (§2.1 and §2.6).
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn bool_filter_restricts_without_changing_score_ordering() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"alpha","category":"a"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"alpha","category":"b"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"match":{"name":"alpha"}}],"filter":[{"term":{"category":"a"}}]}}}"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits array");
    let ids: Vec<&str> = hits
        .iter()
        .map(|h| h["_id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn bool_should_with_minimum_should_match_two_requires_two_matches() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"first":"jean","last":"dupont"}"#).await;
    index_product(&router, "sku-2", r#"{"first":"jean","last":"martin"}"#).await;
    index_product(&router, "sku-3", r#"{"first":"paul","last":"dupont"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"should":[{"match":{"first":"jean"}},{"match":{"last":"dupont"}}],"minimum_should_match":2}},"_source":false}"#,
    )
    .await;

    let ids: Vec<&str> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| h["_id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn bool_should_default_minimum_when_only_should_present_is_one() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"first":"jean","last":"dupont"}"#).await;
    index_product(&router, "sku-2", r#"{"first":"paul","last":"martin"}"#).await;

    // No `minimum_should_match` and no `must`/`filter` → MSM defaults to 1.
    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"should":[{"match":{"first":"jean"}},{"match":{"last":"dupont"}}]}},"_source":false}"#,
    )
    .await;

    let ids: Vec<&str> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| h["_id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn bool_minimum_should_match_percentage_resolves_to_ceiled_integer() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"a":"x","b":"x","c":"y","d":"y"}"#).await;
    index_product(&router, "sku-2", r#"{"a":"x","b":"y","c":"y","d":"y"}"#).await;

    // 4 should-clauses, "50%" → MSM = 2. sku-1 has two matching (a, b),
    // sku-2 has one matching (a).
    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"should":[{"term":{"a":"x"}},{"term":{"b":"x"}},{"term":{"c":"x"}},{"term":{"d":"x"}}],"minimum_should_match":"50%"}},"_source":false}"#,
    )
    .await;

    let ids: Vec<&str> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| h["_id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn bool_clause_boost_scales_inner_score_proportionally() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"alpha"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"alpha beta"}"#).await;

    let baseline = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"match":{"name":"alpha"}}]}}}"#,
    )
    .await;
    let boosted = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"match":{"name":"alpha"}}],"boost":3}}}"#,
    )
    .await;

    let base_top = baseline["hits"]["hits"][0]["_score"]
        .as_f64()
        .expect("baseline _score");
    let boost_top = boosted["hits"]["hits"][0]["_score"]
        .as_f64()
        .expect("boosted _score");

    // 3x boost ≈ 3x score; allow 1 % slack for f64 accumulation.
    let ratio = boost_top / base_top;
    assert!(
        (ratio - 3.0).abs() < 0.03,
        "expected ~3.0 boost ratio, got {ratio}"
    );
}

#[tokio::test]
async fn bool_matchid_nested_should_with_boost_disambiguates_name_order() {
    // Mirrors the canonical matchID `nameQuery(fuzzy=auto)` shape from
    // §2.1 of the DSL inventory: two nested `bool.should` blocks differ
    // only by `boost`, used to prefer the natural "first last" order.
    let router = app_router();
    index_product(&router, "doc-direct", r#"{"first":"jean","last":"dupont"}"#).await;
    index_product(
        &router,
        "doc-swapped",
        r#"{"first":"dupont","last":"jean"}"#,
    )
    .await;

    let body = search_with_body(
        &router,
        r#"{
            "query": {
                "bool": {
                    "minimum_should_match": 1,
                    "should": [
                        {
                            "bool": {
                                "should": [
                                    { "match": { "first": "jean" } },
                                    { "match": { "last": "dupont" } }
                                ],
                                "minimum_should_match": 2,
                                "boost": 2
                            }
                        },
                        {
                            "bool": {
                                "should": [
                                    { "match": { "first": "dupont" } },
                                    { "match": { "last": "jean" } }
                                ],
                                "minimum_should_match": 2,
                                "boost": 0.5
                            }
                        }
                    ]
                }
            }
        }"#,
    )
    .await;

    let hits = body["hits"]["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0]["_id"].as_str().expect("id"), "doc-direct");
    let top_score = hits[0]["_score"].as_f64().expect("top score");
    let next_score = hits[1]["_score"].as_f64().expect("next score");
    // The 2x vs 0.5x boost should keep doc-direct ranked first.
    assert!(top_score > next_score);
}

#[tokio::test]
async fn bool_rejects_empty_body() {
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
                .body(Body::from(r#"{"query":{"bool":{}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
    assert!(body["error"]["reason"]
        .as_str()
        .expect("reason")
        .contains("at least one of"));
}

#[tokio::test]
async fn bool_rejects_negative_boost() {
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
                    r#"{"query":{"bool":{"must":[{"match":{"name":"x"}}],"boost":-1}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
    assert!(body["error"]["reason"]
        .as_str()
        .expect("reason")
        .contains("boost"));
}

#[tokio::test]
async fn bool_filter_alone_is_valid_and_matches_all_filtered() {
    // Mirrors the `geo_distance`-as-filter pattern from §2.6 minus the
    // geo op itself: a `bool.filter` clause restricts the candidate set
    // without contributing to `_score`.
    let router = app_router();
    index_product(&router, "sku-1", r#"{"category":"a"}"#).await;
    index_product(&router, "sku-2", r#"{"category":"b"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"filter":[{"term":{"category":"a"}}]}},"_source":false}"#,
    )
    .await;

    let ids: Vec<&str> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| h["_id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn bool_filter_accepts_object_shorthand() {
    // §2.6 wire shape uses `"filter": { … }` (single object), not the
    // array form. Both must parse.
    let router = app_router();
    index_product(&router, "sku-1", r#"{"category":"a"}"#).await;
    index_product(&router, "sku-2", r#"{"category":"b"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"match_all":{}}],"filter":{"term":{"category":"a"}}}},"_source":false}"#,
    )
    .await;

    let ids: Vec<&str> = body["hits"]["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| h["_id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-1"]);
}

#[tokio::test]
async fn match_query_rejects_unknown_operator_with_opensearch_error() {
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
                    r#"{"query":{"match":{"description":{"query":"rust","operator":"BOTH"}}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
    assert!(body["error"]["reason"]
        .as_str()
        .expect("reason string")
        .contains("operator"));
}

// ---------------------------------------------------------------------------
// A1 — `match` object form with `fuzziness` sub-field.
//
// deces-backend emits `{ "match": { "F": { "query": "JEAN",
// "fuzziness": "AUTO" } } }`. Surch must accept the shape and apply
// bounded Damerau-Levenshtein per analyzed query token, with AUTO →
// edits=1 for terms shorter than 6 characters and edits=2 otherwise.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn match_object_form_with_fuzziness_lowercase_auto_is_accepted() {
    // matchID's deces-backend emits `"fuzziness": "auto"` (lowercase).
    // ES 7.x accepts both casings; Surch must too.
    let router = app_router();
    index_product(&router, "sku-jean", r#"{"name":"JEAN"}"#).await;
    index_product(&router, "sku-other", r#"{"name":"PAUL"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":{"query":"JEAS","fuzziness":"auto"}}}}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 1);
}

#[tokio::test]
async fn match_object_form_with_fuzziness_auto_matches_one_edit_term() {
    let router = app_router();
    // "JEAN" indexed → "JEAS" (single substitution) should still match
    // under AUTO (≤5 chars → edits=1).
    index_product(&router, "sku-jean", r#"{"name":"JEAN"}"#).await;
    index_product(&router, "sku-other", r#"{"name":"PAUL"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":{"query":"JEAS","fuzziness":"AUTO"}}}}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits should be an array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-jean".to_string()]);
}

#[tokio::test]
async fn match_object_form_with_numeric_fuzziness_one_matches_one_edit() {
    let router = app_router();
    // Numeric fuzziness=1 → one substitution permitted.
    index_product(&router, "sku-dupont", r#"{"name":"DUPONT"}"#).await;
    index_product(&router, "sku-martin", r#"{"name":"MARTIN"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":{"query":"DUPONX","fuzziness":1}}}}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits should be an array")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-dupont".to_string()]);
}

#[tokio::test]
async fn match_object_form_with_fuzziness_zero_requires_exact_term() {
    let router = app_router();
    index_product(&router, "sku-jean", r#"{"name":"JEAN"}"#).await;

    // fuzziness="0" → no edits allowed; "JEAS" must not match "JEAN".
    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":{"query":"JEAS","fuzziness":"0"}}}}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 0);
}

#[tokio::test]
async fn match_object_form_without_fuzziness_keeps_default_semantics() {
    let router = app_router();
    index_product(&router, "sku-jean", r#"{"name":"JEAN"}"#).await;
    index_product(&router, "sku-paul", r#"{"name":"PAUL"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":{"query":"JEAN"}}}}"#,
    )
    .await;

    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert_eq!(ids, vec!["sku-jean".to_string()]);
}

// ---------------------------------------------------------------------------
// A8 — `match_all` with optional `boost`.
//
// deces-backend uses `{ "match_all": {} }` as the default query and as
// the must-clause of a geo-filter bool. The empty form keeps the
// existing `_score = null` semantics; the explicit `{ "boost": N }`
// form contributes `N` to the bool-sum so that filter-context callers
// can adjust ranking without writing a `function_score` wrapper.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn match_all_with_boost_object_returns_every_document() {
    let router = app_router();
    index_product(&router, "sku-a", r#"{"name":"alpha"}"#).await;
    index_product(&router, "sku-b", r#"{"name":"beta"}"#).await;

    let body = search_with_body(&router, r#"{"query":{"match_all":{"boost":2.5}}}"#).await;

    assert_eq!(body["hits"]["total"]["value"], 2);
    let ids: Vec<String> = body["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["_id"].as_str().map(str::to_owned).expect("id"))
        .collect();
    assert!(ids.contains(&"sku-a".to_string()));
    assert!(ids.contains(&"sku-b".to_string()));
}

#[tokio::test]
async fn match_all_with_boost_in_bool_must_scales_score() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"rust"}"#).await;

    let body_default = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"match":{"name":"rust"}},{"match_all":{}}]}}}"#,
    )
    .await;
    let body_boosted = search_with_body(
        &router,
        r#"{"query":{"bool":{"must":[{"match":{"name":"rust"}},{"match_all":{"boost":5}}]}}}"#,
    )
    .await;

    // BoolMust sums clause scores → boost=5 adds +5 (vs default +1) to
    // the match clause's BM25 score.
    let s_default = body_default["hits"]["hits"][0]["_score"]
        .as_f64()
        .expect("default score");
    let s_boosted = body_boosted["hits"]["hits"][0]["_score"]
        .as_f64()
        .expect("boosted score");
    let diff = s_boosted - s_default;
    assert!(
        (diff - 4.0).abs() < 1e-6,
        "boost=5 vs default=1 must add 4.0, got diff={diff}"
    );
}

#[tokio::test]
async fn match_all_rejects_unknown_field() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"x"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{"weird":1}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
}

#[tokio::test]
async fn match_all_rejects_negative_boost() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"x"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{"boost":-1}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
}

#[tokio::test]
async fn match_all_rejects_invalid_fuzziness_via_unknown_field_check() {
    // Sanity: the empty body still works after the parser refactor.
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"x"}"#).await;
    let body = search_with_body(&router, r#"{"query":{"match_all":{}}}"#).await;
    assert_eq!(body["hits"]["total"]["value"], 1);
}

#[tokio::test]
async fn match_object_form_rejects_invalid_fuzziness_string() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"JEAN"}"#).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"query":{"match":{"name":{"query":"JEAN","fuzziness":"NOPE"}}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
}

// --- A14: ES-7.x hits.total.{value,relation} wire-shape parity ---
//
// deces-backend reads `hits.total.value` and `hits.total.relation`
// (intake §3). ES 7.x emits `relation = "gte"` when the running total is
// capped by `track_total_hits` and `"eq"` otherwise. Surch already
// implements the shape through `resolve_total_hits`; these tests pin
// down the contract over the public HTTP surface.

#[tokio::test]
async fn search_router_a14_default_track_total_hits_returns_eq_below_cap() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"chair"}"#).await;

    let body = search_with_body(&router, r#"{"query":{"match_all":{}}}"#).await;

    assert_eq!(body["hits"]["total"]["value"], 2);
    assert_eq!(body["hits"]["total"]["relation"], "eq");
}

#[tokio::test]
async fn search_router_a14_track_total_hits_true_returns_eq_relation() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"chair"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"lamp"}"#).await;

    let body =
        search_with_body(&router, r#"{"query":{"match_all":{}},"track_total_hits":true}"#).await;

    assert_eq!(body["hits"]["total"]["value"], 3);
    assert_eq!(body["hits"]["total"]["relation"], "eq");
}

#[tokio::test]
async fn search_router_a14_track_total_hits_caps_value_with_gte_relation() {
    // matchID's wire contract: a numeric `track_total_hits` caps the
    // count and forces `relation = "gte"` once the true total exceeds
    // the limit. Three indexed docs against a limit of 1 forces the cap.
    let router = app_router();
    for index in 0..3 {
        index_product(
            &router,
            &format!("sku-{index}"),
            &format!(r#"{{"name":"item-{index}"}}"#),
        )
        .await;
    }

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"track_total_hits":1,"size":0}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 1);
    assert_eq!(body["hits"]["total"]["relation"], "gte");
}

#[tokio::test]
async fn search_router_a14_total_relation_field_serializes_as_string() {
    // Wire-shape pin: `relation` must serialize as a JSON string, not a
    // number or boolean, so ES-7.x clients (deces-backend) can read it
    // with their existing string parser.
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;

    let body = search_with_body(&router, r#"{"query":{"match_all":{}}}"#).await;

    let relation = body["hits"]["total"]["relation"]
        .as_str()
        .expect("relation must be a string");
    assert!(matches!(relation, "eq" | "gte"));
}

// --- A11: `min_score` top-level body filter ---
//
// matchID's deces-backend sets `min_score` on full-text searches (intake
// §2.8) so weak BM25 hits don't pollute the UI. Surch must drop scored
// hits with `_score < min_score` before pagination, and the response
// `hits.total.value` must reflect the post-filter count.

#[tokio::test]
async fn search_router_a11_min_score_keeps_only_scoring_hits_above_threshold() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk lamp"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"office desk"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"kitchen chair"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":"desk"}},"min_score":1000.0}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 0);
    assert_eq!(body["hits"]["hits"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn search_router_a11_min_score_zero_keeps_every_scored_hit() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk lamp"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"office desk"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match":{"name":"desk"}},"min_score":0.0001}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 2);
}

#[tokio::test]
async fn search_router_a11_min_score_is_ignored_on_unscored_queries() {
    // match_all returns _score = null (no BM25), so `min_score` must
    // not silently drop everything — the filter only applies when
    // scoring is enabled.
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"chair"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"min_score":1000.0}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 2);
}

#[tokio::test]
async fn search_router_a11_min_score_rejects_negative_value() {
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
                    r#"{"query":{"match_all":{}},"min_score":-1.0}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
}

// --- A9: `from` + `size` pagination ---
//
// deces-backend paginates result tables with `from` + `size` (intake
// §2.8). Surch already wires from/size in `paginate_hits` /
// `run_topk_search`; A9 pins the contract and the ES-7.x
// `index.max_result_window = 10 000` cap on `from + size`.

#[tokio::test]
async fn search_router_a9_from_size_returns_requested_slice() {
    let router = app_router();
    for i in 0..5 {
        index_product(&router, &format!("sku-{i}"), r#"{"name":"desk"}"#).await;
    }

    let body =
        search_with_body(&router, r#"{"query":{"match":{"name":"desk"}},"from":0,"size":2}"#)
            .await;
    assert_eq!(body["hits"]["total"]["value"], 5);
    assert_eq!(body["hits"]["hits"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn search_router_a9_from_size_paginates_to_next_page() {
    let router = app_router();
    for i in 0..5 {
        index_product(&router, &format!("sku-{i}"), r#"{"name":"desk"}"#).await;
    }

    let page1 =
        search_with_body(&router, r#"{"query":{"match":{"name":"desk"}},"from":0,"size":2}"#)
            .await;
    let page2 =
        search_with_body(&router, r#"{"query":{"match":{"name":"desk"}},"from":2,"size":2}"#)
            .await;

    let ids = |b: &serde_json::Value| -> Vec<String> {
        b["hits"]["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["_id"].as_str().unwrap().to_string())
            .collect()
    };
    let p1 = ids(&page1);
    let p2 = ids(&page2);
    assert_eq!(p1.len(), 2);
    assert_eq!(p2.len(), 2);
    assert!(
        p1.iter().all(|id| !p2.contains(id)),
        "pages must not overlap: {p1:?} vs {p2:?}"
    );
}

#[tokio::test]
async fn search_router_a9_from_plus_size_above_window_returns_400() {
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
                    r#"{"query":{"match_all":{}},"from":9990,"size":20}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "search_phase_execution_exception");
    assert!(
        body["error"]["reason"]
            .as_str()
            .unwrap()
            .contains("Result window is too large"),
        "got: {}",
        body["error"]["reason"]
    );
}

// --- A10: sort over stored fields + multi-field sub-field aliasing ---
//
// deces-backend sorts UI tables on `DATE_NAISSANCE_NORM` then `NOM.raw`
// (intake §2.8). The `_source` map only carries the parent fields
// today (A13 multi-field mapping isn't shipped yet) — A10 aliases the
// `.raw` / `.norm` sub-field to its parent so the wire shape works
// even before A13 lands; the alias is a no-op once A13 ships real
// keyword normalisers under those sub-paths.

#[tokio::test]
async fn search_router_a10_sort_string_field_ascending() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"banana"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"apple"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"cherry"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"sort":[{"name":"asc"}]}"#,
    )
    .await;

    let names: Vec<String> = body["hits"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["_source"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["apple", "banana", "cherry"]);
}

#[tokio::test]
async fn search_router_a10_sort_string_field_descending() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"banana"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"apple"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"cherry"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"sort":[{"name":"desc"}]}"#,
    )
    .await;

    let names: Vec<String> = body["hits"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["_source"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["cherry", "banana", "apple"]);
}

#[tokio::test]
async fn search_router_a10_sort_subfield_aliases_to_parent() {
    // matchID wire shape uses `NOM.raw`; until A13 ships real
    // multi-fields, Surch aliases the sub-field to its parent so the
    // sort order is deterministic instead of dropping everything to
    // the "missing" bucket.
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"banana"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"apple"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"sort":[{"name.raw":"asc"}]}"#,
    )
    .await;

    let names: Vec<String> = body["hits"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["_source"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["apple", "banana"]);
}

#[tokio::test]
async fn search_router_a10_sort_missing_field_goes_last() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"banana"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"apple"}"#).await;
    index_product(&router, "sku-3", r#"{"other":"no-name"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"sort":[{"name":"asc"}]}"#,
    )
    .await;

    let last_id = body["hits"]["hits"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(last_id, "sku-3", "missing values must sort last regardless of order");
}

// --- A5: `function_score` no-op wrapper ---
//
// matchID's deces-backend wraps every advanced + block-match in
// `function_score` (intake §2.2) even though no scoring functions
// are declared yet. Surch must accept the wrapper, forward to the
// inner query, and apply the optional top-level `boost`.

#[tokio::test]
async fn search_router_a5_function_score_forwards_inner_query() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk lamp"}"#).await;
    index_product(&router, "sku-2", r#"{"name":"office desk"}"#).await;
    index_product(&router, "sku-3", r#"{"name":"chair"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"function_score":{"query":{"match":{"name":"desk"}}}}}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 2);
}

#[tokio::test]
async fn search_router_a5_function_score_accepts_empty_functions_array() {
    let router = app_router();
    index_product(&router, "sku-1", r#"{"name":"desk"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"function_score":{"query":{"match_all":{}},"functions":[],"score_mode":"sum","boost_mode":"multiply"}}}"#,
    )
    .await;

    assert_eq!(body["hits"]["total"]["value"], 1);
}

#[tokio::test]
async fn search_router_a5_function_score_rejects_non_empty_functions() {
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
                    r#"{"query":{"function_score":{"query":{"match_all":{}},"functions":[{"weight":2}]}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
    assert!(body["error"]["reason"]
        .as_str()
        .unwrap()
        .contains("not implemented yet"));
}

// --- A12.1: `terms` aggregation MVP ---
//
// matchID's analytics tab (intake §2.10) expects `aggs.<name>.terms`
// to return a `{ buckets: [{ key, doc_count }, …] }` payload, ordered
// by descending doc_count with key ascending as the tiebreak. Only
// `terms` is honoured today; `date_histogram`, `composite` and
// `cardinality` are tracked under A12 phase 2.

#[tokio::test]
async fn search_router_a12_terms_aggregation_returns_bucket_counts() {
    let router = app_router();
    index_product(&router, "doc-1", r#"{"NOM":"MARTIN"}"#).await;
    index_product(&router, "doc-2", r#"{"NOM":"MARTIN"}"#).await;
    index_product(&router, "doc-3", r#"{"NOM":"DUPONT"}"#).await;
    index_product(&router, "doc-4", r#"{"NOM":"DUPONT"}"#).await;
    index_product(&router, "doc-5", r#"{"NOM":"BERNARD"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"size":0,"aggs":{"by_nom":{"terms":{"field":"NOM"}}}}"#,
    )
    .await;

    let buckets = body["aggregations"]["by_nom"]["buckets"]
        .as_array()
        .expect("buckets array");
    assert_eq!(buckets.len(), 3);
    // doc_count desc tie-broken by key asc -> DUPONT before MARTIN.
    assert_eq!(buckets[0]["key"], "DUPONT");
    assert_eq!(buckets[0]["doc_count"], 2);
    assert_eq!(buckets[1]["key"], "MARTIN");
    assert_eq!(buckets[1]["doc_count"], 2);
    assert_eq!(buckets[2]["key"], "BERNARD");
    assert_eq!(buckets[2]["doc_count"], 1);
}

#[tokio::test]
async fn search_router_a12_terms_aggregation_caps_buckets_to_size() {
    let router = app_router();
    index_product(&router, "doc-1", r#"{"NOM":"MARTIN"}"#).await;
    index_product(&router, "doc-2", r#"{"NOM":"MARTIN"}"#).await;
    index_product(&router, "doc-3", r#"{"NOM":"MARTIN"}"#).await;
    index_product(&router, "doc-4", r#"{"NOM":"DUPONT"}"#).await;
    index_product(&router, "doc-5", r#"{"NOM":"DUPONT"}"#).await;
    index_product(&router, "doc-6", r#"{"NOM":"BERNARD"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"size":0,"aggs":{"by_nom":{"terms":{"field":"NOM","size":2}}}}"#,
    )
    .await;

    let buckets = body["aggregations"]["by_nom"]["buckets"]
        .as_array()
        .expect("buckets array");
    assert_eq!(buckets.len(), 2, "size=2 must cap the bucket list");
    assert_eq!(buckets[0]["key"], "MARTIN");
    assert_eq!(buckets[0]["doc_count"], 3);
    assert_eq!(buckets[1]["key"], "DUPONT");
    assert_eq!(buckets[1]["doc_count"], 2);
}

#[tokio::test]
async fn search_router_a12_terms_aggregation_subfield_aliases_to_parent() {
    // matchID emits `NOM.raw` because its mapping declares
    // `NOM: { type: text, fields: { raw: { type: keyword } } }`. Until
    // A13 ships real multi-fields, Surch aliases the sub-field back to
    // its parent (same alias as A10 sort) so the aggregation produces
    // deterministic buckets instead of an empty payload.
    let router = app_router();
    index_product(&router, "doc-1", r#"{"NOM":"MARTIN"}"#).await;
    index_product(&router, "doc-2", r#"{"NOM":"MARTIN"}"#).await;
    index_product(&router, "doc-3", r#"{"NOM":"DUPONT"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"size":0,"aggs":{"by_nom":{"terms":{"field":"NOM.raw","size":100}}}}"#,
    )
    .await;

    let buckets = body["aggregations"]["by_nom"]["buckets"]
        .as_array()
        .expect("buckets array");
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0]["key"], "MARTIN");
    assert_eq!(buckets[0]["doc_count"], 2);
    assert_eq!(buckets[1]["key"], "DUPONT");
    assert_eq!(buckets[1]["doc_count"], 1);
}

#[tokio::test]
async fn search_router_a12_rejects_unsupported_agg_type_with_phase2_hint() {
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
                    r#"{"query":{"match_all":{}},"aggs":{"by_month":{"date_histogram":{"field":"DATE","calendar_interval":"month"}}}}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
    let reason = body["error"]["reason"].as_str().unwrap();
    assert!(reason.contains("date_histogram"));
    assert!(reason.contains("A12 phase 2"));
}

#[tokio::test]
async fn search_router_a12_accepts_aggregations_long_form_alias() {
    // ES accepts both `aggs` and `aggregations`; matchID's analytics
    // tab emits the long form.
    let router = app_router();
    index_product(&router, "doc-1", r#"{"NOM":"MARTIN"}"#).await;
    index_product(&router, "doc-2", r#"{"NOM":"DUPONT"}"#).await;

    let body = search_with_body(
        &router,
        r#"{"query":{"match_all":{}},"size":0,"aggregations":{"by_nom":{"terms":{"field":"NOM"}}}}"#,
    )
    .await;

    let buckets = body["aggregations"]["by_nom"]["buckets"]
        .as_array()
        .expect("buckets array");
    assert_eq!(buckets.len(), 2);
}
