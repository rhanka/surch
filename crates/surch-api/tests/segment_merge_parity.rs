//! Plan segments S3 gate (`docs/paper/design-segments-pic-borne-2026-07-05.md`,
//! "merge tiered inline sur runs adjacents"): the tiered merge must produce
//! REAL, CASCADING segment reduction (far fewer sealed segments than the
//! un-merged multi-segment engine) while staying BIT-IDENTICAL — same
//! doc-id sets, same BM25 scores, same ranking order — to both the
//! mono-segment engine (`SURCH_FLUSH_BUDGET_BYTES` unset) and the
//! un-merged multi-segment engine (`SURCH_MERGE_FANIN=0`, forced via the
//! per-index override).
//!
//! Mirrors `segment_flush_budget_parity.rs`'s corpus/harness shape, adding
//! the merge-fan-in dimension: three engines are built from the SAME
//! corpus — mono (no budget), multi un-merged (budget forced, fanin
//! forced to `0`), multi merged (budget forced, fanin forced to a small
//! value so a corpus of this size actually cascades) — and every
//! `_search` response is compared pairwise.

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
/// payloads of `PER_CHUNK` docs each — the budget check fires once per
/// chunk, not once for the whole corpus (same rationale as
/// `segment_flush_budget_parity.rs`). Sized larger than the S2 gate
/// (more chunks) so a small merge fan-in has enough sealed segments to
/// actually cascade through more than one tier.
const CHUNKS: usize = 24;
const PER_CHUNK: usize = 10;

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
            let category = if doc_id.is_multiple_of(2) {
                "tools"
            } else {
                "toys"
            };
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
/// and merge-fan-in overrides BEFORE any document lands (mirrors
/// `segment_flush_budget_parity.rs::build_index`'s ordering rationale),
/// then POST the corpus as `CHUNKS` separate `_bulk` requests and refresh
/// once at the end.
async fn build_index(
    index: &str,
    forced_budget_bytes: Option<u64>,
    forced_merge_fanin: usize,
) -> (axum::Router, AppState) {
    let app_state = AppState::default();
    app_state.create_index(index, None, serde_json::json!({}), Default::default());
    app_state.set_flush_budget_bytes_override(index, forced_budget_bytes);
    app_state.set_merge_fanin_override(index, forced_merge_fanin);

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
            "bulk indexing must succeed (forced_budget_bytes={forced_budget_bytes:?}, \
             forced_merge_fanin={forced_merge_fanin}): {bulk_body:?}"
        );
    }

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

fn force_generic_bool_reference(body: &str) -> String {
    let mut request: Value = serde_json::from_str(body).expect("request must be json");
    let request_object = request.as_object_mut().expect("request must be an object");
    let query = request_object
        .remove("query")
        .expect("request must carry query");
    request_object.insert(
        "query".to_owned(),
        serde_json::json!({"function_score":{"query":query}}),
    );
    serde_json::to_string(&request).expect("reference request must serialize")
}

