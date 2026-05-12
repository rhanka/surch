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

async fn mapping(router: &Router, index: &str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/{index}/_mapping"))
                .body(Body::empty())
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

#[tokio::test]
async fn index_creation_applies_matching_template_mapping() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "mappings": {
                "properties": {
                    "message": {
                        "type": "text"
                    }
                }
            }
        }
    });

    let put_response = put_index_template(&router, "logs_template", &template).await;
    assert_eq!(put_response, serde_json::json!({"acknowledged": true}));

    let create_response = create_index(&router, "logs-2026", "");
    assert_eq!(
        create_response.await["acknowledged"],
        serde_json::json!(true)
    );

    let mapping_response = mapping(&router, "logs-2026").await;
    assert_eq!(
        mapping_response["logs-2026"]["mappings"]["properties"]["message"]["type"],
        "text"
    );
}

#[tokio::test]
async fn higher_priority_template_wins_conflicting_mapping_fields() {
    let router = app_router();
    put_index_template(
        &router,
        "z_low_priority",
        &serde_json::json!({
            "index_patterns": ["logs-*"],
            "priority": 1,
            "template": {
                "mappings": {
                    "properties": {
                        "level": {
                            "type": "text"
                        }
                    }
                }
            }
        }),
    )
    .await;
    put_index_template(
        &router,
        "a_high_priority",
        &serde_json::json!({
            "index_patterns": ["logs-*"],
            "priority": 100,
            "template": {
                "mappings": {
                    "properties": {
                        "level": {
                            "type": "keyword"
                        }
                    }
                }
            }
        }),
    )
    .await;

    create_index(&router, "logs-2026", "").await;

    let mapping_response = mapping(&router, "logs-2026").await;
    assert_eq!(
        mapping_response["logs-2026"]["mappings"]["properties"]["level"]["type"],
        "keyword"
    );
}

#[tokio::test]
async fn explicit_mapping_wins_over_matching_template() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "mappings": {
                "properties": {
                    "message": {
                        "type": "text"
                    }
                }
            }
        }
    });
    put_index_template(&router, "logs_template", &template).await;

    let create_response = create_index(
        &router,
        "logs-2026",
        r#"{"mappings":{"properties":{"message":{"type":"keyword"}}}}"#,
    )
    .await;
    assert_eq!(create_response["acknowledged"], serde_json::json!(true));

    let mapping_response = mapping(&router, "logs-2026").await;
    assert_eq!(
        mapping_response["logs-2026"]["mappings"]["properties"]["message"]["type"],
        "keyword"
    );
}

#[tokio::test]
async fn document_indexing_applies_template_when_index_is_created_implicitly() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "mappings": {
                "properties": {
                    "level": {
                        "type": "keyword"
                    }
                }
            }
        }
    });
    put_index_template(&router, "logs_template", &template).await;

    put_document(&router, "logs-2026", "1", r#"{"level":"INFO"}"#).await;

    let mapping_response = mapping(&router, "logs-2026").await;
    assert_eq!(
        mapping_response["logs-2026"]["mappings"]["properties"]["level"]["type"],
        "keyword"
    );
}

#[tokio::test]
async fn bulk_create_applies_template_when_index_is_created_implicitly() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "mappings": {
                "properties": {
                    "level": {
                        "type": "keyword"
                    }
                }
            }
        }
    });
    put_index_template(&router, "logs_template", &template).await;

    let bulk_response = bulk(
        &router,
        r#"{"create":{"_index":"logs-2026","_id":"1"}}
{"level":"INFO"}
"#,
    )
    .await;
    assert_eq!(bulk_response["errors"], false);

    let mapping_response = mapping(&router, "logs-2026").await;
    assert_eq!(
        mapping_response["logs-2026"]["mappings"]["properties"]["level"]["type"],
        "keyword"
    );
}

#[tokio::test]
async fn non_matching_index_pattern_does_not_apply_template() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "mappings": {
                "properties": {
                    "message": {
                        "type": "text"
                    }
                }
            }
        }
    });
    put_index_template(&router, "logs_template", &template).await;

    create_index(&router, "metrics-2026", "").await;

    let mapping_response = mapping(&router, "metrics-2026").await;
    let properties = mapping_response["metrics-2026"]["mappings"]["properties"].as_object();
    assert!(
        properties.is_none()
            || !properties
                .expect("properties should be an object")
                .contains_key("message")
    );
}
