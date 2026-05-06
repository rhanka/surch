use axum::{
    body::{to_bytes, Body},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use surch_api::bulk::bulk_handler;

async fn response_json(response: Response<Body>) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    let json = serde_json::from_slice(&body).expect("response body should be json");

    (status, json)
}

#[tokio::test]
async fn bulk_http_handler_returns_success_response_for_http_fixture() {
    let body = include_str!("../../../tests/opensearch_compat/bulk/http_bulk_request.ndjson");
    let expected_response: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/bulk/http_bulk_response.json"
    ))
    .expect("response fixture should be valid json");

    let response = bulk_handler(body.to_owned()).await.into_response();
    let (status, json) = response_json(response).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json, expected_response);
}

#[tokio::test]
async fn bulk_http_handler_returns_bad_request_for_unknown_action() {
    let response = bulk_handler(r#"{"noop":{"_index":"books","_id":"1"}}"#.to_owned())
        .await
        .into_response();
    let (status, json) = response_json(response).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json,
        json!({
            "error": {
                "type": "parse_exception",
                "reason": "unknown bulk action `noop` at line 1"
            },
            "status": 400
        })
    );
}

#[tokio::test]
async fn bulk_http_handler_returns_bad_request_for_missing_source() {
    let response = bulk_handler(r#"{"index":{"_index":"books","_id":"1"}}"#.to_owned())
        .await
        .into_response();
    let (status, json) = response_json(response).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json,
        json!({
            "error": {
                "type": "parse_exception",
                "reason": "missing source line after `index` action at line 1"
            },
            "status": 400
        })
    );
}
