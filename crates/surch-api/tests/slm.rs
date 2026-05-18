use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::{json, Value};
use surch_api::app_router;
use tower::ServiceExt;

fn tempdir(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "surch-slm-{}-{}-{}",
        label,
        std::process::id(),
        nanos,
    ));
    std::fs::create_dir_all(&dir).expect("temp dir creation");
    dir
}

async fn read_json(response: axum::response::Response<Body>) -> (StatusCode, Value) {
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    let json = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).expect("response body should be json")
    };
    (status, json)
}

async fn request(
    router: &axum::Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
    } else {
        builder.body(Body::empty())
    }
    .expect("request should build");

    read_json(
        router
            .clone()
            .oneshot(request)
            .await
            .expect("router should respond"),
    )
    .await
}

fn valid_policy(repository: &str) -> Value {
    json!({
        "schedule": "0 30 1 * * ?",
        "name": "<daily-{now/d}>",
        "repository": repository,
        "config": {
            "indices": ["logs-*"],
            "include_global_state": false
        },
        "retention": {
            "expire_after": "30d",
            "min_count": 1,
            "max_count": 10
        }
    })
}

#[tokio::test]
async fn slm_put_valid_policy_acknowledges() {
    let router = app_router();

    let (status, body) = request(
        &router,
        Method::PUT,
        "/_slm/policy/daily",
        Some(valid_policy("missing-repo")),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "acknowledged": true }));
}

#[tokio::test]
async fn slm_get_policy_returns_policy_and_next_execution() {
    let router = app_router();
    let (status, _) = request(
        &router,
        Method::PUT,
        "/_slm/policy/daily",
        Some(valid_policy("missing-repo")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = request(&router, Method::GET, "/_slm/policy/daily", None).await;

    assert_eq!(status, StatusCode::OK);
    let policy = &body["daily"];
    assert_eq!(policy["policy"]["repository"], json!("missing-repo"));
    assert_eq!(policy["policy"]["name"], json!("<daily-{now/d}>"));
    assert!(
        policy["next_execution_millis"].is_i64(),
        "next_execution_millis should be set: {body}"
    );
}

#[tokio::test]
async fn slm_put_invalid_cron_returns_illegal_argument_exception() {
    let router = app_router();
    let mut policy = valid_policy("missing-repo");
    policy["schedule"] = json!("not a cron");

    let (status, body) = request(&router, Method::PUT, "/_slm/policy/bad", Some(policy)).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], json!("illegal_argument_exception"));
}

#[tokio::test]
async fn slm_delete_policy_acknowledges_then_get_returns_not_found() {
    let router = app_router();
    let (status, _) = request(
        &router,
        Method::PUT,
        "/_slm/policy/daily",
        Some(valid_policy("missing-repo")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = request(&router, Method::DELETE, "/_slm/policy/daily", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "acknowledged": true }));

    let (status, body) = request(&router, Method::GET, "/_slm/policy/daily", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["type"], json!("resource_not_found_exception"));
}

#[tokio::test]
async fn slm_execute_missing_repository_returns_controlled_error_and_records_failure() {
    let router = app_router();
    let (status, _) = request(
        &router,
        Method::PUT,
        "/_slm/policy/daily",
        Some(valid_policy("missing-repo")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = request(&router, Method::POST, "/_slm/policy/daily/_execute", None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"]["type"], json!("snapshot_exception"));
    assert!(
        body["error"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("repository [missing-repo] missing")),
        "unexpected execute error body: {body}"
    );

    let (status, body) =
        request(&router, Method::GET, "/_slm/policy/daily/_executions", None).await;
    assert_eq!(status, StatusCode::OK);
    let executions = body["executions"]
        .as_array()
        .expect("executions should be an array");
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0]["state"], json!("FAILED"));
    assert_eq!(
        executions[0]["error"],
        json!("repository [missing-repo] missing")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn slm_execute_retention_prunes_old_successful_snapshots_by_max_count() {
    let router = app_router();
    let repo_dir = tempdir("retention-max-count");

    let (status, body) = request(
        &router,
        Method::PUT,
        "/_snapshot/local",
        Some(json!({
            "type": "fs",
            "settings": { "location": repo_dir.display().to_string() }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "put repo body: {body}");

    let mut first = valid_policy("local");
    first["name"] = json!("daily-old");
    first["config"]["indices"] = json!([]);
    first["retention"] = json!({ "max_count": 1 });
    let (status, body) = request(&router, Method::PUT, "/_slm/policy/daily", Some(first)).await;
    assert_eq!(status, StatusCode::OK, "put first policy body: {body}");
    let (status, body) = request(&router, Method::POST, "/_slm/policy/daily/_execute", None).await;
    assert_eq!(status, StatusCode::OK, "execute first body: {body}");

    let mut second = valid_policy("local");
    second["name"] = json!("daily-new");
    second["config"]["indices"] = json!([]);
    second["retention"] = json!({ "max_count": 1 });
    let (status, body) = request(&router, Method::PUT, "/_slm/policy/daily", Some(second)).await;
    assert_eq!(status, StatusCode::OK, "put second policy body: {body}");
    let (status, body) = request(&router, Method::POST, "/_slm/policy/daily/_execute", None).await;
    assert_eq!(status, StatusCode::OK, "execute second body: {body}");

    let (status, body) = request(&router, Method::GET, "/_snapshot/local/daily-old", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["type"], json!("snapshot_missing_exception"));

    let (status, body) = request(&router, Method::GET, "/_snapshot/local/daily-new", None).await;
    assert_eq!(status, StatusCode::OK, "new snapshot body: {body}");
    assert_eq!(body["snapshots"][0]["snapshot"], json!("daily-new"));
}
