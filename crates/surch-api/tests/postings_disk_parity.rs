//! Lot C `C1b` sous-pas 2 gate
//! (`docs/paper/c1b-disk-backed-design-2026-07-02.md`): the disk-backed
//! postings read path (`SURCH_POSTINGS_DISK`) must be BIT-IDENTICAL to the
//! historical 100 % in-RAM engine — same doc-id sets, same BM25 scores, same
//! ranking order — for `match`, `bool` (2- and 3-clause single-token
//! conjunction, exercising the N-cursor disk leapfrog-join), a multi-token
//! `bool.must` clause (the `conjunction_of_matches` decode-owned fallback),
//! and a `should`-all-required conjunction (the fused scoring path).
//!
//! Testability note (see `surch_index::postings::postings_disk_enabled`'s
//! doc comment): the disk flag is normally a process-wide `OnceLock`-cached
//! env var, which cannot flip mid-process — a single test binary can only
//! ever observe ONE value of it. This test therefore never touches the env
//! var at all; it uses [`AppState::set_postings_disk_enabled`], a per-index
//! override that bypasses the `OnceLock` entirely, to build one index with
//! the disk path forced OFF and a second, independent index with it forced
//! ON, in the SAME process, and compares their `_search` responses.

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::{json, Value};
use surch_api::{app_router_with_state, state::AppState, AppRouterState};
use tower::ServiceExt;

async fn response_json(response: axum::response::Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be json")
}

/// Deterministic corpus: 300 docs across 3 `title` groups (so `widget`
/// spans 200 docs — over one 128-posting FoR block, exercising the
/// disk-cursor's block-directory skip — and `gadget` spans 100, under one
/// block), 2 `category` groups, and a `body` term (`common`) shared by
/// EVERY doc (multi-block, the common-term tail the design worries about).
fn corpus_ndjson() -> String {
    let mut out = String::new();
    for i in 0..300 {
        let title = match i % 3 {
            0 => "alpha widget",
            1 => "beta widget",
            _ => "gamma gadget",
        };
        let category = if i % 2 == 0 { "tools" } else { "toys" };
        let body = if i.is_multiple_of(6) {
            "common common lorem ipsum"
        } else {
            "common lorem ipsum"
        };
        out.push_str(&format!(
            "{{\"index\":{{\"_id\":\"{i}\"}}}}\n\
             {{\"title\":\"{title}\",\"category\":\"{category}\",\"body\":\"{body} doc{i}\"}}\n"
        ));
    }
    out
}

