//! A7 end-to-end: `range` queries on a `type: date` field, including
//! date-math bounds (`now`, `now-Ny`). Validates the full PUT-index ->
//! bulk -> _search path on a clean date field (the deces DATE_NAISSANCE
//! stays `keyword` in matchID because the INSEE extract carries malformed
//! placeholder dates like `19530000` that a strict `type: date` would
//! reject — A7's date semantics target general OpenSearch date fields).

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use serde_json::Value;
use surch_api::app_router;
use tower::ServiceExt;

const INDEX: &str = "events";

const CREATE_BODY: &str = r#"{
  "mappings": {
    "properties": {
      "birth": { "type": "date", "format": "yyyyMMdd" },
      "label": { "type": "keyword" }
    }
  }
}"#;

// 3 docs: a 1941 birth, a 2020 birth, a 2024 birth.
const BULK_BODY: &str = "{\"index\":{\"_id\":\"1\",\"_index\":\"events\"}}\n{\"birth\":\"19410813\",\"label\":\"a\"}\n{\"index\":{\"_id\":\"2\",\"_index\":\"events\"}}\n{\"birth\":\"20200101\",\"label\":\"b\"}\n{\"index\":{\"_id\":\"3\",\"_index\":\"events\"}}\n{\"birth\":\"20240115\",\"label\":\"c\"}\n";

#[test]
fn date_range_with_literal_and_date_math_bounds() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let router = app_router();

    let (status, _) = runtime.block_on(execute(
        router.clone(),
        Method::PUT,
        &format!("/{INDEX}"),
        CREATE_BODY.to_string(),
    ));
    assert_eq!(status, StatusCode::OK.as_u16(), "create index OK");

    let (status, body) = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        "/_bulk",
        BULK_BODY.to_string(),
    ));
    assert_eq!(status, StatusCode::OK.as_u16(), "bulk OK: {body:?}");
    assert_eq!(
        body.as_ref().and_then(|v| v.get("errors")),
        Some(&Value::Bool(false)),
        "bulk errors=false"
    );
    let (status, _) = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        &format!("/{INDEX}/_refresh"),
        String::new(),
    ));
    assert_eq!(status, StatusCode::OK.as_u16(), "refresh OK");

    // Literal bounds: births in [1950-01-01, 2030-12-31] -> 2020 + 2024.
    assert_eq!(
        range_total(&runtime, &router, r#"{"gte":"19500101","lte":"20301231"}"#),
        2,
        "literal range 1950..2030 -> 2 docs"
    );
    // Strict upper bound excludes the equal date.
    assert_eq!(
        range_total(&runtime, &router, r#"{"lt":"20200101"}"#),
        1,
        "lt 20200101 -> only the 1941 doc"
    );
    // Date-math: everyone born in the last 200 years (robust for centuries).
    assert_eq!(
        range_total(&runtime, &router, r#"{"gte":"now-200y","lte":"now"}"#),
        3,
        "date-math now-200y..now -> all 3 docs"
    );
    // Date-math: nobody is born in the future.
    assert_eq!(
        range_total(&runtime, &router, r#"{"gt":"now"}"#),
        0,
        "date-math gt now -> no doc"
    );
}

fn range_total(runtime: &tokio::runtime::Runtime, router: &Router, bounds_json: &str) -> u64 {
    let body = format!(r#"{{"query":{{"range":{{"birth":{bounds_json}}}}},"size":10}}"#);
    let (status, body) = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        &format!("/{INDEX}/_search"),
        body,
    ));
    assert_eq!(status, StatusCode::OK.as_u16(), "search OK");
    body.expect("search body")
        .pointer("/hits/total/value")
        .and_then(Value::as_u64)
        .expect("hits.total.value present")
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
        Some(serde_json::from_slice::<Value>(&bytes).expect("body is JSON"))
    };
    (status, value)
}
