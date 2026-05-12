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

fn component_payload() -> serde_json::Value {
    serde_json::json!({
        "template": {
            "settings": {
                "number_of_replicas": 0
            },
            "mappings": {
                "properties": {
                    "message": {
                        "type": "text"
                    }
                }
            },
            "aliases": {
                "logs_current": {}
            }
        }
    })
}

fn assert_flexible_numeric_string(value: &serde_json::Value, expected: i64) {
    if let Some(number) = value.as_i64() {
        assert_eq!(number, expected);
    } else if let Some(text) = value.as_str() {
        assert_eq!(
            text.parse::<i64>().expect("setting should be numeric"),
            expected
        );
    } else {
        panic!("setting value should be numeric or numeric string");
    }
}

async fn put_component_template(
    router: &Router,
    name: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let (status, response) = put_component_template_response(router, name, body).await;
    assert_eq!(status, StatusCode::OK);
    response
}

async fn put_component_template_response(
    router: &Router,
    name: &str,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/_component_template/{name}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let status = response.status();
    (status, response_json(response).await)
}

async fn get_component_template(router: &Router, name: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/_component_template/{name}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let status = response.status();
    (status, response_json(response).await)
}

async fn list_component_templates(router: &Router) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_component_template")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    let status = response.status();
    (status, response_json(response).await)
}

async fn delete_component_template(router: &Router, name: &str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/_component_template/{name}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

async fn put_index_template_response(
    router: &Router,
    name: &str,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
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
    let status = response.status();
    (status, response_json(response).await)
}

async fn put_index_template(
    router: &Router,
    name: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let (status, response) = put_index_template_response(router, name, body).await;
    assert_eq!(status, StatusCode::OK);
    response
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
async fn component_template_router_accepts_payload_and_acknowledges() {
    let router = app_router();
    let response = put_component_template(&router, "base_logs", &component_payload()).await;
    assert_eq!(response["acknowledged"], serde_json::json!(true));
}

#[tokio::test]
async fn component_template_router_rejects_invalid_template_name() {
    let router = app_router();
    let (status, response) =
        put_component_template_response(&router, "bad,name", &component_payload()).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        response["error"]["reason"],
        "component template name contains invalid characters"
    );
}

#[tokio::test]
async fn component_template_router_get_one_returns_name_and_payload() {
    let router = app_router();
    let payload = component_payload();
    put_component_template(&router, "base_logs", &payload).await;

    let (status, response) = get_component_template(&router, "base_logs").await;
    assert_eq!(status, StatusCode::OK);

    let components = response["component_templates"]
        .as_array()
        .expect("component_templates should be an array");
    assert_eq!(components.len(), 1);
    assert_eq!(components[0]["name"], "base_logs");
    assert_eq!(components[0]["component_template"], payload);
}

#[tokio::test]
async fn component_template_router_lists_created_components() {
    let router = app_router();
    let payload = component_payload();
    put_component_template(&router, "base_logs", &payload).await;
    put_component_template(
        &router,
        "security_component",
        &serde_json::json!({
            "template": {
                "settings": {
                    "number_of_shards": 1
                }
            }
        }),
    )
    .await;

    let (status, response) = list_component_templates(&router).await;
    assert_eq!(status, StatusCode::OK);

    let names = response["component_templates"]
        .as_array()
        .expect("component_templates should be an array")
        .iter()
        .map(|component| {
            component["name"]
                .as_str()
                .expect("component name should be a string")
        })
        .collect::<Vec<&str>>();

    assert_eq!(names.len(), 2);
    assert!(names.contains(&"base_logs"));
    assert!(names.contains(&"security_component"));
}

#[tokio::test]
async fn component_template_router_delete_removes_template_and_returns_not_found() {
    let router = app_router();
    let payload = component_payload();
    put_component_template(&router, "base_logs", &payload).await;

    let delete_response = delete_component_template(&router, "base_logs").await;
    assert_eq!(delete_response["acknowledged"], serde_json::json!(true));

    let (status, response) = get_component_template(&router, "base_logs").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response["status"], serde_json::json!(404));
    assert!(response["error"]["type"].is_string());
    assert!(response["error"]["reason"].is_string());
}

