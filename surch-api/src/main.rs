use axum::{
    extract::State,
    routing::{get, post, put},
    Json, Router,
};
use parking_lot::RwLock;
use std::sync::Arc;
use surch_core::{
    common::{
        BulkItemResponse, BulkItemResult, BulkResponse, Document, FieldValue, IndexMetadata,
        IndexResponse, ShardsInfo,
    },
    search::{
        BoolQuery, Bound, ExistsQuery, FuzzyQuery, MatchOperator, MatchPhraseQuery, MatchQuery,
        MultiMatchQuery, PrefixQuery, Query, QueryType, RangeQuery, ScoredDocument, TermQuery,
        TermValue, TermsQuery, WildcardQuery,
    },
    storage::IndexStore,
};

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
        .route("/:index/_bulk", post(bulk_with_default_index))
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

    let parsed = parse_search_request(&payload)?;
    let mut results: Vec<ScoredDocument> = if let Some(query) = parsed.query {
        query.execute(&docs)
    } else {
        docs.iter()
            .map(|d| ScoredDocument {
                doc: d.clone(),
                score: 1.0,
            })
            .collect()
    };

    sort_results(&mut results, parsed.sort);

    let total = results.len();
    let max_score: f64 = results.iter().map(|r| r.score).fold(0.0f64, f64::max);

    let hits: Vec<serde_json::Value> = results
        .into_iter()
        .skip(parsed.from)
        .take(parsed.size)
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
        "_shards": { "total": 1, "successful": 1, "failed": 0 },
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
    body: String,
) -> Result<Json<BulkResponse>, axum::http::StatusCode> {
    process_bulk_request(state, None, &body)
}

async fn bulk_with_default_index(
    State(state): State<AppState>,
    axum::extract::Path(index): axum::extract::Path<String>,
    body: String,
) -> Result<Json<BulkResponse>, axum::http::StatusCode> {
    process_bulk_request(state, Some(index), &body)
}

