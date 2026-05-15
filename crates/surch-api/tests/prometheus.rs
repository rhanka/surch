use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use surch_api::app_router;
use tower::ServiceExt;

/// `GET /_prometheus_metrics` responds with the Prometheus text
/// exposition format. The endpoint exists from the first request on
/// because `app_router` installs the global recorder eagerly.
#[tokio::test]
async fn prometheus_endpoint_returns_text_exposition() {
    let router = app_router();

    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_prometheus_metrics")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .expect("response should carry Content-Type")
        .to_str()
        .expect("Content-Type should be ASCII");
    assert!(
        content_type.starts_with("text/plain"),
        "Prometheus exposition Content-Type should start with 'text/plain', got {content_type:?}",
    );

    let body_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    let body =
        String::from_utf8(body_bytes.to_vec()).expect("Prometheus exposition body should be UTF-8");

    // Sanity: the response is at least syntactically a Prometheus
    // body (it is well-formed UTF-8 and contains either zero or
    // multiple newline-terminated lines, never a partial first line).
    assert!(
        body.is_empty() || body.ends_with('\n'),
        "non-empty Prometheus exposition should end with a newline, got: {body:?}",
    );
}

/// After issuing a `_search` call, the three minimum-viable metrics
/// (`surch_search_total`, `surch_search_cache_hit_total`,
/// `surch_search_duration_seconds`) all appear in the scrape body.
///
/// This guards the wiring between `search_handler` and the global
/// Prometheus recorder: if a future refactor drops one of the
/// `metrics::counter!` or `metrics::histogram!` calls, this test
/// goes red.
#[tokio::test]
async fn prometheus_endpoint_exposes_search_metrics() {
    let router = app_router();

    // Index one document so the `_search` call below actually runs
    // the scoring path and exercises the cache-fill branch.
    let index_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_doc/sku-1")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"desk"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(index_response.status(), StatusCode::CREATED);

    // First search: warms the cache and increments `surch_search_total`
    // + records the histogram.
    let first_search = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match":{"name":"desk"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(first_search.status(), StatusCode::OK);

    // Second identical search: hits the response cache and increments
    // `surch_search_cache_hit_total`.
    let second_search = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/products/_search")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":{"match":{"name":"desk"}}}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(second_search.status(), StatusCode::OK);

    // Scrape.
    let scrape = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_prometheus_metrics")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(scrape.status(), StatusCode::OK);

    let body_bytes = to_bytes(scrape.into_body(), usize::MAX)
        .await
        .expect("scrape body should be readable");
    let body = String::from_utf8(body_bytes.to_vec()).expect("scrape body should be UTF-8");

    for metric in [
        "surch_search_total",
        "surch_search_cache_hit_total",
        "surch_search_duration_seconds",
    ] {
        assert!(
            body.contains(metric),
            "scrape body should mention `{metric}`, got:\n{body}",
        );
    }
}
