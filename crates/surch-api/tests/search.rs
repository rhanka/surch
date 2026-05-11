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