fn process_bulk_request(
    state: AppState,
    default_index: Option<String>,
    body: &str,
) -> Result<Json<BulkResponse>, axum::http::StatusCode> {
    if !body.ends_with('\n') {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    let lines: Vec<&str> = body.lines().collect();
    let mut cursor = 0;
    let mut items = Vec::new();
    let mut errors = false;

    while cursor < lines.len() {
        let action_line: serde_json::Value =
            serde_json::from_str(lines[cursor]).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;
        let action_object = action_line
            .as_object()
            .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

        if action_object.len() != 1 {
            return Err(axum::http::StatusCode::BAD_REQUEST);
        }

        let (action_name, action_payload) = action_object.iter().next().expect("one action");
        let payload = action_payload
            .as_object()
            .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

        match action_name.as_str() {
            "index" | "create" => {
                let source_line = lines
                    .get(cursor + 1)
                    .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
                let source: serde_json::Value =
                    serde_json::from_str(source_line).map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;
                let source_obj = source
                    .as_object()
                    .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

                let index = payload
                    .get("_index")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| default_index.clone())
                    .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
                let id = payload
                    .get("_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

                let fields = source_obj
                    .iter()
                    .map(|(key, value)| (key.clone(), json_to_field_value(value)))
                    .collect();

                let store = state.store.read();
                match store.index_document(&index, Document::new(id.clone()).with_fields(fields)) {
                    Ok(_) => {
                        items.push(BulkItemResponse {
                            index: Some(BulkItemResult {
                                index,
                                id,
                                version: 1,
                                result: "created".to_string(),
                                status: 201,
                            }),
                            delete: None,
                        });
                    }
                    Err(_) => {
                        errors = true;
                        items.push(BulkItemResponse {
                            index: Some(BulkItemResult {
                                index,
                                id,
                                version: 0,
                                result: "error".to_string(),
                                status: 404,
                            }),
                            delete: None,
                        });
                    }
                }

                cursor += 2;
            }
            "delete" => {
                let index = payload
                    .get("_index")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .or_else(|| default_index.clone())
                    .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
                let id = payload
                    .get("_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

                let store = state.store.read();
                let deleted = store
                    .delete_document(&index, &id)
                    .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;

                items.push(BulkItemResponse {
                    index: None,
                    delete: Some(BulkItemResult {
                        index,
                        id,
                        version: 1,
                        result: if deleted {
                            "deleted".to_string()
                        } else {
                            "not_found".to_string()
                        },
                        status: 200,
                    }),
                });

                cursor += 1;
            }
            _ => return Err(axum::http::StatusCode::BAD_REQUEST),
        }
    }

    Ok(Json(BulkResponse { took: 0, errors, items }))
}

struct ParsedSearchRequest {
    query: Option<QueryType>,
    from: usize,
    size: usize,
    sort: Vec<SortSpec>,
}

enum SortSpec {
    ScoreDesc,
    ScoreAsc,
    Field { name: String, asc: bool },
}

fn parse_search_request(payload: &serde_json::Value) -> Result<ParsedSearchRequest, axum::http::StatusCode> {
    let object = payload
        .as_object()
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

    for key in object.keys() {
        if !matches!(key.as_str(), "query" | "from" | "size" | "sort" | "track_total_hits") {
            return Err(axum::http::StatusCode::BAD_REQUEST);
        }
    }

    let from = payload
        .get("from")
        .map(parse_usize)
        .transpose()?
        .unwrap_or(0);
    let size = payload
        .get("size")
        .map(parse_usize)
        .transpose()?
        .unwrap_or(10);

    if let Some(track_total_hits) = payload.get("track_total_hits") {
        if !track_total_hits.is_boolean() {
            return Err(axum::http::StatusCode::BAD_REQUEST);
        }
    }

    let sort = payload
        .get("sort")
        .map(parse_sort_specs)
        .transpose()?
        .unwrap_or_else(|| vec![SortSpec::ScoreDesc]);

    let query = payload.get("query").map(parse_query_type).transpose()?;

    Ok(ParsedSearchRequest {
        query,
        from,
        size,
        sort,
    })
}

fn parse_query_type(value: &serde_json::Value) -> Result<QueryType, axum::http::StatusCode> {
    let object = value
        .as_object()
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

    if object.len() != 1 {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    let (query_type, query_body) = object.iter().next().expect("one query entry");
    match query_type.as_str() {
        "match" => parse_match_query(query_body).map(QueryType::Match),
        "match_phrase" => parse_match_phrase_query(query_body).map(QueryType::MatchPhrase),
        "multi_match" => parse_multi_match_query(query_body).map(QueryType::MultiMatch),
        "term" => parse_term_query(query_body).map(QueryType::Term),
        "terms" => parse_terms_query(query_body).map(QueryType::Terms),
        "range" => parse_range_query(query_body).map(QueryType::Range),
        "exists" => parse_exists_query(query_body).map(QueryType::Exists),
        "bool" => parse_bool_query(query_body).map(QueryType::Bool),
        "prefix" => parse_prefix_query(query_body).map(QueryType::Prefix),
        "wildcard" => parse_wildcard_query(query_body).map(QueryType::Wildcard),
        "fuzzy" => parse_fuzzy_query(query_body).map(QueryType::Fuzzy),
        "regexp" => Err(axum::http::StatusCode::BAD_REQUEST),
        _ => Err(axum::http::StatusCode::BAD_REQUEST),
    }
}

fn parse_match_query(value: &serde_json::Value) -> Result<MatchQuery, axum::http::StatusCode> {
    let (field, inner) = single_field_clause(value)?;
    if let Some(query) = inner.as_str() {
        return Ok(MatchQuery::new(field, query));
    }

    let object = inner.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let query = object
        .get("query")
        .and_then(serde_json::Value::as_str)
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let operator = match object.get("operator").and_then(serde_json::Value::as_str) {
        Some("and") => MatchOperator::And,
        Some("or") | None => MatchOperator::Or,
        _ => return Err(axum::http::StatusCode::BAD_REQUEST),
    };

    let mut match_query = MatchQuery::new(field, query).with_operator(operator);
    if let Some(fuzziness) = object.get("fuzziness") {
        match_query = match_query.with_fuzziness(parse_fuzziness(fuzziness, query)?);
    }
    Ok(match_query)
}

fn parse_match_phrase_query(
    value: &serde_json::Value,
) -> Result<MatchPhraseQuery, axum::http::StatusCode> {
    let (field, inner) = single_field_clause(value)?;
    if let Some(query) = inner.as_str() {
        return Ok(MatchPhraseQuery::new(field, query));
    }

    let object = inner.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let query = object
        .get("query")
        .and_then(serde_json::Value::as_str)
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let slop = object.get("slop").map(parse_usize).transpose()?.unwrap_or(0);

    Ok(MatchPhraseQuery {
        field,
        query: query.to_string(),
        slop,
    })
}

fn parse_multi_match_query(
    value: &serde_json::Value,
) -> Result<MultiMatchQuery, axum::http::StatusCode> {
    let object = value.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let query = object
        .get("query")
        .and_then(serde_json::Value::as_str)
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let fields = object
        .get("fields")
        .and_then(serde_json::Value::as_array)
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?
        .iter()
        .map(|field| {
            field
                .as_str()
                .map(str::to_string)
                .ok_or(axum::http::StatusCode::BAD_REQUEST)
        })
        .collect::<Result<Vec<_>, _>>()?;

    if let Some(query_type) = object.get("type") {
        if query_type.as_str() != Some("best_fields") {
            return Err(axum::http::StatusCode::BAD_REQUEST);
        }
    }

    let mut multi_match = MultiMatchQuery::new(query, fields);
    if let Some(fuzziness) = object.get("fuzziness") {
        multi_match.fuzziness = Some(parse_fuzziness(fuzziness, query)?);
    }
    Ok(multi_match)
}

fn parse_term_query(value: &serde_json::Value) -> Result<TermQuery, axum::http::StatusCode> {
    let (field, inner) = single_field_clause(value)?;
    let term_value = if inner.is_object() {
        let object = inner.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
        let value = object
            .get("value")
            .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
        parse_term_value(value)?
    } else {
        parse_term_value(inner)?
    };

    Ok(TermQuery::new(field, term_value))
}

fn parse_terms_query(value: &serde_json::Value) -> Result<TermsQuery, axum::http::StatusCode> {
    let (field, inner) = single_field_clause(value)?;
    let values = inner.as_array().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    if values.is_empty() {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    Ok(TermsQuery {
        field,
        values: values
            .iter()
            .map(parse_term_value)
            .collect::<Result<Vec<_>, _>>()?,
        boost: 1.0,
    })
}

fn parse_range_query(value: &serde_json::Value) -> Result<RangeQuery, axum::http::StatusCode> {
    let (field, inner) = single_field_clause(value)?;
    let object = inner.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let mut range = RangeQuery::new(field);
    let mut has_bound = false;

    if let Some(value) = object.get("gte") {
        range = range.gte(parse_bound(value)?);
        has_bound = true;
    }
    if let Some(value) = object.get("gt") {
        range = range.gt(parse_bound(value)?);
        has_bound = true;
    }
    if let Some(value) = object.get("lte") {
        range = range.lte(parse_bound(value)?);
        has_bound = true;
    }
    if let Some(value) = object.get("lt") {
        range = range.lt(parse_bound(value)?);
        has_bound = true;
    }

    if !has_bound {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    Ok(range)
}

fn parse_exists_query(value: &serde_json::Value) -> Result<ExistsQuery, axum::http::StatusCode> {
    let field = value
        .get("field")
        .and_then(serde_json::Value::as_str)
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    Ok(ExistsQuery::new(field))
}

fn parse_bool_query(value: &serde_json::Value) -> Result<BoolQuery, axum::http::StatusCode> {
    let object = value.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let mut query = BoolQuery::new();

    if let Some(must) = object.get("must") {
        query.must = parse_query_array(must)?;
    }
    if let Some(filter) = object.get("filter") {
        query.filter = parse_query_array(filter)?;
    }
    if let Some(should) = object.get("should") {
        query.should = parse_query_array(should)?;
    }
    if let Some(must_not) = object.get("must_not") {
        query.must_not = parse_query_array(must_not)?;
    }
    if let Some(msm) = object.get("minimum_should_match") {
        query.minimum_should_match = parse_usize(msm)?;
    }

    if query.must.is_empty() && query.filter.is_empty() && query.should.is_empty() && query.must_not.is_empty() {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }

    Ok(query)
}

fn parse_prefix_query(value: &serde_json::Value) -> Result<PrefixQuery, axum::http::StatusCode> {
    let (field, inner) = single_field_clause(value)?;
    let prefix = if let Some(value) = inner.as_str() {
        value.to_string()
    } else {
        inner
            .get("value")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or(axum::http::StatusCode::BAD_REQUEST)?
    };
    Ok(PrefixQuery::new(field, prefix))
}

fn parse_wildcard_query(value: &serde_json::Value) -> Result<WildcardQuery, axum::http::StatusCode> {
    let (field, inner) = single_field_clause(value)?;
    let wildcard = if let Some(value) = inner.as_str() {
        value.to_string()
    } else {
        inner
            .get("value")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or(axum::http::StatusCode::BAD_REQUEST)?
    };
    Ok(WildcardQuery::new(field, wildcard))
}

fn parse_fuzzy_query(value: &serde_json::Value) -> Result<FuzzyQuery, axum::http::StatusCode> {
    let (field, inner) = single_field_clause(value)?;
    let object = inner.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    let query = object
        .get("value")
        .and_then(serde_json::Value::as_str)
        .ok_or(axum::http::StatusCode::BAD_REQUEST)?;

    let mut fuzzy = FuzzyQuery::new(field, query);
    if let Some(fuzziness) = object.get("fuzziness") {
        fuzzy.fuzziness = parse_fuzziness(fuzziness, query)?;
    }
    if let Some(prefix_length) = object.get("prefix_length") {
        fuzzy.prefix_length = parse_usize(prefix_length)?;
    }
    if let Some(transpositions) = object.get("transpositions") {
        fuzzy.transpositions = transpositions
            .as_bool()
            .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    }
    Ok(fuzzy)
}

fn parse_query_array(value: &serde_json::Value) -> Result<Vec<QueryType>, axum::http::StatusCode> {
    let items = value.as_array().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    items.iter().map(parse_query_type).collect()
}

fn single_field_clause(value: &serde_json::Value) -> Result<(String, &serde_json::Value), axum::http::StatusCode> {
    let object = value.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    if object.len() != 1 {
        return Err(axum::http::StatusCode::BAD_REQUEST);
    }
    let (field, inner) = object.iter().next().expect("one field");
    Ok((field.clone(), inner))
}

fn parse_term_value(value: &serde_json::Value) -> Result<TermValue, axum::http::StatusCode> {
    match value {
        serde_json::Value::String(text) => Ok(TermValue::Text(text.clone())),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                if let Ok(value) = i32::try_from(integer) {
                    Ok(TermValue::Integer(value))
                } else {
                    Ok(TermValue::Long(integer))
                }
            } else {
                number
                    .as_f64()
                    .map(TermValue::Double)
                    .ok_or(axum::http::StatusCode::BAD_REQUEST)
            }
        }
        serde_json::Value::Bool(value) => Ok(TermValue::Bool(*value)),
        _ => Err(axum::http::StatusCode::BAD_REQUEST),
    }
}

fn parse_bound(value: &serde_json::Value) -> Result<Bound, axum::http::StatusCode> {
    match value {
        serde_json::Value::String(text) => Ok(Bound::String(text.clone())),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                if let Ok(value) = i32::try_from(integer) {
                    Ok(Bound::Integer(value))
                } else {
                    Ok(Bound::Long(integer))
                }
            } else {
                number
                    .as_f64()
                    .map(Bound::Double)
                    .ok_or(axum::http::StatusCode::BAD_REQUEST)
            }
        }
        _ => Err(axum::http::StatusCode::BAD_REQUEST),
    }
}

fn parse_fuzziness(value: &serde_json::Value, query: &str) -> Result<usize, axum::http::StatusCode> {
    match value {
        serde_json::Value::String(text) if text == "AUTO" => Ok(auto_fuzziness(query)),
        serde_json::Value::Number(number) => number
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value <= 2)
            .ok_or(axum::http::StatusCode::BAD_REQUEST),
        _ => Err(axum::http::StatusCode::BAD_REQUEST),
    }
}

