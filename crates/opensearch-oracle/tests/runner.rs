use opensearch_oracle::{
    replay::{HttpMethod, ReplayManifest},
    runner::{run_replay, OracleResponse, ReplayRunError},
};
use serde_json::json;

#[test]
fn run_replay_executes_requests_in_order_and_compares_expected_responses() {
    let manifest = ReplayManifest::from_json_str(
        r#"{
            "name": "runner_smoke",
            "dataset": "products_basic",
            "comparison": {
                "ignored_paths": ["took"],
                "score_tolerance": 0.01
            },
            "requests": [
                {
                    "name": "index_product",
                    "method": "PUT",
                    "path": "/products/_doc/1",
                    "body": {"name": "Rust Search"},
                    "expected_status": 201,
                    "expected_response": {"result": "created"}
                },
                {
                    "name": "search_product",
                    "method": "POST",
                    "path": "/products/_search",
                    "body": {"query": {"match": {"name": "rust"}}},
                    "expected_status": 200,
                    "expected_response": {
                        "took": 1,
                        "hits": {
                            "max_score": 1.01,
                            "hits": [{"_id": "1", "_score": 1.01}]
                        }
                    }
                }
            ]
        }"#,
    )
    .expect("manifest should parse");
    let mut seen = Vec::new();

    let report = run_replay(&manifest, |request| {
        seen.push((request.method.clone(), request.path.clone()));
        match request.name.as_str() {
            "index_product" => Ok(OracleResponse {
                status: 201,
                body: Some(json!({"result": "created"})),
            }),
            "search_product" => Ok(OracleResponse {
                status: 200,
                body: Some(json!({
                    "took": 7,
                    "hits": {
                        "max_score": 1.015,
                        "hits": [{"_id": "1", "_score": 1.015}]
                    }
                })),
            }),
            other => panic!("unexpected request {other}"),
        }
    })
    .expect("replay should pass");

    assert_eq!(
        seen,
        vec![
            (HttpMethod::Put, "/products/_doc/1".to_string()),
            (HttpMethod::Post, "/products/_search".to_string())
        ]
    );
    assert_eq!(report.manifest_name, "runner_smoke");
    assert_eq!(report.dataset, "products_basic");
    assert_eq!(report.steps.len(), 2);
    assert_eq!(report.steps[0].request_name, "index_product");
    assert_eq!(report.steps[0].status, 201);
    assert_eq!(report.steps[1].request_name, "search_product");
    assert_eq!(report.steps[1].status, 200);
}

#[test]
fn run_replay_reports_status_mismatch_with_request_name() {
    let manifest = ReplayManifest::from_json_str(
        r#"{
            "name": "runner_status",
            "dataset": "products_basic",
            "requests": [
                {"name": "count", "method": "GET", "path": "/products/_count", "expected_status": 200}
            ]
        }"#,
    )
    .expect("manifest should parse");

    let err = run_replay(&manifest, |_| {
        Ok(OracleResponse {
            status: 500,
            body: Some(json!({"error": "boom"})),
        })
    })
    .expect_err("unexpected status should fail");

    assert_eq!(
        err,
        ReplayRunError::StatusMismatch {
            request_name: "count".to_string(),
            expected: 200,
            actual: 500,
        }
    );
}

#[test]
fn run_replay_requires_body_when_expected_response_is_declared() {
    let manifest = ReplayManifest::from_json_str(
        r#"{
            "name": "runner_body",
            "dataset": "products_basic",
            "requests": [
                {
                    "name": "search",
                    "method": "POST",
                    "path": "/products/_search",
                    "expected_status": 200,
                    "expected_response": {"hits": {"total": {"value": 0, "relation": "eq"}}}
                }
            ]
        }"#,
    )
    .expect("manifest should parse");

    let err = run_replay(&manifest, |_| {
        Ok(OracleResponse {
            status: 200,
            body: None,
        })
    })
    .expect_err("missing body should fail");

    assert_eq!(
        err,
        ReplayRunError::MissingResponseBody {
            request_name: "search".to_string(),
        }
    );
}

#[test]
fn run_replay_wraps_executor_errors_with_request_name() {
    let manifest = ReplayManifest::from_json_str(
        r#"{
            "name": "runner_executor",
            "dataset": "products_basic",
            "requests": [
                {"name": "root", "method": "GET", "path": "/", "expected_status": 200}
            ]
        }"#,
    )
    .expect("manifest should parse");

    let err = run_replay(&manifest, |_| -> Result<OracleResponse, String> {
        Err("connection refused".to_string())
    })
    .expect_err("executor failure should fail");

    assert_eq!(
        err,
        ReplayRunError::Executor {
            request_name: "root".to_string(),
            message: "connection refused".to_string(),
        }
    );
}
