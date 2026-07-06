//! Plan segments S2 gate
//! (`docs/paper/design-segments-pic-borne-2026-07-05.md`): flush-by-budget
//! must produce REAL multi-segment indexing (`DocumentIndex::segments.len()
//! > 1`) while staying BIT-IDENTICAL — same doc-id sets, same BM25 scores,
//! same ranking order — to the mono-segment engine (`SURCH_FLUSH_BUDGET_BYTES`
//! unset, the S1 reversibility flag).
//!
//! Testability note (mirrors `tests/postings_disk_parity.rs`): the budget is
//! normally a process-wide `OnceLock`-cached env var, which cannot flip
//! mid-process. This test never touches the env var; it uses
//! [`AppState::set_flush_budget_bytes_override`], a per-index override that
//! bypasses the `OnceLock` entirely, to build one index with a forced tiny
//! budget (real multi-segment) and a second, independent index with the
//! override forced to `None` (mono-segment), in the SAME process, and
//! compares their `_search` responses.
//!
//! The corpus is deliberately sent as SEVERAL `_bulk` chunks (not one): the
//! budget check runs once per chunk
//! (`InMemoryIndex::append_to_index` -> `DocumentIndex::maybe_flush_by_budget`),
//! so a single giant chunk would only ever seal at most one segment boundary.

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::Value;
use surch_api::{app_router_with_state, state::AppState, AppRouterState};
use tower::ServiceExt;

async fn response_json(response: axum::response::Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be json")
}

/// Deterministic corpus, split into `CHUNKS` separate `_bulk` NDJSON
/// payloads of `PER_CHUNK` docs each — so the budget check fires once per
/// chunk, not once for the whole corpus. 3 `title` groups (`widget` spans
/// most docs, multi-FoR-block; `gadget` a minority), 2 `category` groups, a
/// `body` term (`common`) shared by EVERY doc.
const CHUNKS: usize = 12;
const PER_CHUNK: usize = 25;

fn corpus_chunks() -> Vec<String> {
    let mut chunks = Vec::with_capacity(CHUNKS);
    let mut doc_id = 0usize;
    for _ in 0..CHUNKS {
        let mut out = String::new();
        for _ in 0..PER_CHUNK {
            let title = match doc_id % 3 {
                0 => "alpha widget",
                1 => "beta widget",
                _ => "gamma gadget",
            };
            let category = if doc_id % 2 == 0 { "tools" } else { "toys" };
            out.push_str(&format!(
                "{{\"index\":{{\"_id\":\"{doc_id}\"}}}}\n\
                 {{\"title\":\"{title}\",\"category\":\"{category}\",\"body\":\"common lorem ipsum doc{doc_id}\"}}\n"
            ));
            doc_id += 1;
        }
        chunks.push(out);
    }
    chunks
}

