//! C1 fix (design doc `docs/paper/design-segments-pic-borne-2026-07-05.md`
//! §C1) gate: budget-triggered incremental `densify` (drains the id-maps
//! overlay — `forward_dirty`/`reverse_dirty`/`documents_dirty`/
//! `deleted_since_dense` — mid-bulk, by tranche, instead of holding the
//! WHOLE corpus in the overlay until a single final `_refresh`) must stay
//! BIT-IDENTICAL to the pre-fix behaviour (budget unset: overlay only
//! drained at an explicit `_refresh`, via the unchanged `densify_full`
//! path).
//!
//! Testability note (mirrors `tests/segment_flush_budget_parity.rs`): the
//! budget is normally a process-wide `OnceLock`-cached env var, which
//! cannot flip mid-process. This test never touches the env var; it uses
//! [`AppState::set_densify_budget_docs_override`], a per-index override
//! that bypasses the `OnceLock` entirely, to build one index with a
//! forced tiny budget (real incremental densify, triggered repeatedly
//! across the bulk) and a second, independent index with the override
//! forced to `None` (today's behaviour: overlay only drained at
//! `_refresh`), in the SAME process, and compares their `_search`/
//! `_mget`/`_count` responses.
//!
//! The corpus is sent as SEVERAL `_bulk` chunks (not one): the budget
//! check runs once per chunk (`InMemoryIndex::append_to_index` ->
//! `InMemoryIndex::maybe_densify_by_budget`), so a single giant chunk
//! would only ever cross the budget once, at best.

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

/// Deterministic append-only corpus, split into `CHUNKS` separate `_bulk`
/// NDJSON payloads of `PER_CHUNK` docs each — sequential integer `_id`s,
/// mirroring the real INSEE deces bench harness's id scheme
/// (`deploy/bench-local/fair-ab.sh`: `_id = NR`).
const CHUNKS: usize = 20;
const PER_CHUNK: usize = 15;

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
            out.push_str(&format!(
                "{{\"index\":{{\"_id\":\"{doc_id}\"}}}}\n\
                 {{\"title\":\"{title}\",\"body\":\"common lorem ipsum doc{doc_id}\"}}\n"
            ));
            doc_id += 1;
        }
        chunks.push(out);
    }
    chunks
}

async fn post_bulk(router: &axum::Router, index: &str, ndjson: String) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_bulk"))
                .header("content-type", "application/x-ndjson")
                .body(Body::from(ndjson))
                .expect("bulk request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body["errors"], false,
        "bulk indexing must succeed: {body:?}"
    );
}

