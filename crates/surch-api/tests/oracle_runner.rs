use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
    Router,
};
use opensearch_oracle::{
    dataset::{DatasetManifest, DatasetOperation, DatasetOperationKind},
    replay::{HttpMethod, ReplayManifest, ReplayRequest},
    runner::{run_replay, OracleResponse},
};
use serde_json::Value;
use surch_api::app_router;
use tower::ServiceExt;

#[test]
fn api_router_replays_bootstrap_oracle_manifest_in_memory() {
    let manifest = ReplayManifest::from_json_str(include_str!(
        "../../../tests/opensearch_compat/oracle/replays/api_bootstrap.json"
    ))
    .expect("bootstrap oracle replay should parse");
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    let router = app_router();

    let report = run_replay(&manifest, |request| {
        runtime.block_on(execute_replay_request(router.clone(), request))
    })
    .expect("bootstrap oracle replay should pass against in-memory router");

    assert_eq!(report.manifest_name, "api_bootstrap");
    assert_eq!(report.dataset, "none");
    assert_eq!(report.steps.len(), 3);
    assert_eq!(report.steps[0].request_name, "root");
    assert_eq!(report.steps[1].request_name, "count_empty_products");
    assert_eq!(report.steps[2].request_name, "search_empty_products");
}

#[test]
fn api_router_replays_ban_tiny_oracle_manifest_in_memory() {
    let dataset = DatasetManifest::from_json_str(include_str!(
        "../../../tests/opensearch_compat/oracle/datasets/ban/ban_tiny.json"
    ))
    .expect("BAN tiny dataset manifest should parse");
    let replay = ReplayManifest::from_json_str(include_str!(
        "../../../tests/opensearch_compat/oracle/replays/ban_tiny_search.json"
    ))
    .expect("BAN tiny replay manifest should parse");
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    let router = app_router();

    for operation in &dataset.operations {
        let response = runtime
            .block_on(execute_dataset_operation(router.clone(), operation))
            .expect("dataset operation should execute");
        assert_eq!(Some(response.status), operation.expected_status);
    }

    let report = run_replay(&replay, |request| {
        runtime.block_on(execute_replay_request(router.clone(), request))
    })
    .expect("BAN tiny replay should pass against in-memory router");

    assert_eq!(report.manifest_name, "ban_tiny_search");
    assert_eq!(report.dataset, "ban_tiny");
    assert_eq!(report.steps.len(), 4);
}

async fn execute_dataset_operation(
    router: Router,
    operation: &DatasetOperation,
) -> Result<OracleResponse, String> {
    let method = match operation.kind {
        DatasetOperationKind::CreateIndex => Method::PUT,
        DatasetOperationKind::Bulk => Method::POST,
        DatasetOperationKind::Refresh => Method::POST,
        DatasetOperationKind::DeleteIndex => Method::DELETE,
    };
    let body = match operation.body.as_deref() {
        Some("ban_tiny.ndjson") => {
            include_str!("../../../tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson")
                .to_string()
        }
        Some(body) => return Err(format!("unsupported dataset body fixture `{body}`")),
        None => String::new(),
    };

    execute_api_request(router, method, &operation.path, body).await
}

async fn execute_replay_request(
    router: Router,
    request: &ReplayRequest,
) -> Result<OracleResponse, String> {
    let body = request
        .body
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();

    execute_api_request(router, to_axum_method(&request.method), &request.path, body).await
}

async fn execute_api_request(
    router: Router,
    method: Method,
    path: &str,
    body: String,
) -> Result<OracleResponse, String> {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .map_err(|error| error.to_string())?,
        )
        .await
        .map_err(|error| error.to_string())?;

    let status = response.status().as_u16();
    let body_bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|error| error.to_string())?;
    let body = if body_bytes.is_empty() {
        None
    } else {
        Some(serde_json::from_slice::<Value>(&body_bytes).map_err(|error| error.to_string())?)
    };

    Ok(OracleResponse { status, body })
}

fn to_axum_method(method: &HttpMethod) -> Method {
    match method {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Delete => Method::DELETE,
    }
}
