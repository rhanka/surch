use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use surch_api::{app_router, app_router_with_state, state::AppState, AppRouterState};
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

/// Guards Track A `wp-a-perf-followups.md` Lot 1.5: when a caller
/// issues `_bulk` -> `_refresh` -> `_bulk` -> `_search`, the search
/// must see BOTH the pre-refresh and post-refresh docs. The refresh
/// drops the in-memory `PostingsBuilder` snapshot (recovers ~1 GiB
/// on long-text corpora), so the second bulk falls back to a
/// one-shot full rebuild that re-includes the earlier docs.
#[tokio::test]
async fn bulk_router_bulk_refresh_bulk_search_preserves_old_docs() {
    let router = app_router();

    // First batch: 3 docs.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"index":{"_id":"a1"}}
{"title":"alphaone"}
{"index":{"_id":"a2"}}
{"title":"alphatwo"}
{"index":{"_id":"a3"}}
{"title":"alphathree"}
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);

    // Refresh: drops the PostingsBuilder snapshot.
    let refresh = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_refresh")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(refresh.status(), StatusCode::OK);

    // Second batch: 2 fresh docs after refresh.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"index":{"_id":"b1"}}
{"title":"betaone"}
{"index":{"_id":"b2"}}
{"title":"betatwo"}
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);

    // match_all sees both batches (cumulative).
    let search = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}},"size":20}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(search.status(), StatusCode::OK);
    let body = response_json(search).await;
    assert_eq!(
        body["hits"]["total"]["value"].as_u64(),
        Some(5),
        "cumulative match_all must see pre-refresh + post-refresh docs"
    );

    // Targeted search for a pre-refresh term: postings must still be
    // present after the post-refresh rebuild.
    let pre_refresh = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match":{"title":"alphatwo"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let pre_body = response_json(pre_refresh).await;
    assert_eq!(
        pre_body["hits"]["total"]["value"].as_u64(),
        Some(1),
        "pre-refresh unique term must survive the second bulk"
    );
    assert_eq!(pre_body["hits"]["hits"][0]["_id"], "a2");

    // Targeted search for a post-refresh term too.
    let post_refresh = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match":{"title":"betaone"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let post_body = response_json(post_refresh).await;
    assert_eq!(
        post_body["hits"]["total"]["value"].as_u64(),
        Some(1),
        "post-refresh unique term must be searchable"
    );
    assert_eq!(post_body["hits"]["hits"][0]["_id"], "b1");
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

/// Guards Track A `wp-a-perf-followups.md` Lot 1.6: N sequential
/// `_bulk` POSTs followed by a single `_refresh` must trigger ONE
/// FST rebuild — not N. The previous behaviour rebuilt the term
/// dictionary on every `_bulk` chunk via
/// `DocumentIndex::add_documents_with_mapping`, which made the bulk
/// path quadratic on cumulative terms and accounted for most of the
/// Surch / OpenSearch gap on TREC-COVID. Lot 1.6 defers the rebuild
/// to the next read-after-write boundary (search or refresh), so
/// the `terms_build_count` instrumentation can prove the new
/// schedule.
#[tokio::test]
async fn bulk_router_lot1_6_defers_terms_build_across_chunks() {
    let shared = AppRouterState::default();
    let app: AppState = shared.app.clone();
    let router = app_router_with_state(shared);

    const CHUNKS: usize = 5;
    const PER_CHUNK: usize = 50;

    // Baseline: untouched index has zero rebuilds. We assert the
    // counter from `AppState::index_terms_build_count`, which is a
    // per-`InMemoryIndex` counter (no cross-test pollution).
    assert_eq!(app.index_terms_build_count("catalog"), 0);

    for chunk in 0..CHUNKS {
        let mut ndjson = String::new();
        for i in 0..PER_CHUNK {
            let doc_id = chunk * PER_CHUNK + i;
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
    }

    // After CHUNKS bulks WITHOUT any intervening search or refresh,
    // the FST has not been rebuilt — the writes are still parked in
    // the `PostingsBuilder` snapshot.
    assert_eq!(
        app.index_terms_build_count("catalog"),
        0,
        "deferred build invariant: {CHUNKS} `_bulk` chunks should not rebuild the FST \
         before the first read-after-write boundary",
    );

    // A `_refresh` materializes the FST exactly once (via
    // `IndexData::finalize_terms_for_refresh`) before freeing the
    // `PostingsBuilder` snapshot.
    let refresh = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_refresh")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(refresh.status(), StatusCode::OK);

    assert_eq!(
        app.index_terms_build_count("catalog"),
        1,
        "`_refresh` after {CHUNKS} bulks should rebuild the FST exactly once",
    );

    // A subsequent `_search` does not re-rebuild — `terms_dirty` is
    // already clear, so `ensure_terms_ready` is a no-op.
    let search = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"query":{{"match_all":{{}}}},"size":{}}}"#,
                    CHUNKS * PER_CHUNK
                )))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(search.status(), StatusCode::OK);
    let search_body = response_json(search).await;
    assert_eq!(
        search_body["hits"]["total"]["value"].as_u64(),
        Some((CHUNKS * PER_CHUNK) as u64),
        "every bulked doc must surface in the post-refresh search"
    );
    assert_eq!(
        app.index_terms_build_count("catalog"),
        1,
        "a clean search after refresh must not retrigger the FST rebuild",
    );
}

/// Guards Track A `wp-a-perf-followups.md` Lot 1.6: when a search
/// arrives between two `_bulk` POSTs (no `_refresh`), the search
/// MUST materialize the deferred FST so the new docs are visible.
/// The second bulk also defers, so a follow-up search materializes
/// again — but never more than once per quiet→write→read cycle.
#[tokio::test]
async fn bulk_router_lot1_6_search_between_bulks_materializes_once_per_cycle() {
    let shared = AppRouterState::default();
    let app: AppState = shared.app.clone();
    let router = app_router_with_state(shared);

    // First bulk → defer.
    let _ = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"index":{"_id":"1"}}
{"title":"alpha"}
{"index":{"_id":"2"}}
{"title":"beta"}
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(app.index_terms_build_count("catalog"), 0);

    // Search → materialize (count = 1).
    let _ = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match":{"title":"alpha"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(app.index_terms_build_count("catalog"), 1);

    // Quiet second search → no extra materialize.
    let _ = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match":{"title":"alpha"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(app.index_terms_build_count("catalog"), 1);

    // Second bulk → defer again.
    let _ = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(
                    r#"{"index":{"_id":"3"}}
{"title":"gamma"}
"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(app.index_terms_build_count("catalog"), 1);

    // Search after second bulk → materialize again (count = 2).
    let search = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/catalog/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match":{"title":"gamma"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let body = response_json(search).await;
    assert_eq!(
        body["hits"]["total"]["value"].as_u64(),
        Some(1),
        "post-bulk search must see the freshly-bulked `gamma` doc",
    );
    assert_eq!(
        app.index_terms_build_count("catalog"),
        2,
        "second write→read cycle must trigger exactly one extra FST rebuild",
    );
}
