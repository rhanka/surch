use opensearch_oracle::normalize::{compare_json, normalize_response, NormalizeConfig};
use serde_json::json;

#[test]
fn normalize_removes_ignored_dot_paths_and_wildcard_scores() {
    let value = json!({
        "took": 17,
        "_shards": {
            "total": 3,
            "successful": 3
        },
        "hits": {
            "hits": [
                {"_id": "1", "_score": 1.23, "_source": {"title": "One"}},
                {"_id": "2", "_score": 0.87, "_source": {"title": "Two"}}
            ]
        }
    });
    let config = NormalizeConfig {
        ignored_paths: vec![
            "took".to_string(),
            "_shards.total".to_string(),
            "hits.hits.*._score".to_string(),
        ],
        score_tolerance: 0.001,
    };

    let normalized = normalize_response(&value, &config);

    assert_eq!(
        normalized,
        json!({
            "_shards": {
                "successful": 3
            },
            "hits": {
                "hits": [
                    {"_id": "1", "_source": {"title": "One"}},
                    {"_id": "2", "_source": {"title": "Two"}}
                ]
            }
        })
    );
}

#[test]
fn compare_accepts_fixture_when_took_differs_and_scores_are_within_tolerance() {
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/oracle/normalize/search_expected.json"
    ))
    .expect("expected fixture should parse");
    let actual: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/oracle/normalize/search_actual.json"
    ))
    .expect("actual fixture should parse");
    let config = NormalizeConfig {
        ignored_paths: vec!["took".to_string()],
        score_tolerance: 0.01,
    };

    compare_json(&expected, &actual, &config).expect("fixture responses should compare equal");
}

#[test]
fn compare_bm25_scores_with_tolerance_without_ignoring_them() {
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/oracle/normalize/bm25_expected.json"
    ))
    .expect("expected fixture should parse");
    let actual: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/opensearch_compat/oracle/normalize/bm25_actual.json"
    ))
    .expect("actual fixture should parse");
    let config = NormalizeConfig {
        ignored_paths: vec!["took".to_string()],
        score_tolerance: 0.00001,
    };

    compare_json(&expected, &actual, &config)
        .expect("BM25 score drift within tolerance should compare equal");

    let mut max_score_mismatch = actual.clone();
    max_score_mismatch["hits"]["max_score"] = json!(3.2);
    let err = compare_json(&expected, &max_score_mismatch, &config)
        .expect_err("max_score outside tolerance should fail");
    assert!(err.to_string().contains("hits.max_score"));

    let mut hit_score_mismatch = actual;
    hit_score_mismatch["hits"]["hits"][0]["_score"] = json!(3.2);
    let err = compare_json(&expected, &hit_score_mismatch, &config)
        .expect_err("hit _score outside tolerance should fail");
    assert!(err.to_string().contains("hits.hits.0._score"));
}

#[test]
fn compare_rejects_score_mismatch_outside_tolerance() {
    let expected = json!({"hits": {"hits": [{"_id": "1", "_score": 1.0}]}});
    let actual = json!({"hits": {"hits": [{"_id": "1", "_score": 1.5}]}});
    let config = NormalizeConfig {
        ignored_paths: Vec::new(),
        score_tolerance: 0.01,
    };

    let err = compare_json(&expected, &actual, &config)
        .expect_err("score difference outside tolerance should fail");

    assert!(err.to_string().contains("hits.hits.0._score"));
}

#[test]
fn compare_rejects_missing_path() {
    let expected = json!({"hits": {"total": {"value": 1, "relation": "eq"}}});
    let actual = json!({"hits": {"total": {"value": 1}}});
    let config = NormalizeConfig {
        ignored_paths: Vec::new(),
        score_tolerance: 0.01,
    };

    let err =
        compare_json(&expected, &actual, &config).expect_err("missing expected path should fail");

    assert!(err.to_string().contains("hits.total.relation"));
}
