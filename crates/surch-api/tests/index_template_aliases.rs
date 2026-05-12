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

async fn put_index_template(
    router: &Router,
    name: &str,
    payload: &serde_json::Value,
) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/_index_template/{name}"))
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn create_index(router: &Router, index: &str, body: &str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/{index}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn put_document(router: &Router, index: &str, id: &str, body: &str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/{index}/_doc/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::CREATED);
    response_json(response).await
}

async fn bulk(router: &Router, body: &str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/_bulk")
                .header("content-type", "application/x-ndjson")
                .body(Body::from(body.to_owned()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn get_alias(router: &Router, name: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/_alias/{name}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let status = response.status();
    (status, response_json(response).await)
}

#[tokio::test]
async fn template_aliases_reject_empty_alias_name() {
    let router = app_router();
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/_index_template/logs_template")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "index_patterns": ["logs-*"],
                        "template": {
                            "aliases": {
                                "": {}
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
    assert_eq!(
        body["error"]["reason"],
        "_index_template `template.aliases` names must not be empty"
    );
}

#[tokio::test]
async fn template_aliases_reject_non_object_alias_body() {
    let router = app_router();
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri("/_index_template/logs_template")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "index_patterns": ["logs-*"],
                        "template": {
                            "aliases": {
                                "logs_current": true
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["type"], "parsing_exception");
    assert_eq!(
        body["error"]["reason"],
        "_index_template `template.aliases.logs_current` must be an object"
    );
}

#[tokio::test]
async fn index_template_aliases_apply_on_explicit_index_creation() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "aliases": {
                "logs_current": {}
            }
        }
    });
    let create_template = put_index_template(&router, "logs_template", &template).await;
    assert_eq!(create_template, serde_json::json!({"acknowledged": true}));

    let create_response = create_index(&router, "logs-2026", "").await;
    assert_eq!(create_response["acknowledged"], serde_json::json!(true));

    let (status, aliases) = get_alias(&router, "logs_current").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        aliases,
        serde_json::json!({
            "logs-2026": { "aliases": {"logs_current": {}}}
        })
    );
}

#[tokio::test]
async fn template_aliases_apply_when_index_created_implicitly_via_document() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "aliases": {
                "logs_current": {}
            }
        }
    });
    put_index_template(&router, "logs_template", &template).await;

    let create_response = put_document(&router, "logs-2027", "1", r#"{"level":"INFO"}"#).await;
    assert_eq!(create_response["result"], "created");

    let (status, aliases) = get_alias(&router, "logs_current").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        aliases,
        serde_json::json!({
            "logs-2027": { "aliases": {"logs_current": {}}}
        })
    );
}

#[tokio::test]
async fn template_aliases_apply_when_index_created_implicitly_via_bulk() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "aliases": {
                "logs_current": {}
            }
        }
    });
    put_index_template(&router, "logs_template", &template).await;

    let bulk_response = bulk(
        &router,
        r#"{"create":{"_index":"logs-2028","_id":"1"}}
{"level":"INFO"}
"#,
    )
    .await;
    assert_eq!(bulk_response["errors"], false);

    let (status, aliases) = get_alias(&router, "logs_current").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        aliases,
        serde_json::json!({
            "logs-2028": { "aliases": {"logs_current": {}}}
        })
    );
}

#[tokio::test]
async fn template_aliases_do_not_apply_to_non_matching_index() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "aliases": {
                "logs_current": {}
            }
        }
    });
    put_index_template(&router, "logs_template", &template).await;

    let create_response = create_index(&router, "metrics-2026", "").await;
    assert_eq!(create_response["acknowledged"], serde_json::json!(true));

    let (status, body) = get_alias(&router, "logs_current").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["status"], serde_json::json!(404));
}