#[tokio::test]
async fn index_template_router_composes_component_templates() {
    let router = app_router();
    let component = component_payload();
    put_component_template(&router, "base_logs", &component).await;

    let index_template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "composed_of": ["base_logs"],
        "template": {
            "settings": {
                "number_of_shards": 2
            }
        }
    });
    put_index_template(&router, "logs_template", &index_template).await;

    create_index(&router, "logs-2026", "").await;

    let (_status, response) = get_index(&router, "logs-2026").await;
    let logs_2026 = &response["logs-2026"];

    let settings = &logs_2026["settings"]["index"];
    assert_flexible_numeric_string(&settings["number_of_replicas"], 0);
    assert_flexible_numeric_string(&settings["number_of_shards"], 2);
    assert_eq!(
        logs_2026["mappings"]["properties"]["message"]["type"],
        serde_json::json!("text")
    );
    assert_eq!(
        logs_2026["aliases"],
        serde_json::json!({"logs_current": {}})
    );
}

#[tokio::test]
async fn index_template_composition_allows_inline_and_create_overrides() {
    let router = app_router();
    put_component_template(
        &router,
        "base_logs",
        &serde_json::json!({
            "template": {
                "settings": {
                    "number_of_replicas": 0,
                    "refresh_interval": "30s"
                },
                "mappings": {
                    "properties": {
                        "message": {
                            "type": "text"
                        },
                        "component_only": {
                            "type": "keyword"
                        }
                    }
                },
                "aliases": {
                    "logs_current": {
                        "routing": "component"
                    }
                }
            }
        }),
    )
    .await;

    let index_template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "composed_of": ["base_logs"],
        "template": {
            "settings": {
                "number_of_replicas": 1
            },
            "mappings": {
                "properties": {
                    "message": {
                        "type": "keyword"
                    }
                }
            },
            "aliases": {
                "logs_current": {
                    "routing": "inline"
                },
                "logs_search": {
                    "is_write_index": false
                }
            }
        }
    });
    put_index_template(&router, "logs_template", &index_template).await;

    create_index(
        &router,
        "logs-2026",
        r#"{
            "settings": {
                "number_of_shards": 3
            },
            "mappings": {
                "properties": {
                    "create_only": {
                        "type": "keyword"
                    }
                }
            },
            "aliases": {
                "logs_current": {
                    "routing": "create"
                }
            }
        }"#,
    )
    .await;

    let (_status, response) = get_index(&router, "logs-2026").await;
    let logs_2026 = &response["logs-2026"];

    assert_flexible_numeric_string(&logs_2026["settings"]["index"]["number_of_replicas"], 1);
    assert_flexible_numeric_string(&logs_2026["settings"]["index"]["number_of_shards"], 3);
    assert_eq!(
        logs_2026["settings"]["index"]["refresh_interval"],
        serde_json::json!("30s")
    );
    assert_eq!(
        logs_2026["mappings"]["properties"]["message"]["type"],
        serde_json::json!("keyword")
    );
    assert_eq!(
        logs_2026["mappings"]["properties"]["component_only"]["type"],
        serde_json::json!("keyword")
    );
    assert_eq!(
        logs_2026["mappings"]["properties"]["create_only"]["type"],
        serde_json::json!("keyword")
    );
    assert_eq!(
        logs_2026["aliases"],
        serde_json::json!({
            "logs_current": {
                "routing": "create"
            },
            "logs_search": {
                "is_write_index": false
            }
        })
    );
}

#[tokio::test]
async fn index_template_composition_is_snapshotted_when_component_is_deleted() {
    let router = app_router();
    let component = component_payload();
    put_component_template(&router, "base_logs", &component).await;

    let index_template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "composed_of": ["base_logs"]
    });
    put_index_template(&router, "logs_template", &index_template).await;
    delete_component_template(&router, "base_logs").await;

    create_index(&router, "logs-2026", "").await;

    let (_status, response) = get_index(&router, "logs-2026").await;
    let logs_2026 = &response["logs-2026"];

    assert_flexible_numeric_string(&logs_2026["settings"]["index"]["number_of_replicas"], 0);
    assert_eq!(
        logs_2026["mappings"]["properties"]["message"]["type"],
        serde_json::json!("text")
    );
    assert_eq!(
        logs_2026["aliases"],
        serde_json::json!({"logs_current": {}})
    );
}

#[tokio::test]
async fn index_template_router_rejects_missing_composed_of_component() {
    let router = app_router();
    let index_template = serde_json::json!({
        "index_patterns": ["logs-*"],
        "composed_of": ["missing_component"],
        "template": {
            "settings": {
                "number_of_shards": 2
            }
        }
    });

    let (status, response) =
        put_index_template_response(&router, "logs_template", &index_template).await;
    assert!(!status.is_success());
    assert!(response["error"]["type"].is_string());
}
