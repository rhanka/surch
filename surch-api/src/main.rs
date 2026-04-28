use axum::{
    extract::State,
    routing::{delete, get, post, put},
    Json, Router,
};
use parking_lot::RwLock;
use std::sync::Arc;
use surch_core::{
    common::{
        BulkRequest, BulkResponse, Document, FieldValue, IndexMetadata, IndexRequest,
        IndexResponse, ShardsInfo,
    },
    search::{MatchQuery, Query, ScoredDocument},
    storage::IndexStore,
};
use tokio::sync::oneshot;
use tracing_subscriber;

mod routes;

#[derive(Clone)]
struct AppState {
    store: Arc<RwLock<IndexStore>>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let store = IndexStore::new("./data").expect("Failed to create index store");
    let state = AppState {
        store: Arc::new(RwLock::new(store)),
    };

    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9200").await.unwrap();

    tracing::info!("Surch server started on http://0.0.0.0:9200");

    axum::serve(listener, app).await.unwrap();
}

async fn root() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "surch",
        "cluster_name": "surch-cluster",
        "cluster_uuid": "surch-uuid",
        "version": {
            "number": "0.1.0",
            "build_flavor": "default",
            "build_type": "tar",
            "build_hash": "surch",
            "build_date": "2024-01-01T00:00:00.000000Z",
            "build_snapshot": false,
            "lucene_version": "9.8.0",
            "minimum_wire_compatibility_version": "7.10.0",
            "minimum_index_compatibility_version": "7.0.0"
        },
        "tagline": "You know, for search"
    }))
}

async fn cluster_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "cluster_name": "surch-cluster",
        "status": "green",
        "timed_out": false,
        "number_of_nodes": 1,
        "number_of_data_nodes": 1,
        "active_primary_shards": 1,
        "active_shards": 1,
        "relocating_shards": 0,
        "initializing_shards": 0,
        "unassigned_shards": 0
    }))
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/_cluster/health", get(cluster_health))
        .route("/_cat/indices", get(list_all_indexes))
        .route(
            "/:index",
            put(create_index).delete(delete_index).get(get_index),
        )
        .route("/:index/_mapping", get(get_mapping))
        .route(
            "/:index/_doc/:id",
            put(index_document)
                .get(get_document)
                .delete(delete_document),
        )
        .route("/:index/_search", post(search))
        .route("/:index/_refresh", post(refresh_index))
        .route("/:index/_flush", post(flush_index))
        .route("/_bulk", post(bulk))
        .with_state(state)
}

async fn list_all_indexes(State(state): State<AppState>) -> Json<serde_json::Value> {
    let store = state.store.read();
    let indexes = store.list_indexes();

    let mut result = serde_json::Map::new();
    for idx in indexes {
        let idx_name = idx.name.clone();
        result.insert(
            idx_name.clone(),
            serde_json::json!({
                "health": "green",
                "status": "open",
                "index": idx_name,
                "uuid": idx.uuid,
                "pri": 1,
                "rep": 0,
                "docs.count": 0,
                "docs.deleted": 0,
                "store.size": "1kb"
            }),
        );
    }

    Json(serde_json::Value::Object(result))
}

async fn create_index(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let mut metadata = IndexMetadata::new(index.clone());

    if let Some(mappings) = payload.get("mappings") {
        if let Some(props) = mappings.get("properties") {
            if let Some(obj) = props.as_object() {
                for (name, def) in obj {
                    let field_type = match def.get("type").and_then(|t| t.as_str()) {
                        Some("text") => surch_core::common::FieldType::Text,
                        Some("keyword") => surch_core::common::FieldType::Keyword,
                        Some("integer") => surch_core::common::FieldType::Integer,
                        Some("long") => surch_core::common::FieldType::Long,
                        Some("float") => surch_core::common::FieldType::Float,
                        Some("double") => surch_core::common::FieldType::Double,
                        Some("boolean") => surch_core::common::FieldType::Boolean,
                        Some("date") => surch_core::common::FieldType::Date,
                        _ => surch_core::common::FieldType::Text,
                    };
                    metadata
                        .mapping
                        .add_field(surch_core::common::FieldDefinition::new(name, field_type));
                }
            }
        }
    }

    let store = state.store.read();
    store
        .create_index(metadata)
        .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    Ok(Json(serde_json::json!({
        "acknowledged": true,
        "shards_acknowledged": true,
        "index": index
    })))
}

