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

/// Guards Track A `wp-a-perf-followups.md` Lot 1: sequential `_bulk`
/// POSTs against the same index must accumulate without re-indexing
/// the cumulative document store at each chunk. The shape mirrors
/// `scripts/bench/trec-covid-ndcg.sh` which splits the BEIR corpus
/// into ~21 chunks and POSTs them one by one before a single
/// `_refresh`. All previously-ingested docs must remain searchable
/// after each subsequent chunk lands.
#[tokio::test]
async fn bulk_router_accumulates_across_multiple_chunks() {
    let router = app_router();

    const CHUNKS: usize = 3;
    const PER_CHUNK: usize = 80;

    for chunk in 0..CHUNKS {
        let mut ndjson = String::new();
        for i in 0..PER_CHUNK {
            let doc_id = chunk * PER_CHUNK + i;
            // `uniquetermNNN` is a single token per doc and per chunk so a
            // targeted search can isolate exactly one doc later.
            ndjson.push_str(&format!(
                "{{\"index\":{{\"_id\":\"{doc_id}\"}}}}\n{{\"title\":\"uniqueterm{doc_id}\"}}\n"
            ));
        }

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/catalog/_bulk")
                    .header("content-type", "application/x-ndjson")
                    .body(Body::from(ndjson))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["errors"], false, "chunk {chunk} surfaced errors");
        assert_eq!(
            body["items"].as_array().expect("items array").len(),
            PER_CHUNK,
            "chunk {chunk} item count"
        );

        let search_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/catalog/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"query":{{"match_all":{{}}}},"size":{}}}"#,
                        (chunk + 1) * PER_CHUNK
                    )))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(search_response.status(), StatusCode::OK);
        let search_body = response_json(search_response).await;
        let expected_total = ((chunk + 1) * PER_CHUNK) as u64;
        assert_eq!(
            search_body["hits"]["total"]["value"].as_u64(),
            Some(expected_total),
            "search after chunk {chunk} should see {expected_total} docs"
        );
    }

    // After all chunks, an `_mget` for a doc that landed in the FIRST
    // chunk must still return its source — guards that the incremental
    // append path keeps earlier doc state addressable after later
    // chunks landed.
    let by_id = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_mget")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"docs":[{"_index":"catalog","_id":"5"}]}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(by_id.status(), StatusCode::OK);
    let by_id_body = response_json(by_id).await;
    assert_eq!(by_id_body["docs"][0]["_id"], "5");
    assert_eq!(by_id_body["docs"][0]["found"], true);
    assert_eq!(by_id_body["docs"][0]["_source"]["title"], "uniqueterm5");
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
