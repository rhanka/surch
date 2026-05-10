use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use surch_api::app_router;
use tower::ServiceExt;

async fn response_json(response: axum::response::Response<Body>) -> serde_json::Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be json")
}

async fn post_analyze(router: &Router, path: &str, body: &'static str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

#[tokio::test]
async fn analyze_uses_standard_analyzer_by_default() {
    let body = post_analyze(&app_router(), "/_analyze", r#"{"text":"Hello World"}"#).await;
    let tokens = body["tokens"].as_array().expect("tokens array");
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0]["token"], "hello");
    assert_eq!(tokens[0]["start_offset"], 0);
    assert_eq!(tokens[0]["end_offset"], 5);
    assert_eq!(tokens[0]["position"], 0);
    assert_eq!(tokens[1]["token"], "world");
    assert_eq!(tokens[1]["start_offset"], 6);
    assert_eq!(tokens[1]["end_offset"], 11);
    assert_eq!(tokens[1]["position"], 1);
}

#[tokio::test]
async fn analyze_keeps_full_input_with_keyword_analyzer() {
    let body = post_analyze(
        &app_router(),
        "/_analyze",
        r#"{"analyzer":"keyword","text":"Trail Running Shoes"}"#,
    )
    .await;
    let tokens = body["tokens"].as_array().expect("tokens array");
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0]["token"], "Trail Running Shoes");
    assert_eq!(tokens[0]["start_offset"], 0);
    assert_eq!(tokens[0]["end_offset"], 19);
}

#[tokio::test]
async fn analyze_skips_stop_words_with_stop_analyzer_and_advances_position() {
    let body = post_analyze(
        &app_router(),
        "/_analyze",
        r#"{"analyzer":"stop","text":"the rust search"}"#,
    )
    .await;
    let tokens = body["tokens"].as_array().expect("tokens array");
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0]["token"], "rust");
    assert_eq!(tokens[1]["token"], "search");
    let position_first = tokens[0]["position"].as_i64().expect("position int");
    let position_second = tokens[1]["position"].as_i64().expect("position int");
    assert!(position_second > position_first);
}

#[tokio::test]
async fn analyze_concatenates_multiple_texts_with_continuous_offsets() {
    let body = post_analyze(&app_router(), "/_analyze", r#"{"text":["hello","world"]}"#).await;
    let tokens = body["tokens"].as_array().expect("tokens array");
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0]["start_offset"], 0);
    assert_eq!(tokens[0]["end_offset"], 5);
    assert_eq!(tokens[1]["start_offset"], 5);
    assert_eq!(tokens[1]["end_offset"], 10);
}

#[tokio::test]
async fn analyze_rejects_unknown_analyzer_with_opensearch_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_analyze")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"analyzer":"french","text":"bonjour"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "error": {
                "type": "parsing_exception",
                "reason": "unknown analyzer `french`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn analyze_rejects_missing_text_with_opensearch_error() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_analyze")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"analyzer":"standard"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({
            "error": {
                "type": "parsing_exception",
                "reason": "_analyze request body must contain `text`"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn analyze_returns_404_for_missing_index() {
    let response = app_router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/missing/_analyze")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"hello"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(response).await["error"]["type"],
        "index_not_found_exception"
    );
}