async fn delete_index(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let store = state.store.read();
    store
        .delete_index(&index)
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        "acknowledged": true
    })))
}

async fn get_index(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let store = state.store.read();
    let metadata = store
        .get_index(&index)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    let mut index_info = serde_json::Map::new();
    let mut settings = serde_json::Map::new();
    settings.insert("index.number_of_shards".to_string(), serde_json::json!(1));
    settings.insert("index.number_of_replicas".to_string(), serde_json::json!(0));

    index_info.insert(
        "aliases".to_string(),
        serde_json::Value::Object(serde_json::Map::new()),
    );
    index_info.insert("settings".to_string(), serde_json::Value::Object(settings));
    index_info.insert(
        "mappings".to_string(),
        serde_json::to_value(&metadata.mapping).unwrap_or(serde_json::Value::Null),
    );

    Ok(Json(serde_json::json!({
        index: index_info
    })))
}

async fn get_mapping(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let store = state.store.read();
    let metadata = store
        .get_index(&index)
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        index: {
            "mappings": metadata.mapping
        }
    })))
}

async fn index_document(
    State(state): State<AppState>,
    axum::extract::Path((index, id)): axum::extract::Path<(String, String)>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<IndexResponse>, axum::http::StatusCode> {
    let doc_id = id.clone();

    let fields: std::collections::HashMap<String, FieldValue> = payload
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), json_to_field_value(v)))
                .collect()
        })
        .unwrap_or_default();

    let doc = Document::new(id).with_fields(fields);

    let store = state.store.read();
    let indexed_id = store
        .index_document(&index, doc)
        .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    Ok(Json(IndexResponse {
        _index: index,
        _id: doc_id,
        version: 1,
        _seq_no: indexed_id,
        _primary_term: 1,
        result: "created".to_string(),
        shards: ShardsInfo::default(),
    }))
}

async fn get_document(
    State(state): State<AppState>,
    axum::extract::Path((index, id)): axum::extract::Path<(String, String)>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let store = state.store.read();
    let doc = store
        .get_document(&index, &id)
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)?;

    match doc {
        Some(d) => Ok(Json(serde_json::json!({
            "_index": index,
            "_id": id,
            "_version": 1,
            "_seq_no": 0,
            "_primary_term": 1,
            "found": true,
            "_source": d.fields
        }))),
        None => Ok(Json(serde_json::json!({
            "_index": index,
            "_id": id,
            "found": false
        }))),
    }
}

async fn delete_document(
    State(state): State<AppState>,
    axum::extract::Path((index, id)): axum::extract::Path<(String, String)>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let store = state.store.read();
    let deleted = store
        .delete_document(&index, &id)
        .map_err(|_| axum::http::StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        "_index": index,
        "_id": id,
        "result": if deleted { "deleted" } else { "not_found" },
        "_version": if deleted { 2 } else { 1 }
    })))
}

async fn search(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let store = state.store.read();
    let docs = store.get_all_documents(&index).unwrap_or_default();

    let results: Vec<surch_core::search::ScoredDocument> = if let Some(query) = payload.get("query")
    {
        let q = surch_core::search::MatchQuery::new("_all", query.as_str().unwrap_or("*"));
        q.execute(&docs)
    } else {
        docs.iter()
            .map(|d| surch_core::search::ScoredDocument {
                doc: d.clone(),
                score: 1.0,
            })
            .collect()
    };

    let total = results.len();
    let max_score: f64 = results
        .iter()
        .map(|r| r.score)
        .fold(0.0f64, |a: f64, b| a.max(b));

    let hits: Vec<serde_json::Value> = results
        .into_iter()
        .take(10)
        .map(|r| {
            serde_json::json!({
                "_index": index,
                "_id": r.doc.id,
                "_score": r.score,
                "_source": r.doc.fields
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "took": 0,
        "timed_out": false,
        "shards": { "total": 1, "successful": 1, "failed": 0 },
        "hits": {
            "total": { "value": total, "relation": "eq" },
            "max_score": max_score,
            "hits": hits
        }
    })))
}

async fn refresh_index(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let store = state.store.read();
    store
        .refresh(&index)
        .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    Ok(Json(serde_json::json!({
        "_shards": { "total": 1, "successful": 1, "failed": 0 }
    })))
}