async fn p1a_direct_must_fused_counter(router: &axum::Router) -> u64 {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_prometheus_metrics")
                .body(Body::empty())
                .expect("metrics request should build"),
        )
        .await
        .expect("metrics router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("metrics body should be readable");
    let exposition = std::str::from_utf8(&body).expect("metrics must be utf-8");
    exposition
        .lines()
        .find_map(|line| {
            line.strip_prefix("surch_bool_direct_must_fused_total ")
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(0)
}

fn p1a_scored_response_fingerprint(body: &Value) -> (Vec<(String, u64)>, Option<u64>, Value) {
    let hits = body["hits"]["hits"]
        .as_array()
        .expect("hits must be an array");
    let scored_hits = hits
        .iter()
        .map(|hit| {
            (
                hit["_id"].as_str().expect("hit must carry _id").to_owned(),
                hit["_score"]
                    .as_f64()
                    .expect("hit must carry score")
                    .to_bits(),
            )
        })
        .collect();
    (
        scored_hits,
        body["hits"]["max_score"].as_f64().map(f64::to_bits),
        body["hits"]["total"].clone(),
    )
}

/// `(id, score)` pairs in response order — order matters, not just set
/// membership, since a scoring divergence between engines could also
/// reorder ties.
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
async fn tiered_merge_cascades_and_matches_mono_and_unmerged_bit_identical() {
    let index = "seg-merge-parity-idx";

    // `Some(1)`: any non-empty `PostingsBuilder` crosses the budget, so
    // every one of the `CHUNKS` `_bulk` POSTs seals its own segment —
    // `router_unmerged` (fanin=0) is the S2-shaped ground truth (real
    // multi-segment, but merge never runs); `router_merged` (fanin=3)
    // must cascade that down to far fewer segments. `router_mono`
    // (`None` budget) reproduces the S1 mono-segment engine.
    let (router_mono, app_state_mono) = build_index(index, None, 0).await;
    let (router_unmerged, app_state_unmerged) = build_index(index, Some(1), 0).await;
    let (router_merged, app_state_merged) = build_index(index, Some(1), 3).await;

    assert_eq!(
        app_state_mono.index_segment_count(index),
        1,
        "budget-unset (forced None) index must stay mono-segment — the S1 invariant"
    );
    assert_eq!(
        app_state_unmerged.index_segment_count(index),
        CHUNKS + 1,
        "fanin=0 (S3 reversibility flag) must reproduce the exact S2 layout: \
         one sealed segment per chunk plus the fresh active one"
    );
    assert!(
        app_state_merged.index_segment_count(index) < app_state_unmerged.index_segment_count(index),
        "fanin=3 must cascade to fewer segments than the unmerged engine: \
         merged={}, unmerged={}",
        app_state_merged.index_segment_count(index),
        app_state_unmerged.index_segment_count(index)
    );
    assert!(
        app_state_merged.index_segment_count(index) <= CHUNKS / 2,
        "merge should have cascaded well below one segment per chunk, got {} \
         sealed+active segments for {CHUNKS} chunks",
        app_state_merged.index_segment_count(index)
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
            "2-clause bool.must duplicate (P1a direct must)",
            r#"{"query":{"bool":{"must":[{"match":{"title":"widget"}},{"match":{"title":"widget"}}]}},"size":400}"#,
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
        let body_unmerged = search(&router_unmerged, index, query_body).await;
        let body_merged = search(&router_merged, index, query_body).await;

        assert_eq!(
            body_mono["hits"]["total"]["value"], body_unmerged["hits"]["total"]["value"],
            "[{label}] total hits diverged mono vs unmerged for {query_body}"
        );
        assert_eq!(
            body_mono["hits"]["total"]["value"], body_merged["hits"]["total"]["value"],
            "[{label}] total hits diverged mono vs merged for {query_body}"
        );

        let hits_mono = extract_ordered_hits(&body_mono);
        let hits_unmerged = extract_ordered_hits(&body_unmerged);
        let hits_merged = extract_ordered_hits(&body_merged);
        assert_eq!(
            hits_mono, hits_unmerged,
            "[{label}] (id, score) hits diverged mono vs unmerged for {query_body}"
        );
        assert_eq!(
            hits_mono, hits_merged,
            "[{label}] (id, score) hits diverged mono vs merged for {query_body}"
        );
    }

    let p1a_direct = r#"{"query":{"bool":{"must":[{"match":{"title":"widget"}},{"match":{"title":"widget"}}]}},"size":400}"#;
    for (layout, router, direct_eligible) in [
        ("mono-segment", &router_mono, true),
        ("multi-segment non fusionné", &router_unmerged, false),
        ("multi-segment fusionné", &router_merged, false),
    ] {
        let counter_before = p1a_direct_must_fused_counter(router).await;
        let direct = search(router, index, p1a_direct).await;
        let counter_after = p1a_direct_must_fused_counter(router).await;
        assert_eq!(
            counter_after,
            counter_before + if direct_eligible { 1 } else { 0 },
            "[P1a {layout}] le compteur ne doit augmenter que pour le mono-segment"
        );
        let generic = search(router, index, &force_generic_bool_reference(p1a_direct)).await;
        assert_eq!(
            p1a_scored_response_fingerprint(&direct),
            p1a_scored_response_fingerprint(&generic),
            "[P1a {layout}] réponse rapide et référence générique divergent"
        );
    }

    // `match_all` at `size` >= corpus recovers every doc: an independent
    // sanity check that all three engines actually indexed the full corpus
    // (not just agreeing on an accidentally-truncated candidate set).
    let body_all = search(
        &router_merged,
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
