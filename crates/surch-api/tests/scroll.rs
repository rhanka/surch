//! Integration tests for the `_search?scroll=…` / `POST /_search/scroll`
//! lifecycle (matchID intake batch §2.9).

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::Value;
use surch_api::app_router;
use tower::ServiceExt;

async fn response_json(response: axum::response::Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be JSON")
}

/// Index 5 documents under `docs/_doc/<id>` so we can paginate a known
/// fixture corpus by 2-row pages.
async fn seed_five_docs(router: &axum::Router) {
    for (id, name) in [
        ("d1", "alpha"),
        ("d2", "beta"),
        ("d3", "gamma"),
        ("d4", "delta"),
        ("d5", "epsilon"),
    ] {
        let body = format!(r#"{{"name":"{name}"}}"#);
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/docs/_doc/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(response.status(), StatusCode::CREATED, "doc {id} indexed");
    }
}

#[tokio::test]
async fn search_with_scroll_returns_scroll_id_and_first_page() {
    let router = app_router();
    seed_five_docs(&router).await;

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/docs/_search?scroll=1m")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}},"size":2}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let scroll_id = body
        .get("_scroll_id")
        .and_then(Value::as_str)
        .expect("first page should expose _scroll_id");
    assert!(!scroll_id.is_empty(), "scroll id should not be empty");

    let hits = body["hits"]["hits"]
        .as_array()
        .expect("hits should be an array");
    assert_eq!(hits.len(), 2, "first page returns size=2 hits");
    assert_eq!(
        body["hits"]["total"]["value"].as_u64(),
        Some(5),
        "total reflects the full corpus"
    );
}

#[tokio::test]
async fn scroll_followup_returns_next_page_and_eventually_empty() {
    let router = app_router();
    seed_five_docs(&router).await;

    let initial = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/docs/_search?scroll=1m")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match_all":{}},"size":2}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(initial.status(), StatusCode::OK);
    let initial_body = response_json(initial).await;
    let mut scroll_id = initial_body["_scroll_id"]
        .as_str()
        .expect("first page should expose _scroll_id")
        .to_string();
    let mut seen = initial_body["hits"]["hits"]
        .as_array()
        .expect("hits should be an array")
        .len();

    // Second page — should contain the next 2 docs and a fresh scroll id.
    let next_body = format!(r#"{{"scroll":"1m","scroll_id":"{scroll_id}"}}"#);
    let next = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_search/scroll")
                .header("content-type", "application/json")
                .body(Body::from(next_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(next.status(), StatusCode::OK);
    let next_json = response_json(next).await;
    let next_hits = next_json["hits"]["hits"]
        .as_array()
        .expect("hits should be an array");
    assert_eq!(next_hits.len(), 2, "second page returns the next 2 docs");
    seen += next_hits.len();
    scroll_id = next_json["_scroll_id"]
        .as_str()
        .expect("non-empty page should keep returning a scroll id")
        .to_string();

    // Third page — should be the trailing single doc.
    let third_body = format!(r#"{{"scroll":"1m","scroll_id":"{scroll_id}"}}"#);
    let third = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_search/scroll")
                .header("content-type", "application/json")
                .body(Body::from(third_body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(third.status(), StatusCode::OK);
    let third_json = response_json(third).await;
    let third_hits = third_json["hits"]["hits"]
        .as_array()
        .expect("hits should be an array");
    assert_eq!(third_hits.len(), 1, "third page returns the tail doc");
    seen += third_hits.len();
    assert_eq!(seen, 5, "scroll iterates the full corpus exactly once");

    // The third page exhausted the corpus; the scroll context is not
    // reinstalled, so a follow-up scroll with its id is treated as a
    // missing context. We accept either the new id (empty next page)
    // or — when the cursor sat exactly on `total` — its absence.
    if let Some(tail_id) = third_json["_scroll_id"].as_str() {
        let tail_body = format!(r#"{{"scroll":"1m","scroll_id":"{tail_id}"}}"#);
        let tail = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/_search/scroll")
                    .header("content-type", "application/json")
                    .body(Body::from(tail_body))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(tail.status(), StatusCode::OK);
        let tail_json = response_json(tail).await;
        assert_eq!(
            tail_json["hits"]["hits"]
                .as_array()
                .expect("hits should be an array")
                .len(),
            0,
            "scroll past the tail returns an empty page"
        );
    }
}

#[tokio::test]
async fn scroll_unknown_id_returns_404() {
    let router = app_router();
    seed_five_docs(&router).await;

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_search/scroll")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"scroll":"1m","scroll_id":"deadbeefdeadbeefdeadbeefdeadbeef"}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response_json(response).await;
    assert_eq!(
        body["error"]["type"].as_str(),
        Some("search_context_missing_exception"),
        "OpenSearch-shaped error envelope"
    );
}
