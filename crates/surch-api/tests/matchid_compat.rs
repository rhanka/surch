//! matchID compatibility replay harness (gap B1).
//!
//! Loads the frozen `tests/matchid_compat/deces/` slice into an
//! in-memory Surch router and replays the curated request set from
//! `tests/matchid_compat/replays/deces_v1.json`. Each non-skipped
//! request must reproduce `hits.total.value` and `hits.hits[0]._id`.
//!
//! Skipped entries carry a `skip` field referencing the active gap
//! (e.g. `gap-A4` for scroll, `gap-A6` for `prefix + index_prefixes`).
//! They are not executed; they document the workload the future
//! parser-side gap closure must unblock. As of 2026-05-18, the manifest
//! is expected to execute all 30 entries against Surch HEAD.

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use serde::Deserialize;
use serde_json::Value;
use surch_api::app_router;
use tower::ServiceExt;

const MAPPING_BODY: &str = include_str!("../../../tests/matchid_compat/deces/mapping.json");
const SLICE_NDJSON: &str = include_str!("../../../tests/matchid_compat/deces/slice-1000.ndjson");
const REPLAY_JSON: &str = include_str!("../../../tests/matchid_compat/replays/deces_v1.json");

const INDEX_NAME: &str = "deces";

#[derive(Debug, Deserialize)]
struct ReplayFile {
    name: String,
    dataset: String,
    requests: Vec<ReplayEntry>,
}

#[derive(Debug, Deserialize)]
struct ReplayEntry {
    name: String,
    request: ReplayRequest,
    expected: Option<ReplayExpected>,
    #[serde(default)]
    skip: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReplayRequest {
    method: String,
    path: String,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    body_ndjson: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
struct ReplayExpected {
    #[serde(rename = "hits.total.value")]
    total_value: u64,
    #[serde(rename = "hits.hits[0]._id")]
    top_id: Option<String>,
}

#[test]
fn matchid_replay_deces_v1_executes_all_non_skipped_requests() {
    let manifest: ReplayFile =
        serde_json::from_str(REPLAY_JSON).expect("replay manifest must parse");
    assert_eq!(manifest.name, "deces_v1");
    assert_eq!(manifest.dataset, "deces");
    assert_eq!(
        manifest.requests.len(),
        30,
        "replay manifest must declare exactly 30 entries (8 advanced + 5 block-match + 4 \
         full-text + 3 sort + 2 function_score + 3 prefix + 3 scroll + 2 range)"
    );

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime must start");
    let router = app_router();

    // Setup index + load slice.
    let create_status = runtime.block_on(execute(
        router.clone(),
        Method::PUT,
        &format!("/{INDEX_NAME}"),
        MAPPING_BODY.to_string(),
    ));
    assert_eq!(
        create_status.0,
        StatusCode::OK.as_u16(),
        "create index must succeed: body={:?}",
        create_status.1
    );

    let bulk_status = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        "/_bulk",
        SLICE_NDJSON.to_string(),
    ));
    assert_eq!(
        bulk_status.0,
        StatusCode::OK.as_u16(),
        "bulk load must succeed: body={:?}",
        bulk_status.1
    );

