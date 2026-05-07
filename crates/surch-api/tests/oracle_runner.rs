use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use opensearch_oracle::{
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

    let report = run_replay(&manifest, |request| {
        runtime.block_on(execute_api_request(request))
    })
    .expect("bootstrap oracle replay should pass against in-memory router");

    assert_eq!(report.manifest_name, "api_bootstrap");
    assert_eq!(report.dataset, "none");
    assert_eq!(report.steps.len(), 3);
    assert_eq!(report.steps[0].request_name, "root");
    assert_eq!(report.steps[1].request_name, "count_empty_products");
    assert_eq!(report.steps[2].request_name, "search_empty_products");
}

async fn execute_api_request(request: &ReplayRequest) -> Result<OracleResponse, String> {
    let method = to_axum_method(&request.method);
    let body = request
        .body
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();

    let response = app_router()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(&request.path)
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
