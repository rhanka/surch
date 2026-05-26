//! A1/A13 end-to-end: edge_ngram autocomplete sub-field through the real
//! PUT-index -> bulk -> _search path.
//!
//! Validates the full wiring: `settings.analysis` (an `edge_ngram`
//! tokenizer + `autocomplete_analyzer`) is captured on the stored index
//! mapping at create time, the `NOM.autocomplete` sub-field fans out
//! prefix postings at index time, and a `match` on that sub-field is
//! tokenized with its `search_analyzer` (standard) so a short prefix
//! query hits without being itself ngram-expanded.

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use serde_json::Value;
use surch_api::app_router;
use tower::ServiceExt;

const INDEX: &str = "deces2";

const CREATE_BODY: &str = r#"{
  "settings": {
    "analysis": {
      "tokenizer": {
        "edge_ngram_tokenizer": {
          "type": "edge_ngram",
          "min_gram": 2,
          "max_gram": 20,
          "token_chars": ["letter", "digit"]
        }
      },
      "analyzer": {
        "autocomplete_analyzer": {
          "tokenizer": "edge_ngram_tokenizer",
          "filter": ["lowercase", "asciifolding"]
        }
      }
    }
  },
  "mappings": {
    "properties": {
      "NOM": {
        "type": "text",
        "analyzer": "standard",
        "fields": {
          "autocomplete": {
            "type": "text",
            "analyzer": "autocomplete_analyzer",
            "search_analyzer": "standard"
          }
        }
      }
    }
  }
}"#;

const BULK_BODY: &str = "{\"index\":{\"_id\":\"1\",\"_index\":\"deces2\"}}\n{\"NOM\":\"DUPONT\"}\n{\"index\":{\"_id\":\"2\",\"_index\":\"deces2\"}}\n{\"NOM\":\"MARTIN\"}\n";

#[test]
fn edge_ngram_autocomplete_subfield_end_to_end() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let router = app_router();

    let (status, _) = runtime.block_on(execute(
        router.clone(),
        Method::PUT,
        &format!("/{INDEX}"),
        CREATE_BODY.to_string(),
    ));
    assert_eq!(status, StatusCode::OK.as_u16(), "create index OK");

    let (status, body) = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        "/_bulk",
        BULK_BODY.to_string(),
    ));
    assert_eq!(status, StatusCode::OK.as_u16(), "bulk OK: {body:?}");
    assert_eq!(
        body.as_ref().and_then(|v| v.get("errors")),
        Some(&Value::Bool(false)),
        "bulk errors=false"
    );

    let (status, _) = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        &format!("/{INDEX}/_refresh"),
        String::new(),
    ));
    assert_eq!(status, StatusCode::OK.as_u16(), "refresh OK");

    // A 3-char prefix of DUPONT, searched on the autocomplete sub-field.
    // The sub-field indexed edge_ngram prefixes ("du","dup",…); the query
    // is tokenized with `standard` (search_analyzer) -> ["dup"], matching
    // the indexed "dup". DUPONT (id 1) must hit; MARTIN must not.
    let total = autocomplete_hits(&runtime, &router, "dup");
    assert_eq!(total, 1, "match NOM.autocomplete=dup must hit only DUPONT");

    // A prefix matching nothing returns zero hits.
    let total = autocomplete_hits(&runtime, &router, "zzz");
    assert_eq!(total, 0, "match NOM.autocomplete=zzz must hit nothing");
}

fn autocomplete_hits(runtime: &tokio::runtime::Runtime, router: &Router, prefix: &str) -> u64 {
    let body = serde_json::json!({
        "query": { "match": { "NOM.autocomplete": prefix } },
        "size": 5
    })
    .to_string();
    let (status, body) = runtime.block_on(execute(
        router.clone(),
        Method::POST,
        &format!("/{INDEX}/_search"),
        body,
    ));
    assert_eq!(status, StatusCode::OK.as_u16(), "search OK");
    body.expect("search body")
        .pointer("/hits/total/value")
        .and_then(Value::as_u64)
        .expect("hits.total.value present")
}

async fn execute(router: Router, method: Method, path: &str, body: String) -> (u16, Option<Value>) {
    let response = router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body collects");
    let value = if bytes.is_empty() {
        None
    } else {
        Some(serde_json::from_slice::<Value>(&bytes).expect("body is JSON"))
    };
    (status, value)
}
