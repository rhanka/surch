use opensearch_oracle::replay::{HttpMethod, ReplayManifest, ReplayManifestError};

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
    ];

    for (json, expected_error) in cases {
        let actual_error = ReplayManifest::from_json_str(json).expect_err("manifest should fail");
        assert_eq!(actual_error, expected_error);
    }
}
