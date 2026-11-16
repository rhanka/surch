//! MVP L2b guard: the `/version` sorted `match_all` walk
//! ([`super::run_sorted_match_all_top`]) returns byte for byte what the
//! general path returns (hydrate every document, sort, paginate), on the
//! three index layouts, with deleted documents and documents missing a
//! sort field.

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{parse_search_request, run_search_paths, run_sorted_match_all_top};
use crate::{app_router_with_state, state::AppState, AppRouterState};

const DECES_INDEX_REAL: &str =
    include_str!("../../../tests/matchid_compat/deces/mapping_real.json");
const INDEX: &str = "deces";

#[derive(Clone, Copy, Debug)]
enum Layout {
    SingleRam,
    MultiSegment,
    DiskBacked,
}

async fn send(router: &axum::Router, method: Method, uri: &str, body: String) -> Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body readable");
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn corpus(total: usize) -> Vec<(String, Value)> {
    let mut seed: u64 = 0x2b2b_1115_2026_0001;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    (0..total)
        .map(|n| {
            // Few distinct values, mixed case and separators: the sort key
            // is the `norm`-normalised term.
            let source_value = ["2019-M12", "2020-m01", "2020-M02", "2020-m02"][next(4) as usize];
            let mut source = json!({
                "UID": format!("uid{n:04}"),
                "SOURCE": source_value,
                "NOM": "DUPONT",
                "SEXE": if n % 2 == 0 { "M" } else { "F" },
            });
            // One document in seven has no death date (missing: _last).
            if n % 7 != 3 {
                let date = format!("2020{:02}{:02}", 1 + next(12), 1 + next(28));
                source["DATE_DECES"] = json!(date);
            }
            (format!("d{n:04}"), source)
        })
        .collect()
}

async fn build(layout: Layout, docs: &[(String, Value)]) -> AppState {
    let state = AppState::default();
    let router = app_router_with_state(AppRouterState {
        app: state.clone(),
        ..Default::default()
    });
    let created = send(
        &router,
        Method::PUT,
        &format!("/{INDEX}"),
        DECES_INDEX_REAL.to_owned(),
    )
    .await;
    assert_eq!(created["acknowledged"], true, "PUT /{INDEX}: {created}");
    state.set_merge_fanin_override(INDEX, 0);
    match layout {
        Layout::SingleRam => state.set_flush_budget_bytes_override(INDEX, None),
        Layout::MultiSegment => state.set_flush_budget_bytes_override(INDEX, Some(1)),
        Layout::DiskBacked => {
            state.set_flush_budget_bytes_override(INDEX, None);
            state.set_postings_disk_enabled(INDEX, true);
        }
    }
    for chunk in docs.chunks(docs.len().div_ceil(4).max(1)) {
        let mut ndjson = String::new();
        for (id, source) in chunk {
            ndjson.push_str(&format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{source}\n"));
        }
        let body = send(&router, Method::POST, &format!("/{INDEX}/_bulk"), ndjson).await;
        assert_eq!(body["errors"], false, "bulk failed: {body:?}");
    }
    // Delete a few documents, the likely top hits among them.
    for id in ["d0000", "d0001", "d0010", "d0100"] {
        send(
            &router,
            Method::DELETE,
            &format!("/{INDEX}/_doc/{id}"),
            String::new(),
        )
        .await;
    }
    send(
        &router,
        Method::POST,
        &format!("/{INDEX}/_refresh"),
        String::new(),
    )
    .await;
    state
}

fn serialized(mut response: super::SearchResponse) -> String {
    response.took = 0;
    String::from_utf8(crate::es_json::to_es_json_vec(&response).expect("serializes")).expect("utf8")
}

#[tokio::test]
async fn sorted_match_all_walk_equals_the_general_path() {
    let docs = corpus(400);
    let shapes = [
        r#"{"sort":[{"SOURCE":"desc"},{"DATE_DECES.raw":"desc"}],"size":1}"#,
        r#"{"sort":[{"SOURCE":"asc"},{"DATE_DECES.raw":"asc"}],"size":7,"_source":["UID"]}"#,
        r#"{"sort":[{"DATE_DECES.raw":"desc"}],"from":3,"size":5,"_source":["UID","DATE_DECES"]}"#,
        r#"{"sort":[{"SOURCE":"desc"},{"DATE_DECES.raw":"asc"}],"size":120,"_source":false}"#,
        r#"{"query":{"match_all":{}},"sort":[{"SOURCE":"desc"},{"UID":"asc"}],"size":3,"track_total_hits":true}"#,
        r#"{"sort":[{"SOURCE":"asc"},{"DATE_DECES.raw":"desc"}],"size":400,"_source":["UID"]}"#,
    ];
    for layout in [Layout::SingleRam, Layout::MultiSegment, Layout::DiskBacked] {
        let state = build(layout, &docs).await;
        let indices = vec![INDEX.to_owned()];
        for shape in shapes {
            let request = parse_search_request(shape).expect("parses");
            let fast =
                run_sorted_match_all_top(&state, &indices, &request, std::time::Instant::now())
                    .unwrap_or_else(|| panic!("{layout:?}: the walk declined {shape}"));
            // `min_score` makes the walk decline; every hit scores the
            // `match_all` constant 1.0 >= 0, so the general path answers
            // the same request.
            let mut general_request = request.clone();
            general_request.min_score = Some(0.0);
            assert!(run_sorted_match_all_top(
                &state,
                &indices,
                &general_request,
                std::time::Instant::now()
            )
            .is_none());
            let general = run_search_paths(&state, &indices, &general_request);
            assert_eq!(serialized(fast), serialized(general), "{layout:?}: {shape}");
        }
    }
}

#[tokio::test]
async fn sorted_match_all_walk_declines_other_shapes() {
    let state = build(Layout::SingleRam, &corpus(40)).await;
    let indices = vec![INDEX.to_owned()];
    for shape in [
        // Default score sort, a `_score` clause, a non-keyword field, a
        // real query, aggregations-free but windows beyond the cap.
        r#"{"size":1}"#,
        r#"{"sort":[{"_score":"desc"}],"size":1}"#,
        r#"{"sort":[{"SOURCE":"desc"},"_score"],"size":1}"#,
        r#"{"sort":[{"AGE_DECES":"desc"}],"size":1}"#,
        r#"{"query":{"match":{"NOM":"dupont"}},"sort":[{"SOURCE":"desc"}],"size":1}"#,
        r#"{"sort":[{"SOURCE":"desc"}],"size":1001}"#,
    ] {
        let request = parse_search_request(shape).expect("parses");
        assert!(
            run_sorted_match_all_top(&state, &indices, &request, std::time::Instant::now())
                .is_none(),
            "{shape}"
        );
    }
}