async fn refresh(router: &axum::Router, index: &str) {
    let response = router
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
    assert_eq!(response.status(), StatusCode::OK);
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
                .expect("request should build"),
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

async fn mget(router: &axum::Router, index: &str, ids: &[String]) -> Value {
    let ids_json = serde_json::to_string(ids).expect("ids should serialise");
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_mget"))
                .header("content-type", "application/json")
                .body(Body::from(format!("{{\"ids\":{ids_json}}}")))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn count(router: &axum::Router, index: &str) -> u64 {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/{index}/_count"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await["count"]
        .as_u64()
        .expect("count should be a number")
}

/// `(id, score)` pairs in response order — order matters, not just set
/// membership.
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

/// Build a fresh `AppState`-backed router, pin `index`'s densify-budget
/// override BEFORE any document lands, POST the append-only corpus as
/// `CHUNKS` separate `_bulk` requests, then `_refresh` once at the end
/// (exactly the fair-ab bench's "gros bulk puis un seul refresh final"
/// flow the design doc's §C1 diagnostic targets).
async fn build_index(index: &str, forced_budget_docs: Option<u64>) -> (axum::Router, AppState) {
    let app_state = AppState::default();
    app_state.create_index(index, None, serde_json::json!({}), Default::default());
    app_state.set_densify_budget_docs_override(index, forced_budget_docs);

    let router = app_router_with_state(AppRouterState {
        app: app_state.clone(),
        ..Default::default()
    });

    for chunk in corpus_chunks() {
        post_bulk(&router, index, chunk).await;
    }
    refresh(&router, index).await;

    (router, app_state)
}

#[tokio::test]
async fn budget_densify_matches_refresh_only_densify_after_multi_chunk_bulk() {
    let index = "densify-budget-parity-idx";

    // `Some(4)`: the budget check runs once per `_bulk` chunk boundary
    // (`InMemoryIndex::append_to_index`), and `PER_CHUNK = 15` always
    // crosses it, so the incremental fast path
    // (`InMemoryIndex::densify_append_only`) fires on EVERY chunk across
    // the whole bulk, each time on top of whatever the previous tranche
    // already folded in. `None`: overlay only drained by the final
    // `_refresh` (today's behaviour, reproduced exactly).
    let (router_budget, _state_budget) = build_index(index, Some(4)).await;
    let (router_no_budget, _state_no_budget) = build_index(index, None).await;

    let total = CHUNKS * PER_CHUNK;
    assert_eq!(count(&router_budget, index).await, total as u64);
    assert_eq!(count(&router_no_budget, index).await, total as u64);

    let queries: &[(&str, &str)] = &[
        (
            "single-token match (maxscore OR-match, common multi-block term)",
            r#"{"query":{"match":{"body":"common"}},"size":400}"#,
        ),
        (
            "single-token match (split term)",
            r#"{"query":{"match":{"title":"widget"}},"size":400}"#,
        ),
        (
            "match_all (baseline recall/order sanity check)",
            r#"{"query":{"match_all":{}},"size":400}"#,
        ),
    ];
    for &(label, query_body) in queries {
        let body_budget = search(&router_budget, index, query_body).await;
        let body_no_budget = search(&router_no_budget, index, query_body).await;
        assert_eq!(
            body_budget["hits"]["total"]["value"],
            body_no_budget["hits"]["total"]["value"],
            "[{label}] total hits diverged between budgeted and refresh-only densify for {query_body}"
        );
        assert_eq!(
            extract_ordered_hits(&body_budget),
            extract_ordered_hits(&body_no_budget),
            "[{label}] (id, score) hits diverged between budgeted and refresh-only densify for {query_body}"
        );
    }

    // `_mget` on every id (including a few misses) exercises
    // `resolve_uid`/`blob_for_doc_id` through the DENSE snapshot produced
    // by repeated incremental appends — must resolve every uid to the
    // exact same `_source` as the refresh-only baseline.
    let mut ids: Vec<String> = (0..total).map(|i| i.to_string()).collect();
    ids.push("missing-id".to_owned());
    let mget_budget = mget(&router_budget, index, &ids).await;
    let mget_no_budget = mget(&router_no_budget, index, &ids).await;
    assert_eq!(
        mget_budget, mget_no_budget,
        "_mget diverged between budgeted and refresh-only densify"
    );
}

/// Exercises `InMemoryIndex::densify`'s interference detector: an update
/// AND a delete against a doc_id that the incremental fast path has
/// ALREADY densified (from an earlier budget-triggered tranche) must
/// route the next densify to the full from-scratch fallback
/// (`densify_full`) — and still produce correct results, not a stale or
/// resurrected document.
#[tokio::test]
async fn budget_densify_falls_back_correctly_across_update_and_delete() {
    let index = "densify-budget-interference-idx";
    let app_state = AppState::default();
    app_state.create_index(index, None, serde_json::json!({}), Default::default());
    // Small budget: the first `_bulk` chunk alone (10 docs) already
    // crosses it, so "0".."9" are densified via the incremental fast
    // path before the update/delete below ever land.
    app_state.set_densify_budget_docs_override(index, Some(3));
    let router = app_router_with_state(AppRouterState {
        app: app_state.clone(),
        ..Default::default()
    });

    let mut first_chunk = String::new();
    for doc_id in 0..10 {
        first_chunk.push_str(&format!(
            "{{\"index\":{{\"_id\":\"{doc_id}\"}}}}\n{{\"title\":\"first-{doc_id}\"}}\n"
        ));
    }
    post_bulk(&router, index, first_chunk).await;
    assert_eq!(count(&router, index).await, 10);

    // Update doc "3" (already densified via the fast path above) and
    // delete doc "7" (also already densified) — both are "old doc_id"
    // interference for the NEXT densify call.
    post_bulk(
        &router,
        index,
        "{\"index\":{\"_id\":\"3\"}}\n{\"title\":\"updated-3\"}\n\
         {\"delete\":{\"_id\":\"7\"}}\n"
            .to_owned(),
    )
    .await;
    assert_eq!(count(&router, index).await, 9);

    // A few more fresh inserts past the update/delete, then the explicit
    // final `_refresh` — the SAME shape as the real bench flow.
    let mut second_chunk = String::new();
    for doc_id in 10..14 {
        second_chunk.push_str(&format!(
            "{{\"index\":{{\"_id\":\"{doc_id}\"}}}}\n{{\"title\":\"second-{doc_id}\"}}\n"
        ));
    }
    post_bulk(&router, index, second_chunk).await;
    refresh(&router, index).await;

    assert_eq!(
        count(&router, index).await,
        13,
        "10 originals - 1 delete + 4 new"
    );

    let ids: Vec<String> = (0..14).map(|i| i.to_string()).collect();
    let body = mget(&router, index, &ids).await;
    let docs = body["docs"].as_array().expect("docs array");
    assert_eq!(docs.len(), 14);
    for (doc_id, doc) in docs.iter().enumerate() {
        if doc_id == 7 {
            assert_eq!(doc["found"], false, "doc 7 was deleted, must stay gone");
            continue;
        }
        assert_eq!(doc["found"], true, "doc {doc_id} should be found");
        let expected_title = if doc_id == 3 {
            "updated-3".to_owned()
        } else if doc_id < 10 {
            format!("first-{doc_id}")
        } else {
            format!("second-{doc_id}")
        };
        assert_eq!(
            doc["_source"]["title"], expected_title,
            "doc {doc_id} title mismatch"
        );
    }
}
