use opensearch_oracle::replay::{
    HttpMethod, ReplayComparison, ReplayManifest, ReplayManifestError,
};

fn parse_manifest(json: &str) -> ReplayManifest {
    ReplayManifest::from_json_str(json).expect("manifest should parse and validate")
}

#[test]
fn replay_fixture_products_search_manifest_is_valid() {
    let manifest = parse_manifest(include_str!(
        "../../../tests/opensearch_compat/oracle/replays/products_search.json"
    ));

    assert_eq!(manifest.name, "products_search");
    assert_eq!(manifest.dataset, "products_basic");
    assert_eq!(manifest.requests.len(), 4);

    assert_eq!(manifest.requests[0].name, "index_product");
    assert_eq!(manifest.requests[0].method, HttpMethod::Put);
    assert_eq!(manifest.requests[0].path, "/products/_doc/1");
    assert!(manifest.requests[0].body.is_some());
    assert_eq!(manifest.requests[0].expected_status, 201);
    assert!(manifest.requests[0].expected_response.is_some());

    assert_eq!(manifest.requests[2].method, HttpMethod::Post);
    assert_eq!(manifest.requests[2].path, "/products/_search");
    assert_eq!(manifest.requests[3].method, HttpMethod::Get);
    assert_eq!(manifest.requests[3].path, "/products/_count");
}

#[test]
fn replay_fixture_products_bm25_manifest_declares_score_comparison_rules() {
    let manifest = parse_manifest(include_str!(
        "../../../tests/opensearch_compat/oracle/replays/products_bm25_search.json"
    ));

    assert_eq!(manifest.name, "products_bm25_search");
    assert_eq!(manifest.dataset, "products_basic");
    assert_eq!(
        manifest.comparison,
        ReplayComparison {
            ignored_paths: vec!["took".to_string(), "_shards.total".to_string()],
            score_tolerance: 0.001,
        }
    );
    assert_eq!(
        manifest.comparison.to_normalize_config().score_tolerance,
        0.001
    );

    let search = manifest
        .requests
        .iter()
        .find(|request| request.name == "search_products_by_text")
        .expect("BM25 replay should include a search request");
    let expected_response = search
        .expected_response
        .as_ref()
        .expect("BM25 search should pin an expected response");

    assert_eq!(search.method, HttpMethod::Post);
    assert_eq!(search.path, "/products/_search");
    assert_eq!(expected_response["hits"]["max_score"], 0.875_468_73);
    assert_eq!(expected_response["hits"]["hits"][0]["_score"], 0.875_468_73);
}

#[test]
fn replay_manifest_defaults_to_exact_response_comparison() {
    let manifest = parse_manifest(
        r#"{
            "name": "default_comparison",
            "dataset": "products_basic",
            "requests": [
                {"name": "search", "method": "GET", "path": "/idx/_search", "expected_status": 200}
            ]
        }"#,
    );

    assert_eq!(manifest.comparison, ReplayComparison::default());
    assert!(manifest.comparison.ignored_paths.is_empty());
    assert_eq!(manifest.comparison.score_tolerance, 0.0);
}

#[test]
fn replay_manifest_accepts_supported_http_methods() {
    let manifest = parse_manifest(
        r#"{
            "name": "methods",
            "dataset": "products_basic",
            "requests": [
                {"name": "get", "method": "GET", "path": "/idx", "expected_status": 200},
                {"name": "post", "method": "POST", "path": "/idx/_search", "expected_status": 200},
                {"name": "put", "method": "PUT", "path": "/idx/_doc/1", "expected_status": 201},
                {"name": "delete", "method": "DELETE", "path": "/idx/_doc/1", "expected_status": 200}
            ]
        }"#,
    );

    assert_eq!(manifest.requests[0].method, HttpMethod::Get);
    assert_eq!(manifest.requests[1].method, HttpMethod::Post);
    assert_eq!(manifest.requests[2].method, HttpMethod::Put);
    assert_eq!(manifest.requests[3].method, HttpMethod::Delete);
}

#[test]
fn replay_manifest_rejects_invalid_manifests_with_typed_validation_errors() {
    let cases = [
        (
            r#"{"name":"","dataset":"products_basic","requests":[{"name":"search","method":"GET","path":"/products/_search","expected_status":200}]}"#,
            ReplayManifestError::EmptyName,
        ),
        (
            r#"{"name":"products","dataset":"","requests":[{"name":"search","method":"GET","path":"/products/_search","expected_status":200}]}"#,
            ReplayManifestError::EmptyDataset,
        ),
        (
            r#"{"name":"products","dataset":"products_basic","requests":[]}"#,
            ReplayManifestError::EmptyRequests,
        ),
        (
            r#"{"name":"products","dataset":"products_basic","requests":[{"name":"","method":"GET","path":"/products/_search","expected_status":200}]}"#,
            ReplayManifestError::EmptyRequestName { index: 0 },
        ),
        (
            r#"{"name":"products","dataset":"products_basic","requests":[{"name":"search","method":"GET","path":"products/_search","expected_status":200}]}"#,
            ReplayManifestError::InvalidPath { index: 0 },
        ),
        (
            r#"{"name":"products","dataset":"products_basic","requests":[{"name":"search","method":"GET","path":"/products/_search","expected_status":99}]}"#,
            ReplayManifestError::InvalidExpectedStatus {
                index: 0,
                status: 99,
            },
        ),
        (
            r#"{"name":"products","dataset":"products_basic","requests":[{"name":"search","method":"GET","path":"/products/_search","expected_status":200,"expected_response":{}}]}"#,
            ReplayManifestError::EmptyExpectedResponse { index: 0 },
        ),
        (
            r#"{"name":"products","dataset":"products_basic","requests":[{"name":"search","method":"GET","path":"/products/_search","expected_status":200,"expected_response":[]}]}"#,
            ReplayManifestError::EmptyExpectedResponse { index: 0 },
        ),
        (
            r#"{"name":"products","dataset":"products_basic","requests":[{"name":"search","method":"GET","path":"/products/_search","expected_status":200,"expected_response":""}]}"#,
            ReplayManifestError::EmptyExpectedResponse { index: 0 },
        ),
        (
            r#"{"name":"products","dataset":"products_basic","comparison":{"ignored_paths":[" "],"score_tolerance":0.001},"requests":[{"name":"search","method":"GET","path":"/products/_search","expected_status":200}]}"#,
            ReplayManifestError::EmptyIgnoredComparisonPath { index: 0 },
        ),
        (
            r#"{"name":"products","dataset":"products_basic","comparison":{"ignored_paths":[],"score_tolerance":-0.1},"requests":[{"name":"search","method":"GET","path":"/products/_search","expected_status":200}]}"#,
            ReplayManifestError::InvalidScoreTolerance { tolerance: -0.1 },
        ),
    ];

    for (json, expected_error) in cases {
        let actual_error = ReplayManifest::from_json_str(json).expect_err("manifest should fail");
        assert_eq!(actual_error, expected_error);
    }
}