async fn flush_index(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let store = state.store.read();
    store
        .flush(&index)
        .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

    Ok(Json(serde_json::json!({
        "_shards": { "total": 1, "successful": 1, "failed": 0 }
    })))
}

async fn bulk(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Json<BulkResponse> {
    Json(BulkResponse {
        took: 0,
        errors: false,
        items: vec![],
    })
}

fn json_to_field_value(v: &serde_json::Value) -> FieldValue {
    match v {
        serde_json::Value::Null => FieldValue::Null,
        serde_json::Value::Bool(b) => FieldValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                FieldValue::Long(i)
            } else if let Some(f) = n.as_f64() {
                FieldValue::Double(f)
            } else {
                FieldValue::Null
            }
        }
        serde_json::Value::String(s) => FieldValue::Text(s.clone()),
        serde_json::Value::Array(arr) => {
            FieldValue::Array(arr.iter().map(json_to_field_value).collect())
        }
        _ => FieldValue::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::{build_app, AppState};
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::Router;
    use serde_json::Value;
    use std::sync::Arc;
    use surch_core::storage::IndexStore;
    use tower::ServiceExt;

    fn test_app() -> Router {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let store = IndexStore::new(temp_dir.path()).expect("create store");
        let state = AppState {
            store: Arc::new(parking_lot::RwLock::new(store)),
        };
        build_app(state)
    }

    #[tokio::test]
    async fn create_index_returns_acknowledged_shape() {
        let app = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mappings":{"properties":{"title":{"type":"text"}}}}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(json.get("acknowledged").and_then(Value::as_bool), Some(true));
        assert_eq!(
            json.get("shards_acknowledged").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(json.get("index").and_then(Value::as_str), Some("books"));
    }

    #[tokio::test]
    async fn delete_index_returns_acknowledged_shape() {
        let app = test_app();

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("create response");

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/books")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(json.get("acknowledged").and_then(Value::as_bool), Some(true));
    }

    #[tokio::test]
    async fn get_mapping_returns_index_keyed_mappings() {
        let app = test_app();

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mappings":{"properties":{"title":{"type":"text"}}}}"#))
                    .expect("request"),
            )
            .await
            .expect("create response");

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/books/_mapping")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");

        assert!(json.get("books").is_some());
        assert!(json["books"].get("mappings").is_some());
    }

    #[tokio::test]
    async fn get_index_returns_index_keyed_settings_and_mappings() {
        let app = test_app();

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mappings":{"properties":{"title":{"type":"text"}}}}"#))
                    .expect("request"),
            )
            .await
            .expect("create response");

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/books")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");

        assert!(json.get("books").is_some());
        assert!(json["books"].get("settings").is_some());
        assert!(json["books"].get("mappings").is_some());
    }

    #[tokio::test]
    async fn index_document_returns_expected_metadata() {
        let app = test_app();

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("create response");

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books/_doc/doc-1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"Hello"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(json.get("_index").and_then(Value::as_str), Some("books"));
        assert_eq!(json.get("_id").and_then(Value::as_str), Some("doc-1"));
        assert_eq!(json.get("result").and_then(Value::as_str), Some("created"));
        assert!(json.get("_seq_no").is_some());
        assert!(json.get("_primary_term").is_some());
        assert!(json.get("_shards").is_some());
    }

    #[tokio::test]
    async fn get_document_returns_found_false_for_missing_doc() {
        let app = test_app();

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("create response");

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/books/_doc/missing")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(json.get("found").and_then(Value::as_bool), Some(false));
    }

    #[tokio::test]
    async fn delete_document_returns_not_found_for_missing_doc() {
        let app = test_app();

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("create response");

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/books/_doc/missing")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(json.get("result").and_then(Value::as_str), Some("not_found"));
    }

    #[tokio::test]
    async fn delete_document_removes_existing_doc() {
        let app = test_app();

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("create response");

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/books/_doc/doc-1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"Hello"}"#))
                    .expect("request"),
            )
            .await
            .expect("index response");

        let delete_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/books/_doc/doc-1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("delete response");

        assert_eq!(delete_response.status(), StatusCode::OK);

        let get_response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/books/_doc/doc-1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("get response");

        let body = to_bytes(get_response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json.get("found").and_then(Value::as_bool), Some(false));
    }
}
