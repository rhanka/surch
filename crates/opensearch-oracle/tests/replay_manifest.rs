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
fn replay_fixture_ban_tiny_search_manifest_is_valid_for_oracle_replay() {
    let manifest_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/opensearch_compat/oracle/replays/ban_tiny_search.json");
    let manifest_json =
        std::fs::read_to_string(manifest_path).expect("BAN tiny replay fixture should exist");
    let manifest = parse_manifest(&manifest_json);

    assert_eq!(manifest.name, "ban_tiny_search");
    assert_eq!(manifest.dataset, "ban_tiny");
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

    assert_eq!(manifest.requests.len(), 4);

    let count = &manifest.requests[0];
    assert_eq!(count.name, "count_ban_tiny_addresses");
    assert_eq!(count.method, HttpMethod::Get);
    assert_eq!(count.path, "/ban_tiny/_count");
    assert_eq!(count.expected_status, 200);
    assert_eq!(count.expected_response.as_ref().unwrap()["count"], 3);

    let match_label = &manifest.requests[1];
    assert_eq!(match_label.name, "search_ban_tiny_by_label");
    assert_eq!(match_label.method, HttpMethod::Post);
    assert_eq!(match_label.path, "/ban_tiny/_search");
    assert_eq!(match_label.expected_status, 200);
    assert_eq!(
        match_label.body.as_ref().unwrap()["query"]["match"]["label"],
        "Rue de Rivoli"
    );
    assert_eq!(
        match_label.expected_response.as_ref().unwrap()["hits"]["total"]["value"],
        1
    );

    let match_address = &manifest.requests[2];
    assert_eq!(match_address.name, "search_ban_tiny_by_address_fields");
    assert_eq!(match_address.method, HttpMethod::Post);
    assert_eq!(match_address.path, "/ban_tiny/_search");
    assert_eq!(match_address.expected_status, 200);
    assert_eq!(
        match_address.body.as_ref().unwrap()["query"]["bool"]["must"][0]["match"]["street_name"],
        "Cours de l'Intendance"
    );
    assert_eq!(
        match_address.body.as_ref().unwrap()["query"]["bool"]["must"][1]["match"]["postcode"],
        "33000"
    );

    let fuzzy = &manifest.requests[3];
    assert_eq!(fuzzy.name, "future_fuzzy_label_typo");
    assert_eq!(fuzzy.method, HttpMethod::Post);
    assert_eq!(fuzzy.path, "/ban_tiny/_search");
    assert_eq!(fuzzy.expected_status, 200);
    assert_eq!(
        fuzzy.body.as_ref().unwrap()["query"]["fuzzy"]["label"]["value"],
        "Ale des Erables"
    );
    assert_eq!(
        fuzzy.body.as_ref().unwrap()["query"]["fuzzy"]["label"]["fuzziness"],
        2
    );
    assert_eq!(
        fuzzy.expected_response.as_ref().unwrap()["hits"]["total"]["value"],
        1
    );
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