fn auto_fuzziness(query: &str) -> usize {
    match query.chars().count() {
        0..=2 => 0,
        3..=5 => 1,
        _ => 2,
    }
}

fn parse_sort_specs(value: &serde_json::Value) -> Result<Vec<SortSpec>, axum::http::StatusCode> {
    let items = value.as_array().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
    items.iter()
        .map(|item| {
            if let Some(text) = item.as_str() {
                return Ok(if text == "_score" {
                    SortSpec::ScoreDesc
                } else {
                    SortSpec::Field {
                        name: text.to_string(),
                        asc: true,
                    }
                });
            }

            let object = item.as_object().ok_or(axum::http::StatusCode::BAD_REQUEST)?;
            if object.len() != 1 {
                return Err(axum::http::StatusCode::BAD_REQUEST);
            }
            let (field, value) = object.iter().next().expect("one sort entry");
            let direction = value
                .as_str()
                .or_else(|| value.get("order").and_then(serde_json::Value::as_str))
                .ok_or(axum::http::StatusCode::BAD_REQUEST)?;
            let asc = match direction {
                "asc" => true,
                "desc" => false,
                _ => return Err(axum::http::StatusCode::BAD_REQUEST),
            };

            Ok(if field == "_score" {
                if asc {
                    SortSpec::ScoreAsc
                } else {
                    SortSpec::ScoreDesc
                }
            } else {
                SortSpec::Field {
                    name: field.to_string(),
                    asc,
                }
            })
        })
        .collect()
}

