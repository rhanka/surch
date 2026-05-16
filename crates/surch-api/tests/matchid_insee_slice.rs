//! INSEE 10k slice acceptance test (gap B2 v1).
//!
//! Loads `tests/matchid_compat/deces/slice-10000.ndjson.gz` (a real
//! INSEE Open Licence 2.0 extract — see the README in that directory)
//! into an in-memory Surch router and asserts:
//!   - the slice gunzips to exactly 10 000 documents (20 000 NDJSON
//!     lines: action + source),
//!   - the bulk load succeeds against the matchID-shaped mapping,
//!   - a representative `match` query against `NOM` returns at least
//!     one hit (i.e. the analysis chain wired into B1 works on the
//!     real INSEE corpus, not only on the synthetic AWK fixture).
//!
//! The deterministic B1 replay manifest (`replays/deces_v1.json`)
//! stays pinned to the synthetic `slice-1000.ndjson` fixture: its
//! per-request `hits.total.value` and `hits.hits[0]._id` expectations
//! are bound to the AWK-generated documents and would need a full
//! re-capture to track a different corpus. This INSEE slice is the
//! "real-data smoke" complement; the deces_v2 replay (gated on the
//! poc-k8s ES-7.x oracle cross-check) will replace this test with
//! frozen expectations.

use std::io::Read;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use flate2::read::GzDecoder;
use serde_json::Value;
use surch_api::app_router;
use tower::ServiceExt;

const MAPPING_BODY: &str =
    include_str!("../../../tests/matchid_compat/deces/mapping.json");
const SLICE_GZ: &[u8] =
    include_bytes!("../../../tests/matchid_compat/deces/slice-10000.ndjson.gz");

const INDEX_NAME: &str = "deces";

fn decode_slice() -> String {
    let mut decoder = GzDecoder::new(SLICE_GZ);
    let mut out = String::new();
    decoder
        .read_to_string(&mut out)
        .expect("slice-10000.ndjson.gz must decode");
    out
}

#[test]
fn matchid_insee_slice_b2_v1_loads_and_serves_match_query() {
    let slice = decode_slice();

    // 10 000 docs × 2 NDJSON lines (action + source) = 20 000 lines.
    let line_count = slice.lines().count();
    assert_eq!(
        line_count, 20_000,
        "INSEE slice must contain 10 000 documents (20 000 NDJSON lines)"
    );

    // Spot-check the first action header carries the documented id
    // namespace (`ins_NNNNNNN`, 7-digit zero-padded). This guards
    // against accidental synthetic-namespace reuse (`deces_NNNNN`)
    // when the slice is regenerated.
    let first_action = slice.lines().next().expect("first line");
    let first_action_json: Value =
        serde_json::from_str(first_action).expect("first action parses");
    let first_id = first_action_json
        .pointer("/index/_id")
        .and_then(Value::as_str)
        .expect("first action carries _id");
    assert!(
        first_id.starts_with("ins_") && first_id.len() == 11,
        "first _id must use the `ins_NNNNNNN` namespace, got `{first_id}`"
    );

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let router = app_router();

    let (create_status, _) = runtime.block_on(execute(
        router.clone(),
        Method::PUT,
        &format!("/{INDEX_NAME}"),
        MAPPING_BODY.to_string(),
    ));
    assert_eq!(create_status, StatusCode::OK.as_u16(), "create index OK");

    let (bulk_status, bulk_body) = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        "/_bulk",
        slice,
    ));
    assert_eq!(
        bulk_status,
        StatusCode::OK.as_u16(),
        "bulk load OK: body={bulk_body:?}"
    );
    let errors = bulk_body
        .as_ref()
        .and_then(|v| v.get("errors"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    assert!(!errors, "bulk load must report errors=false");

    let (refresh_status, _) = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        &format!("/{INDEX_NAME}/_refresh"),
        String::new(),
    ));
    assert_eq!(refresh_status, StatusCode::OK.as_u16(), "refresh OK");

    // `match` on a NOM known to appear at row 1 of the INSEE 2024 m01
    // extract (RUMIANO FERNANDO). This validates that the analysis
    // chain wired by B1 (text + index_prefixes) does not regress on
    // real-data inputs.
    let body = serde_json::json!({
        "query": { "match": { "NOM": "RUMIANO" } },
        "size": 1
    })
    .to_string();
    let (search_status, search_body) = runtime.block_on(execute(
        router,
        Method::POST,
        &format!("/{INDEX_NAME}/_search"),
        body,
    ));
    assert_eq!(search_status, StatusCode::OK.as_u16(), "search OK");
    let body = search_body.expect("search body");
    let total = body
        .pointer("/hits/total/value")
        .and_then(Value::as_u64)
        .expect("hits.total.value present");
    assert!(
        total >= 1,
        "match NOM=RUMIANO must hit at least 1 doc in INSEE slice (got {total})"
    );
    let top_id = body
        .pointer("/hits/hits/0/_id")
        .and_then(Value::as_str)
        .expect("hits.hits[0]._id present");
    assert_eq!(
        top_id, "ins_0000001",
        "top hit for NOM=RUMIANO must be the documented row-1 record"
    );
}

async fn execute(
    router: Router,
    method: Method,
    path: &str,
    body: String,
) -> (u16, Option<Value>) {
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
        Some(serde_json::from_slice::<Value>(&bytes).expect("body is JSON"))
    };
    (status, value)
}