/// Build a fresh `AppState`-backed router, pin `index`'s flush-by-budget
/// override BEFORE any document lands (mirrors
/// `postings_disk_parity.rs::build_index`'s ordering rationale — the
/// override must be set before the first `PostingsBuilder`-consuming call),
/// then POST the corpus as `CHUNKS` separate `_bulk` requests and refresh
/// once at the end.
async fn build_index(index: &str, forced_budget_bytes: Option<u64>) -> (axum::Router, AppState) {
    let app_state = AppState::default();
    app_state.create_index(index, None, serde_json::json!({}), Default::default());
    app_state.set_flush_budget_bytes_override(index, forced_budget_bytes);

    let router = app_router_with_state(AppRouterState {
        app: app_state.clone(),
        ..Default::default()
    });

    for chunk in corpus_chunks() {
        let bulk_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/{index}/_bulk"))
                    .header("content-type", "application/x-ndjson")
                    .body(Body::from(chunk))
                    .expect("bulk request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(bulk_response.status(), StatusCode::OK);
        let bulk_body = response_json(bulk_response).await;
        assert_eq!(
            bulk_body["errors"], false,
            "bulk indexing must succeed (forced_budget_bytes={forced_budget_bytes:?}): {bulk_body:?}"
        );
    }

    // Explicit `_refresh` so the same production `_bulk` (x N) -> `_refresh`
    // -> `_search` sequence real usage follows is exercised here too — and,
    // per the design, `_refresh` ALSO seals the active segment as one more
    // boundary when a budget is configured.
    let refresh_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_refresh"))
                .body(Body::empty())
                .expect("refresh request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(refresh_response.status(), StatusCode::OK);

    (router, app_state)
}

async fn search(router: &axum::Router, index: &str, query_body: &str) -> Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_search"))
                .header("content-type", "application/json")
                .body(Body::from(query_body.to_owned()))
                .expect("search request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "query {query_body} failed"
    );
    response_json(response).await
}

/// `(id, score)` pairs in response order — order matters, not just set
/// membership, since a scoring divergence between the mono- and
/// multi-segment paths could also reorder ties.
fn extract_ordered_hits(body: &Value) -> Vec<(String, f64)> {
    body["hits"]["hits"]
        .as_array()
        .expect("hits should be an array")
        .iter()
        .map(|hit| {
            let id = hit["_id"].as_str().expect("hit must carry _id").to_owned();
            let score = hit["_score"].as_f64().unwrap_or(0.0);
            (id, score)
        })
        .collect()
}

#[tokio::test]
async fn budget_flush_forces_multi_segment_and_matches_mono_segment_bit_identical() {
    let index = "seg-flush-parity-idx";

    // `Some(1)`: any non-empty `PostingsBuilder` (>= 1 byte) crosses the
    // budget, so every one of the `CHUNKS` `_bulk` POSTs seals its own
    // segment — the strongest possible exercise of real multi-segment
    // indexing. `None`: forces "no budget" regardless of the process env,
    // reproducing the S1 mono-segment engine exactly.
    let (router_multi, app_state_multi) = build_index(index, Some(1)).await;
    let (router_mono, app_state_mono) = build_index(index, None).await;

    assert_eq!(
        app_state_mono.index_segment_count(index),
        1,
        "budget-unset (forced None) index must stay mono-segment — the S1 invariant"
    );
    assert!(
        app_state_multi.index_segment_count(index) > 1,
        "a budget of 1 byte across {CHUNKS} separate `_bulk` chunks must have sealed \
         more than one segment (got {})",
        app_state_multi.index_segment_count(index)
    );

    let total = CHUNKS * PER_CHUNK;
    let queries: &[(&str, &str)] = &[
        (
            "single-token match (maxscore OR-match, common multi-block term)",
            r#"{"query":{"match":{"body":"common"}},"size":400}"#,
        ),
        (
            "single-token match (maxscore OR-match, split term)",
            r#"{"query":{"match":{"title":"widget"}},"size":400}"#,
        ),
        (
            "2-clause bool.must single-token conjunction (conjunction_hits_internal)",
            r#"{"query":{"bool":{"must":[{"match":{"title":"widget"}},{"match":{"category":"tools"}}]}},"size":400}"#,
        ),
        (
            "3-clause bool.must single-token conjunction",
            r#"{"query":{"bool":{"must":[{"match":{"title":"widget"}},{"match":{"category":"tools"}},{"match":{"body":"common"}}]}},"size":400}"#,
        ),
        (
            "multi-token bool.must clause (conjunction_of_matches)",
            r#"{"query":{"bool":{"must":[{"match":{"title":"alpha widget"}},{"match":{"category":"tools"}}]}},"size":400}"#,
        ),
        (
            "should-all-required conjunction (fused_conjunction_scores)",
            r#"{"query":{"bool":{"should":[{"match":{"title":"widget"}},{"match":{"category":"tools"}}],"minimum_should_match":2}},"size":400}"#,
        ),
        (
            "match_all (baseline recall/order sanity check)",
            r#"{"query":{"match_all":{}},"size":400}"#,
        ),
    ];

    for &(label, query_body) in queries {
        let body_mono = search(&router_mono, index, query_body).await;
        let body_multi = search(&router_multi, index, query_body).await;

        assert_eq!(
            body_mono["hits"]["total"]["value"], body_multi["hits"]["total"]["value"],
            "[{label}] total hits diverged between mono- and multi-segment for {query_body}"
        );

        let hits_mono = extract_ordered_hits(&body_mono);
        let hits_multi = extract_ordered_hits(&body_multi);
        assert_eq!(
            hits_mono, hits_multi,
            "[{label}] (id, score) hits diverged between mono- and multi-segment for {query_body}"
        );
    }

    // `match_all` at `size` >= corpus recovers every doc: an independent
    // sanity check that BOTH engines actually indexed the full corpus (not
    // just agreeing on an accidentally-truncated candidate set).
    let body_all = search(
        &router_mono,
        index,
        r#"{"query":{"match_all":{}},"size":10000}"#,
    )
    .await;
    assert_eq!(
        body_all["hits"]["total"]["value"].as_u64(),
        Some(total as u64),
        "expected the full corpus to be indexed"
    );
}