/// Build a fresh `AppState`-backed router with `index` created up front and
/// its disk-backed postings flag pinned to `disk_enabled` BEFORE any
/// document lands — `set_postings_disk_enabled` only takes effect at the
/// next `PostingsBuilder::build_with_disk_flag` call (the bulk-then-refresh
/// below), so the ordering here is load-bearing.
async fn build_index(index: &str, disk_enabled: bool) -> axum::Router {
    let app_state = AppState::default();
    app_state.create_index(index, None, json!({}), Default::default());
    app_state.set_postings_disk_enabled(index, disk_enabled);

    let router = app_router_with_state(AppRouterState {
        app: app_state,
        ..Default::default()
    });

    let bulk_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/{index}/_bulk"))
                .header("content-type", "application/x-ndjson")
                .body(Body::from(corpus_ndjson()))
                .expect("bulk request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(bulk_response.status(), StatusCode::OK);
    let bulk_body = response_json(bulk_response).await;
    assert_eq!(
        bulk_body["errors"], false,
        "bulk indexing must succeed (disk_enabled={disk_enabled}): {bulk_body:?}"
    );

    // Explicit `_refresh` so the disk segment / `TermDictionary` rebuild
    // under the pinned flag is exercised the same way production
    // `_bulk` -> `_refresh` -> `_search` usage would (mirrors
    // `PostingsBuilder::build_with_disk_flag`'s call site in
    // `DocumentIndex::materialize_terms_and_finalize_postings`).
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

    router
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
    // Le surlignage force la référence à contourner tout scorer top-K direct
    // sans modifier les ids ni les scores comparés par le fingerprint.
    request_object.insert(
        "highlight".to_owned(),
        serde_json::json!({"fields":{"title":{}}}),
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

/// `(id, score)` pairs in response order — order matters here, not just
/// set membership, since a scoring divergence between the RAM and disk
/// paths could also reorder ties. `_score` is absent on `match_all` (surch
/// omits it, see `match_all_query_keeps_max_score_null_and_omits_score_field`
/// in `tests/search.rs`); both sides then read `0.0` identically, which is
/// still a valid — if less informative — comparison for that one query.
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
async fn postings_disk_flag_on_matches_flag_off_bit_identical() {
    let index = "parity-idx";
    let router_off = build_index(index, false).await;
    let router_on = build_index(index, true).await;

    // Each query is chosen to exercise a specific piece of this sous-pas's
    // read-path wiring:
    let queries: &[(&str, &str)] = &[
        (
            "single-token match (maxscore OR-match, common multi-block term)",
            r#"{"query":{"match":{"body":"common"}},"size":100}"#,
        ),
        (
            "single-token match (maxscore OR-match, split term)",
            r#"{"query":{"match":{"title":"widget"}},"size":100}"#,
        ),
        (
            "2-clause bool.must single-token conjunction (conjunction_hits_internal / DiskPostingsCursor leapfrog)",
            r#"{"query":{"bool":{"must":[{"match":{"title":"widget"}},{"match":{"category":"tools"}}]}},"size":100}"#,
        ),
        (
            "2-clause bool.must duplicate (P1a direct must)",
            r#"{"query":{"bool":{"must":[{"match":{"title":"widget"}},{"match":{"title":"widget"}}]}},"size":100}"#,
        ),
        (
            "3-clause bool.must single-token conjunction (N-cursor disk leapfrog-join)",
            r#"{"query":{"bool":{"must":[{"match":{"title":"widget"}},{"match":{"category":"tools"}},{"match":{"body":"common"}}]}},"size":100}"#,
        ),
        (
            "multi-token bool.must clause (conjunction_of_matches decode-owned fallback)",
            r#"{"query":{"bool":{"must":[{"match":{"title":"alpha widget"}},{"match":{"category":"tools"}}]}},"size":100}"#,
        ),
        (
            "should-all-required conjunction (fused_conjunction_scores)",
            r#"{"query":{"bool":{"should":[{"match":{"title":"widget"}},{"match":{"category":"tools"}}],"minimum_should_match":2}},"size":100}"#,
        ),
        (
            "match_all (baseline recall/order sanity check)",
            r#"{"query":{"match_all":{}},"size":100}"#,
        ),
    ];

    for &(label, query_body) in queries {
        let body_off = search(&router_off, index, query_body).await;
        let body_on = search(&router_on, index, query_body).await;

        assert_eq!(
            body_off["hits"]["total"]["value"], body_on["hits"]["total"]["value"],
            "[{label}] total hits diverged between flag OFF and flag ON for {query_body}"
        );

        let hits_off = extract_ordered_hits(&body_off);
        let hits_on = extract_ordered_hits(&body_on);
        assert_eq!(
            hits_off, hits_on,
            "[{label}] (id, score) hits diverged between flag OFF and flag ON for {query_body}"
        );
    }

    // Deux termes distincts, pagination et `min_score` évitent une fausse
    // parité où le même `df` serait utilisé deux fois ou où la finalisation
    // ignorerait ses filtres. Le chemin forcé-générique reste l'oracle.
    let p1a_direct = r#"{"from":5,"size":17,"min_score":0.8,"query":{"bool":{"must":[{"match":{"title":"widget"}},{"match":{"category":"tools"}}]}}}"#;
    for (layout, router, direct_eligible) in [
        ("RAM", &router_off, true),
        // Après le lot S (`cb0ada8`), P2 couvre le disque checked : le
        // compteur P1a doit donc augmenter sans affaiblir la parité.
        ("disque", &router_on, true),
    ] {
        let counter_before = p1a_direct_must_fused_counter(router).await;
        let direct = search(router, index, p1a_direct).await;
        let counter_after = p1a_direct_must_fused_counter(router).await;
        assert_eq!(
            counter_after,
            counter_before + if direct_eligible { 1 } else { 0 },
            "[P2 {layout}] le compteur doit augmenter exactement une fois"
        );
        let generic = search(router, index, &force_generic_bool_reference(p1a_direct)).await;
        assert_eq!(
            p1a_scored_response_fingerprint(&direct),
            p1a_scored_response_fingerprint(&generic),
            "[P1a {layout}] réponse rapide et référence générique divergent"
        );
        assert_eq!(
            direct["hits"]["hits"].as_array().map(Vec::len),
            Some(17),
            "[P2 {layout}] `from`/`size` doit conserver la fenêtre demandée"
        );
    }
}

#[tokio::test]
async fn p2_trois_clauses_scorees_conservent_min_score_ordre_et_bits() {
    let index = "p2-trois-clauses-scorees";
    let router = build_index(index, true).await;
    let base = r#"{"from":0,"size":300,"query":{"bool":{"should":[{"match":{"title":"widget"}},{"match":{"category":"tools"}},{"match":{"body":"common"}}],"minimum_should_match":3}}}"#;
    let sans_filtre = search(&router, index, base).await;
    let scores: Vec<f64> = sans_filtre["hits"]["hits"]
        .as_array()
        .expect("les hits non filtrés doivent être un tableau")
        .iter()
        .map(|hit| hit["_score"].as_f64().expect("hit scoré"))
        .collect();
    let score_min = scores
        .iter()
        .copied()
        .reduce(f64::min)
        .expect("au moins un hit scoré");
    let score_max = scores
        .iter()
        .copied()
        .reduce(f64::max)
        .expect("au moins un hit scoré");
    assert!(
        score_min < score_max,
        "la fixture doit placer des scores de part et d'autre du seuil"
    );
    let min_score = (score_min + score_max) / 2.0;
    let request = serde_json::to_string(&json!({
        "from": 0,
        "size": 300,
        "min_score": min_score,
        "query": {
            "bool": {
                "should": [
                    {"match": {"title": "widget"}},
                    {"match": {"category": "tools"}},
                    {"match": {"body": "common"}}
                ],
                "minimum_should_match": 3
            }
        }
    }))
    .expect("requête P2 sérialisable");

    let direct = search(&router, index, &request).await;
    let generic = search(&router, index, &force_generic_bool_reference(&request)).await;
    assert_eq!(
        p1a_scored_response_fingerprint(&direct),
        p1a_scored_response_fingerprint(&generic),
        "P2 doit conserver les ids, l'ordre, les bits, max_score et total"
    );
    assert!(
        direct["hits"]["total"]["value"]
            .as_u64()
            .expect("total filtré")
            < sans_filtre["hits"]["total"]["value"]
                .as_u64()
                .expect("total non filtré"),
        "min_score doit éliminer au moins un hit P2"
    );
    assert!(
        direct["hits"]["hits"]
            .as_array()
            .expect("hits filtrés")
            .iter()
            .all(|hit| hit["_score"]
                .as_f64()
                .is_some_and(|score| score >= min_score)),
        "aucun hit P2 sous min_score ne doit survivre"
    );
}