fn parse_usize(value: &serde_json::Value) -> Result<usize, axum::http::StatusCode> {
    value.as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(axum::http::StatusCode::BAD_REQUEST)
}

fn sort_results(results: &mut [ScoredDocument], sort: Vec<SortSpec>) {
    for sort_spec in sort.into_iter().rev() {
        match sort_spec {
            SortSpec::ScoreDesc => {
                results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
            }
            SortSpec::ScoreAsc => {
                results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
            }
            SortSpec::Field { name, asc } => {
                results.sort_by(|a, b| {
                    let a_value = a.doc.get_field(&name).map(field_value_sort_key).unwrap_or_default();
                    let b_value = b.doc.get_field(&name).map(field_value_sort_key).unwrap_or_default();
                    if asc {
                        a_value.cmp(&b_value)
                    } else {
                        b_value.cmp(&a_value)
                    }
                });
            }
        }
    }
}

fn field_value_sort_key(value: &FieldValue) -> String {
    match value {
        FieldValue::Null => String::new(),
        FieldValue::Bool(value) => value.to_string(),
        FieldValue::Integer(value) => value.to_string(),
        FieldValue::Long(value) => value.to_string(),
        FieldValue::Float(value) => value.to_string(),
        FieldValue::Double(value) => value.to_string(),
        FieldValue::Text(value) => value.clone(),
        FieldValue::Keyword(value) => value.clone(),
        FieldValue::Date(value) => value.clone(),
        FieldValue::Array(values) => values.first().map(field_value_sort_key).unwrap_or_default(),
    }
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

    #[tokio::test]
    async fn bulk_endpoint_indexes_document_from_ndjson() {
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
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_bulk")
                    .header("content-type", "application/x-ndjson")
                    .body(Body::from("{\"index\":{\"_index\":\"books\",\"_id\":\"doc-1\"}}\n{\"title\":\"Hello\"}\n"))
                    .expect("request"),
            )
            .await
            .expect("bulk response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json.get("errors").and_then(Value::as_bool), Some(false));
        assert_eq!(json["items"].as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn bulk_endpoint_rejects_malformed_ndjson() {
        let app = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_bulk")
                    .header("content-type", "application/x-ndjson")
                    .body(Body::from("{\"index\":{\"_index\":\"books\"}}"))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn refresh_and_flush_return_shard_summary() {
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

        for path in ["/books/_refresh", "/books/_flush"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");

            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
            let json: Value = serde_json::from_slice(&body).expect("json body");
            assert!(json.get("_shards").is_some());
        }
    }

    #[tokio::test]
    async fn search_term_query_returns_matching_hit() {
        let app = test_app();

        seed_search_docs(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":{"term":{"status":"published"}}}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json["hits"]["total"]["value"].as_u64(), Some(2));
        assert_eq!(json["hits"]["hits"].as_array().map(Vec::len), Some(2));
    }

    #[tokio::test]
    async fn search_match_phrase_respects_slop_zero() {
        let app = test_app();

        seed_search_docs(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"query":{"match_phrase":{"title":{"query":"search engine","slop":0}}}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        let hits = json["hits"]["hits"].as_array().expect("hits array");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["_id"].as_str(), Some("2"));
    }

    #[tokio::test]
    async fn search_fuzzy_query_matches_transposition() {
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
                    .uri("/books/_doc/1")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title":"ba"}"#))
                    .expect("request"),
            )
            .await
            .expect("index response");

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"query":{"fuzzy":{"title":{"value":"ab","fuzziness":1}}}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        let hits = json["hits"]["hits"].as_array().expect("hits array");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["_id"].as_str(), Some("1"));
    }

    #[tokio::test]
    async fn search_applies_from_size_and_sort() {
        let app = test_app();

        seed_search_docs(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"from":1,"size":1,"sort":[{"title":"asc"}]}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        let hits = json["hits"]["hits"].as_array().expect("hits array");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["_id"].as_str(), Some("2"));
    }

    #[tokio::test]
    async fn search_rejects_regexp_query_for_mvp() {
        let app = test_app();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":{"regexp":{"title":{"value":"s.*"}}}}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn search_bool_query_filters_hits() {
        let app = test_app();

        seed_search_docs(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"query":{"bool":{"must":[{"term":{"status":"published"}}],"filter":[{"range":{"year":{"lte":2023}}}]}}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        let hits = json["hits"]["hits"].as_array().expect("hits array");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["_id"].as_str(), Some("2"));
    }

    #[tokio::test]
    async fn search_prefix_query_matches_prefix() {
        let app = test_app();

        seed_search_docs(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":{"prefix":{"title":"sea"}}}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        let hits = json["hits"]["hits"].as_array().expect("hits array");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["_id"].as_str(), Some("2"));
    }

    #[tokio::test]
    async fn search_wildcard_query_matches_pattern() {
        let app = test_app();

        seed_search_docs(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"query":{"wildcard":{"title":"search*"}}}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        let hits = json["hits"]["hits"].as_array().expect("hits array");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["_id"].as_str(), Some("2"));
    }

    #[tokio::test]
    async fn search_multi_match_query_matches_any_listed_field() {
        let app = test_app();

        seed_search_docs(&app).await;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/books/_search")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"query":{"multi_match":{"query":"manual","fields":["title","body"],"type":"best_fields"}}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        let body = to_bytes(response.into_body(), usize::MAX).await.expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json body");
        let hits = json["hits"]["hits"].as_array().expect("hits array");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["_id"].as_str(), Some("3"));
    }

    async fn seed_search_docs(app: &Router) {
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

        for (id, title, status, year) in [
            ("1", "rust search", "published", 2024),
            ("2", "search engine", "published", 2023),
            ("3", "zebra manual", "draft", 2025),
        ] {
            let body = serde_json::json!({
                "title": title,
                "body": format!("body {title}"),
                "status": status,
                "year": year,
            });

            let _ = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri(format!("/books/_doc/{id}"))
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .expect("request"),
                )
                .await
                .expect("index response");
        }
    }
}
