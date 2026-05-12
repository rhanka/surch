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

fn base_template_payload() -> serde_json::Value {
    serde_json::json!({
        "index_patterns": ["products-*"],
        "template": {
            "settings": {
                "number_of_shards": 1,
            },
            "mappings": {
                "properties": {
                    "name": {
                        "type": "text"
                    }
                }
            }
        }
    })
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

async fn get_index_template(router: &Router, name: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/_index_template/{name}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    let status = response.status();
    let body = response_json(response).await;
    (status, body)
}

async fn get_all_index_templates(router: &Router) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_index_template")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    let status = response.status();
    let body = response_json(response).await;
    (status, body)
}

async fn delete_index_template(router: &Router, name: &str) -> serde_json::Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/_index_template/{name}"))
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);

    response_json(response).await
}

#[tokio::test]
async fn index_template_router_accepts_minimal_template_payload() {
    let router = app_router();
    let response = put_index_template(&router, "products_template", &base_template_payload()).await;

    assert_eq!(response["acknowledged"], true);
}

#[tokio::test]
async fn index_template_router_get_one_returns_name_and_payload() {
    let router = app_router();
    let template_payload = base_template_payload();
    put_index_template(&router, "products_template", &template_payload).await;

    let (status, response) = get_index_template(&router, "products_template").await;
    assert_eq!(status, StatusCode::OK);

    let templates = response["index_templates"]
        .as_array()
        .expect("index_templates should be an array");
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0]["name"], "products_template");
    assert_eq!(
        templates[0]["index_template"]["index_patterns"],
        template_payload["index_patterns"]
    );
    assert_eq!(
        templates[0]["index_template"]["template"],
        template_payload["template"]
    );
}

#[tokio::test]
async fn index_template_router_get_all_returns_created_templates() {
    let router = app_router();
    put_index_template(&router, "products_template", &base_template_payload()).await;
    put_index_template(
        &router,
        "logs_template",
        &serde_json::json!({
            "index_patterns": ["logs-*"],
            "template": {
                "settings": {
                    "number_of_shards": 1,
                },
                "mappings": {
                    "properties": {
                        "event": {
                            "type": "keyword"
                        }
                    }
                }
            }
        }),
    )
    .await;

    let (status, response) = get_all_index_templates(&router).await;
    assert_eq!(status, StatusCode::OK);

    let templates = response["index_templates"]
        .as_array()
        .expect("index_templates should be an array");
    let names: Vec<&str> = templates
        .iter()
        .map(|template| {
            template["name"]
                .as_str()
                .expect("template name should be a string")
        })
        .collect();

    assert_eq!(names.len(), 2);
    assert!(names.contains(&"products_template"));
    assert!(names.contains(&"logs_template"));
}

#[tokio::test]
async fn index_template_router_delete_removes_template() {
    let router = app_router();
    put_index_template(&router, "products_template", &base_template_payload()).await;

    let delete_response = delete_index_template(&router, "products_template").await;
    assert_eq!(delete_response["acknowledged"], true);

    let (status, response) = get_index_template(&router, "products_template").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(response["status"], 404);
    assert!(response["error"]["type"].is_string());
    assert!(response["error"]["reason"].is_string());
}

#[tokio::test]
async fn index_template_router_get_unknown_template_returns_not_found_error_payload() {
    let router = app_router();
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_index_template/missing-template")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response_json(response).await["status"], 404);
}
