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
    if body.is_empty() {
        return serde_json::json!(null);
    }
    serde_json::from_slice(&body).expect("response body should be json")
}

async fn put_index(router: &Router, index: &str, body: &str) -> serde_json::Value {
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

async fn put_index_template(
    router: &Router,
    name: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/_index_template/{name}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn get_index(router: &Router, index: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/{index}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let status = response.status();
    (status, response_json(response).await)
}

#[tokio::test]
async fn index_metadata_router_returns_settings_mappings_and_aliases() {
    let router = app_router();
    let body = serde_json::json!({
        "settings": {
            "number_of_shards": 3
        },
        "mappings": {
            "properties": {
                "name": {
                    "type": "text"
                }
            }
        },
        "aliases": {
            "products_current": {}
        }
    });

    let create_response = put_index(&router, "products", &body.to_string()).await;
    assert_eq!(create_response["acknowledged"], serde_json::json!(true));
    assert_eq!(
        create_response["shards_acknowledged"],
        serde_json::json!(true)
    );
    assert_eq!(create_response["index"], "products");

    let (status, get_response) = get_index(&router, "products").await;
    assert_eq!(status, StatusCode::OK);
    let products = &get_response["products"];

    assert_eq!(
        products["mappings"],
        serde_json::json!({
            "properties": {
                "name": {
                    "type": "text"
                }
            }
        })
    );
    assert_eq!(
        products["aliases"],
        serde_json::json!({"products_current": {}})
    );

    let shards = &products["settings"]["index"]["number_of_shards"];
    if let Some(number) = shards.as_u64() {
        assert_eq!(number, 3);
    } else if let Some(string) = shards.as_str() {
        assert_eq!(string, "3");
    } else {
        panic!("settings.index.number_of_shards should be numeric 3 or string \"3\"");
    }
}

#[tokio::test]
async fn index_metadata_router_returns_missing_index_as_opensearch_error() {
    let (status, response) = get_index(&app_router(), "missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response["status"], serde_json::json!(404));
    assert_eq!(response["error"]["type"], "index_not_found_exception");
}

#[tokio::test]
async fn index_metadata_router_fans_out_alias_targets() {
    let router = app_router();
    put_index(
        &router,
        "products_v1",
        r#"{"aliases":{"products_current":{}}}"#,
    )
    .await;
    put_index(
        &router,
        "products_v2",
        r#"{"aliases":{"products_current":{}}}"#,
    )
    .await;

    let (status, response) = get_index(&router, "products_current").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        response["products_v1"]["aliases"],
        serde_json::json!({"products_current": {}})
    );
    assert_eq!(
        response["products_v2"]["aliases"],
        serde_json::json!({"products_current": {}})
    );
}

#[tokio::test]
async fn index_metadata_router_inherits_template_metadata() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "settings": {
                "number_of_replicas": 0
            },
            "aliases": {
                "logs_current": {}
            },
            "mappings": {
                "properties": {
                    "message": {
                        "type": "text"
                    }
                }
            }
        }
    });
    let template_response = put_index_template(&router, "logs_template", &template).await;
    assert_eq!(template_response["acknowledged"], serde_json::json!(true));

    let create_response = put_index(&router, "logs-2026", "").await;
    assert_eq!(create_response["acknowledged"], serde_json::json!(true));

    let (_status, get_response) = get_index(&router, "logs-2026").await;
    assert_eq!(
        get_response["logs-2026"]["settings"]["index"]["number_of_replicas"],
        serde_json::json!(0)
    );
    assert_eq!(
        get_response["logs-2026"]["mappings"]["properties"]["message"]["type"],
        "text"
    );
    assert_eq!(
        get_response["logs-2026"]["aliases"],
        serde_json::json!({"logs_current": {}})
    );
}

#[tokio::test]
async fn index_metadata_router_allows_template_settings_override_on_create() {
    let router = app_router();
    let template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "template": {
            "settings": {
                "number_of_replicas": 2
            },
            "aliases": {
                "logs_current": {}
            },
            "mappings": {
                "properties": {
                    "message": {
                        "type": "text"
                    }
                }
            }
        }
    });
    let template_response = put_index_template(&router, "logs_template", &template).await;
    assert_eq!(template_response["acknowledged"], serde_json::json!(true));

    let create_response = put_index(
        &router,
        "logs-2026",
        r#"{"settings":{"number_of_replicas":1},"mappings":{"properties":{"message":{"type":"text"}}}}"#,
    )
    .await;
    assert_eq!(create_response["acknowledged"], serde_json::json!(true));

    let (_status, get_response) = get_index(&router, "logs-2026").await;
    assert_eq!(
        get_response["logs-2026"]["settings"]["index"]["number_of_replicas"],
        serde_json::json!(1)
    );
    assert_eq!(
        get_response["logs-2026"]["aliases"],
        serde_json::json!({"logs_current": {}})
    );
    assert_eq!(
        get_response["logs-2026"]["mappings"]["properties"]["message"]["type"],
        "text"
    );
}