    let refresh_status = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        &format!("/{INDEX_NAME}/_refresh"),
        String::new(),
    ));
    assert_eq!(refresh_status.0, StatusCode::OK.as_u16());

    let mut executed = 0usize;
    let mut skipped = 0usize;
    for entry in &manifest.requests {
        if let Some(gap) = entry.skip.as_deref() {
            // Sanity: skipped entries must reference an A-series gap.
            assert!(
                gap.starts_with("gap-A"),
                "skip reason for `{}` must reference an A-series gap, got `{}`",
                entry.name,
                gap
            );
            skipped += 1;
            continue;
        }

        let expected = entry.expected.as_ref().unwrap_or_else(|| {
            panic!(
                "non-skipped replay entry `{}` must declare expected",
                entry.name
            )
        });

        let method = match entry.request.method.as_str() {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            other => panic!("unsupported method `{other}` in entry `{}`", entry.name),
        };

        let body = match (&entry.request.body, &entry.request.body_ndjson) {
            (Some(_), Some(_)) => panic!(
                "replay entry `{}` must not declare both `body` and `body_ndjson`",
                entry.name
            ),
            (None, Some(lines)) => {
                let mut buf = String::new();
                for line in lines {
                    buf.push_str(&serde_json::to_string(line).expect("ndjson line must serialize"));
                    buf.push('\n');
                }
                buf
            }
            (Some(v), None) => {
                if v.is_null() {
                    String::new()
                } else {
                    serde_json::to_string(v).expect("body must serialize")
                }
            }
            (None, None) => String::new(),
        };
        let (status, response_body) =
            runtime.block_on(execute(router.clone(), method, &entry.request.path, body));

        assert_eq!(
            status,
            StatusCode::OK.as_u16(),
            "request `{}` returned status {} (expected 200), body={:?}",
            entry.name,
            status,
            response_body
        );

        let body =
            response_body.unwrap_or_else(|| panic!("request `{}` returned empty body", entry.name));

        if entry.request.path.ends_with("_msearch") {
            // _msearch responses carry `responses[]`; each sub-response has its own
            // `hits.total.value`. The expected total is the SUM across sub-responses
            // and the expected top_id is the top hit of the first sub-response.
            let responses = body
                .get("responses")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!(
                        "msearch request `{}` response must contain `responses`",
                        entry.name
                    )
                });
            let mut total_sum = 0u64;
            let mut first_top_id: Option<String> = None;
            for (idx, sub) in responses.iter().enumerate() {
                let total = sub
                    .pointer("/hits/total/value")
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| {
                        panic!(
                            "msearch entry `{}` sub-response[{}] missing hits.total.value",
                            entry.name, idx
                        )
                    });
                total_sum += total;
                if idx == 0 {
                    first_top_id = sub
                        .pointer("/hits/hits/0/_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            assert_eq!(
                total_sum, expected.total_value,
                "msearch `{}` summed hits.total.value mismatch",
                entry.name
            );
            if let Some(expected_top) = &expected.top_id {
                assert_eq!(
                    first_top_id.as_deref(),
                    Some(expected_top.as_str()),
                    "msearch `{}` top hit of first sub-response mismatch",
                    entry.name
                );
            }
        } else {
            let total = body
                .pointer("/hits/total/value")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| {
                    panic!(
                        "request `{}` response missing hits.total.value, got {:?}",
                        entry.name, body
                    )
                });
            assert_eq!(
                total, expected.total_value,
                "request `{}` hits.total.value mismatch",
                entry.name
            );

            if let Some(expected_top) = &expected.top_id {
                let actual_top = body
                    .pointer("/hits/hits/0/_id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!(
                            "request `{}` response missing hits.hits[0]._id, got {:?}",
                            entry.name, body
                        )
                    });
                assert_eq!(
                    actual_top, expected_top,
                    "request `{}` top hit id mismatch",
                    entry.name
                );
            }

            // For scroll-initiating requests (`?scroll=...`), the response must
            // also carry a non-empty `_scroll_id`. The follow-up
            // `POST /_search/scroll` lifecycle is covered by
            // `crates/surch-api/tests/scroll.rs`; here we only assert the
            // initial page exposes a usable cursor.
            if entry.request.path.contains("?scroll=") || entry.request.path.contains("&scroll=") {
                let scroll_id = body
                    .get("_scroll_id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!(
                            "scroll request `{}` response missing _scroll_id, got {:?}",
                            entry.name, body
                        )
                    });
                assert!(
                    !scroll_id.is_empty(),
                    "scroll request `{}` returned an empty _scroll_id",
                    entry.name
                );
            }
        }
        executed += 1;
    }

    assert!(
        executed >= 3,
        "matchID v0 acceptance: at least 3 replay entries must execute against Surch (executed={executed}, skipped={skipped})"
    );
    // The fixture is committed with the documented split: all 30 entries execute.
    // B1 phase 2 lifted the 3 scroll skips (gap-A4) by asserting the initial
    // `?scroll=1m` response carries a non-empty `_scroll_id`; the follow-up
    // `POST /_search/scroll` lifecycle is covered by
    // `crates/surch-api/tests/scroll.rs`. A6 phase 2 had previously lifted the two
    // text-typed prefix skips (prefix_nom, prefix_prenoms) via write-time
    // `index_prefixes` postings on NOM/PRENOMS. A6 phase 3 (2026-05-16) lifted the
    // last skip (`prefix_date_naissance_short`, gap-A6) via an FST range-scan
    // iterator on the term dictionary (see
    // `surch_index::DocumentIndex::term_prefix_doc_ids`). This batch lifts the
    // final replay skip by executing `adv_bool_must_not_sexe` instead of rejecting
    // `bool.must_not` at parse time.
    assert_eq!(executed + skipped, 30);
    assert_eq!(skipped, 0, "expected 0 skipped entries against Surch HEAD");
    assert_eq!(
        executed, 30,
        "expected 30 executed entries against Surch HEAD"
    );
}

async fn execute(router: Router, method: Method, path: &str, body: String) -> (u16, Option<Value>) {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body collects");
    let value = if bytes.is_empty() {
        None
    } else {
        Some(serde_json::from_slice::<Value>(&bytes).expect("response body must be JSON"))
    };
    (status, value)
}
