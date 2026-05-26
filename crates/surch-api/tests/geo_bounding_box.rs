//! A2 end-to-end: `geo_bounding_box` filter on a `type: geo_point` field.

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use serde_json::Value;
use surch_api::app_router;
use tower::ServiceExt;

const INDEX: &str = "places";

const CREATE_BODY: &str = r#"{
  "mappings": { "properties": { "loc": { "type": "geo_point" }, "name": { "type": "keyword" } } }
}"#;

// Paris, London, New York.
const BULK_BODY: &str = "{\"index\":{\"_id\":\"1\",\"_index\":\"places\"}}\n{\"name\":\"paris\",\"loc\":{\"lat\":48.85,\"lon\":2.35}}\n{\"index\":{\"_id\":\"2\",\"_index\":\"places\"}}\n{\"name\":\"london\",\"loc\":{\"lat\":51.5,\"lon\":-0.12}}\n{\"index\":{\"_id\":\"3\",\"_index\":\"places\"}}\n{\"name\":\"nyc\",\"loc\":{\"lat\":40.7,\"lon\":-74.0}}\n";

#[test]
fn geo_bounding_box_filters_points_inside_the_box() {
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

    // Western-Europe box contains Paris + London, not NYC.
    assert_eq!(
        box_total(&runtime, &router, 52.0, -1.0, 48.0, 3.0),
        2,
        "WE box -> paris + london"
    );
    // Tight box around Paris excludes London (lat 51.5 > 49).
    assert_eq!(
        box_total(&runtime, &router, 49.0, 2.0, 48.0, 3.0),
        1,
        "tight box -> paris only"
    );
    // A box over the mid-Atlantic contains nobody.
    assert_eq!(
        box_total(&runtime, &router, 45.0, -40.0, 30.0, -20.0),
        0,
        "atlantic box -> none"
    );
}

#[allow(clippy::too_many_arguments)]
fn box_total(
    runtime: &tokio::runtime::Runtime,
    router: &Router,
    top_lat: f64,
    left_lon: f64,
    bottom_lat: f64,
    right_lon: f64,
) -> u64 {
    let body = serde_json::json!({
        "query": {
            "geo_bounding_box": {
                "loc": {
                    "top_left": { "lat": top_lat, "lon": left_lon },
                    "bottom_right": { "lat": bottom_lat, "lon": right_lon }
                }
            }
        },
        "size": 10
    })
    .to_string();
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
