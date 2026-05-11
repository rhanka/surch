use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::Instant;
use surch_search::fuzzy::{
    bounded_damerau_levenshtein, edits_for_term_len, parse_fuzziness, Fuzziness,
};
use surch_search::scoring::{bm25_score, Bm25Config};

use crate::{
    index::validate_index_name,
    state::{AppState, StoredDocument},
    OpenSearchError,
};

/// OpenSearch-compatible `_search` request body for the P0 bootstrap surface.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchRequest {
    pub query: Option<SearchQuery>,
    pub from: Option<u64>,
    pub size: Option<u64>,
    pub source: Option<SourceFilter>,
    pub track_total_hits: Option<TrackTotalHits>,
    pub sort: Vec<SortClause>,
}

/// Single OpenSearch `sort` clause.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortClause {
    pub field: String,
    pub order: SortOrder,
}

/// Direction component of a `sort` clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// Boolean combination of analyzed query tokens for `match`/`multi_match`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MatchOperator {
    #[default]
    Or,
    And,
}

/// OpenSearch-compatible `track_total_hits` request mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackTotalHits {
    Disabled,
    Exact,
    UpTo(u64),
}

/// OpenSearch-compatible `_source` filter request mode.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceFilter {
    Disabled,
    Includes(Vec<String>),
    IncludesExcludes {
        includes: Vec<String>,
        excludes: Vec<String>,
    },
}

/// Supported P0 `_search` queries.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchQuery {
    MatchAll,
    Match {
        field: String,
        value: String,
        operator: MatchOperator,
    },
    MatchPhrase {
        field: String,
        value: String,
    },
    Term {
        field: String,
        value: String,
    },
    BoolMust(Vec<SearchQuery>),
    Fuzzy {
        field: String,
        value: String,
        fuzziness: Fuzziness,
    },
    Range {
        field: String,
        bounds: RangeBounds,
    },
    Exists {
        field: String,
    },
    Terms {
        field: String,
        values: Vec<String>,
    },
    Prefix {
        field: String,
        value: String,
    },
    Wildcard {
        field: String,
        pattern: String,
    },
    MultiMatch {
        query: String,
        fields: Vec<String>,
    },
}

/// Inclusive/exclusive numeric or lexicographic bounds for `range` queries.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeBounds {
    pub gt: Option<RangeValue>,
    pub gte: Option<RangeValue>,
    pub lt: Option<RangeValue>,
    pub lte: Option<RangeValue>,
}

/// Scalar bound for a `range` query (numeric or string).
#[derive(Clone, Debug, PartialEq)]
pub enum RangeValue {
    Number(f64),
    Text(String),
}

/// OpenSearch-compatible `_search` response for the bootstrap engine-less API.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchResponse {
    pub took: u64,
    pub timed_out: bool,
    #[serde(rename = "_shards")]
    pub shards: SearchShards,
    pub hits: SearchHits,
}

/// OpenSearch-compatible shard summary for `_search`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchShards {
    pub total: u64,
    pub successful: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// OpenSearch-compatible hit summary for `_search`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchHits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<SearchHitsTotal>,
    pub max_score: Option<f64>,
    pub hits: Vec<Value>,
}

/// OpenSearch-compatible total hit count metadata.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchHitsTotal {
    pub value: u64,
    pub relation: &'static str,
}

pub fn build_search_response(hits: Vec<Value>, total: u64, took: u64) -> SearchResponse {
    build_search_response_with_total(
        hits,
        Some(SearchHitsTotal {
            value: total,
            relation: "eq",
        }),
        took,
    )
}

pub fn build_search_response_with_total(
    hits: Vec<Value>,
    total: Option<SearchHitsTotal>,
    took: u64,
) -> SearchResponse {
    SearchResponse {
        took,
        timed_out: false,
        shards: SearchShards {
            total: 1,
            successful: 1,
            skipped: 0,
            failed: 0,
        },
        hits: SearchHits {
            total,
            max_score: None,
            hits,
        },
    }
}

/// Resolve the OpenSearch `hits.total` field shape from a `track_total_hits` mode.
pub fn resolve_total_hits(total: u64, mode: Option<&TrackTotalHits>) -> Option<SearchHitsTotal> {
    match mode {
        None | Some(TrackTotalHits::Exact) => Some(SearchHitsTotal {
            value: total,
            relation: "eq",
        }),
        Some(TrackTotalHits::Disabled) => None,
        Some(TrackTotalHits::UpTo(limit)) => {
            if total <= *limit {
                Some(SearchHitsTotal {
                    value: total,
                    relation: "eq",
                })
            } else {
                Some(SearchHitsTotal {
                    value: *limit,
                    relation: "gte",
                })
            }
        }
    }
}

fn documents_by_term(
    state: &AppState,
    index: &str,
    field: &str,
    value: &str,
) -> Vec<StoredDocument> {
    documents_for_ids(state, index, &state.documents_for_term(index, field, value))
}

fn documents_for_ids(state: &AppState, index: &str, ids: &[String]) -> Vec<StoredDocument> {
    if ids.is_empty() {
        return Vec::new();
    }

    let wanted = ids.iter().collect::<BTreeSet<_>>();
    state
        .documents(index)
        .into_iter()
        .filter(|document| wanted.contains(&document.id))
        .collect()
}

fn intersect_term_clauses(
    state: &AppState,
    index: &str,
    queries: &[SearchQuery],
) -> Option<Vec<String>> {
    let mut matches: Option<BTreeSet<String>> = None;
    for query in queries {
        match query {
            SearchQuery::Term { field, value } => {
                let current: BTreeSet<String> = state
                    .documents_for_term(index, field, value)
                    .into_iter()
                    .collect();
                matches = Some(match matches {
                    Some(previous) => previous.intersection(&current).cloned().collect(),
                    None => current,
                });
            }
            _ => return None,
        }
    }

    matches.map(|ids: BTreeSet<String>| ids.into_iter().collect())
}

/// Axum handler for the OpenSearch-compatible `/{index}/_search` endpoint.
pub async fn search_handler(
    State(state): State<AppState>,
    Path(target): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&target) {
        return error.into_response();
    }
    let indices = state.resolve_index(&target);
    if indices.is_empty() {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{target}] missing"),
        )
        .into_response();
    }

    match parse_search_request(&body) {
        Ok(request) => {
            let response = run_search(&state, &indices, &request);
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// A matched document paired with its `_score`.
#[derive(Clone, Debug)]
struct ScoredDocument {
    doc: StoredDocument,
    score: f64,
}

/// Execute a parsed search request against a set of physical indices and build the response.
pub fn run_search(state: &AppState, indices: &[String], request: &SearchRequest) -> SearchResponse {
    let started_at = Instant::now();
    let mut matched_documents: Vec<ScoredDocument> = Vec::new();
    for index in indices {
        matched_documents.extend(match_documents_for_index(
            state,
            index,
            request.query.as_ref(),
        ));
    }
    let scoring_enabled = request.query.as_ref().is_some_and(is_scoring_query);
    sort_scored_documents(&mut matched_documents, &request.sort, scoring_enabled);
    let max_score = compute_max_score(&matched_documents, scoring_enabled);
    let total = matched_documents.len() as u64;
    let hits = paginate_hits(request, &matched_documents, scoring_enabled);
    let total_summary = resolve_total_hits(total, request.track_total_hits.as_ref());

    let mut response = build_search_response_with_total(
        hits,
        total_summary,
        started_at.elapsed().as_millis() as u64,
    );
    response.hits.max_score = max_score;
    response
}

fn is_scoring_query(query: &SearchQuery) -> bool {
    matches!(
        query,
        SearchQuery::Match { .. }
            | SearchQuery::MatchPhrase { .. }
            | SearchQuery::MultiMatch { .. }
            | SearchQuery::Fuzzy { .. }
            | SearchQuery::BoolMust(_)
    )
}

fn compute_max_score(documents: &[ScoredDocument], scoring_enabled: bool) -> Option<f64> {
    if !scoring_enabled || documents.is_empty() {
        return None;
    }
    documents
        .iter()
        .map(|d| d.score)
        .fold(None, |acc, score| match acc {
            None => Some(score),
            Some(current) if score > current => Some(score),
            other => other,
        })
}

fn match_documents_for_index(
    state: &AppState,
    index: &str,
    query: Option<&SearchQuery>,
) -> Vec<ScoredDocument> {
    let plain = match query {
        None => state.documents(index),
        Some(query) => match query {
            SearchQuery::Term { field, value } => documents_by_term(state, index, field, value),
            SearchQuery::BoolMust(queries) => {
                if let Some(ids) = intersect_term_clauses(state, index, queries) {
                    documents_for_ids(state, index, &ids)
                } else {
                    state
                        .documents(index)
                        .into_iter()
                        .filter(|document| query_matches(query, &document.source))
                        .collect()
                }
            }
            SearchQuery::MatchAll => state.documents(index),
            _ => state
                .documents(index)
                .into_iter()
                .filter(|document| query_matches(query, &document.source))
                .collect(),
        },
    };
    score_documents(state, index, query, plain)
}

fn score_documents(
    state: &AppState,
    index: &str,
    query: Option<&SearchQuery>,
    documents: Vec<StoredDocument>,
) -> Vec<ScoredDocument> {
    let Some(query) = query else {
        return documents
            .into_iter()
            .map(|doc| ScoredDocument { doc, score: 1.0 })
            .collect();
    };

    let doc_count = state.count(index);
    documents
        .into_iter()
        .map(|doc| {
            let score = score_for_query(state, index, query, &doc, doc_count);
            ScoredDocument { doc, score }
        })
        .collect()
}

fn score_for_query(
    state: &AppState,
    index: &str,
    query: &SearchQuery,
    document: &StoredDocument,
    doc_count: u64,
) -> f64 {
    match query {
        SearchQuery::Match { field, value, .. } => {
            bm25_field_score(state, index, field, value, &document.source, doc_count).unwrap_or(1.0)
        }
        SearchQuery::MultiMatch { query, fields } => fields
            .iter()
            .map(|field| {
                bm25_field_score(state, index, field, query, &document.source, doc_count)
                    .unwrap_or(0.0)
            })
            .fold(0.0_f64, f64::max)
            .max(1.0 / 1e9_f64.max(1.0)),
        SearchQuery::MatchPhrase { field, value } => {
            bm25_field_score(state, index, field, value, &document.source, doc_count).unwrap_or(1.0)
        }
        SearchQuery::Fuzzy { field, value, .. } => {
            bm25_field_score(state, index, field, value, &document.source, doc_count).unwrap_or(1.0)
        }
        SearchQuery::BoolMust(clauses) => clauses
            .iter()
            .map(|clause| score_for_query(state, index, clause, document, doc_count))
            .sum(),
        _ => 1.0,
    }
}

fn bm25_field_score(
    state: &AppState,
    index: &str,
    field: &str,
    query: &str,
    source: &Value,
    doc_count: u64,
) -> Option<f64> {
    let query_tokens = tokenize_for_search(query);
    if query_tokens.is_empty() || doc_count == 0 {
        return None;
    }
    let field_tokens = field_tokens_for_source(source, field);
    if field_tokens.is_empty() {
        return None;
    }
    let doc_len = field_tokens.len() as u64;
    let avg_doc_len = compute_avg_doc_len(state, index, field)?;
    if avg_doc_len <= 0.0 {
        return None;
    }
    let config = Bm25Config::default();
    let mut total = 0.0_f64;
    for query_token in &query_tokens {
        let term_freq = field_tokens
            .iter()
            .filter(|token| token.as_str() == query_token.as_str())
            .count() as u64;
        if term_freq == 0 {
            continue;
        }
        let doc_freq = state.documents_for_term(index, field, query_token).len() as u64;
        if doc_freq == 0 || doc_freq > doc_count {
            continue;
        }
        if let Ok(score) = bm25_score(config, doc_count, doc_freq, term_freq, doc_len, avg_doc_len)
        {
            total += score;
        }
    }
    if total > 0.0 {
        Some(total)
    } else {
        None
    }
}

fn field_tokens_for_source(source: &Value, field: &str) -> Vec<String> {
    field_text(source, field)
        .map(|text| tokenize_for_search(&text))
        .unwrap_or_default()
}

fn compute_avg_doc_len(state: &AppState, index: &str, field: &str) -> Option<f64> {
    let documents = state.documents(index);
    if documents.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    let mut docs_with_field: u64 = 0;
    for doc in &documents {
        let tokens = field_tokens_for_source(&doc.source, field);
        if !tokens.is_empty() {
            total += tokens.len() as u64;
            docs_with_field += 1;
        }
    }
    if docs_with_field == 0 {
        None
    } else {
        Some(total as f64 / docs_with_field as f64)
    }
}

pub fn parse_search_request(body: &str) -> Result<SearchRequest, OpenSearchError> {
    if body.trim().is_empty() {
        return Ok(SearchRequest {
            query: None,
            from: None,
            size: None,
            source: None,
            track_total_hits: None,
            sort: Vec::new(),
        });
    }

    let value: Value = serde_json::from_str(body).map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            error.to_string(),
        )
    })?;

    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "search request body must be an object",
        )
    })?;

    let query = object.get("query").map(parse_search_query).transpose()?;
    let from = object
        .get("from")
        .map(|value| parse_non_negative_integer("from", value))
        .transpose()?;
    let size = object
        .get("size")
        .map(|value| parse_non_negative_integer("size", value))
        .transpose()?;
    let source = object.get("_source").map(parse_source_filter).transpose()?;
    let track_total_hits = object
        .get("track_total_hits")
        .map(parse_track_total_hits)
        .transpose()?;
    let sort = object
        .get("sort")
        .map(parse_sort)
        .transpose()?
        .unwrap_or_default();

    Ok(SearchRequest {
        query,
        from,
        size,
        source,
        track_total_hits,
        sort,
    })
}

fn parse_sort(value: &Value) -> Result<Vec<SortClause>, OpenSearchError> {
    match value {
        Value::String(field) => Ok(vec![SortClause {
            field: parse_sort_field_name(field)?,
            order: SortOrder::Asc,
        }]),
        Value::Array(items) => items.iter().map(parse_sort_entry).collect(),
        Value::Object(_) => parse_sort_entry(value).map(|clause| vec![clause]),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`sort` must be a string, object, or array",
        )),
    }
}

fn parse_sort_entry(value: &Value) -> Result<SortClause, OpenSearchError> {
    match value {
        Value::String(field) => Ok(SortClause {
            field: parse_sort_field_name(field)?,
            order: SortOrder::Asc,
        }),
        Value::Object(object) => {
            if object.len() != 1 {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "`sort` object must contain exactly one field",
                ));
            }
            let (field, body) = object.iter().next().expect("object has one field");
            let field = parse_sort_field_name(field)?;
            let order = parse_sort_order(body)?;
            Ok(SortClause { field, order })
        }
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`sort` entries must be strings or objects",
        )),
    }
}

fn parse_sort_field_name(field: &str) -> Result<String, OpenSearchError> {
    if field.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`sort` field name must not be empty",
        ));
    }
    Ok(field.to_owned())
}

fn parse_sort_order(value: &Value) -> Result<SortOrder, OpenSearchError> {
    let order_string = match value {
        Value::String(text) => text.clone(),
        Value::Object(object) => match object.get("order") {
            Some(Value::String(text)) => text.clone(),
            Some(_) => {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "`sort.order` must be a string",
                ));
            }
            None => "asc".to_owned(),
        },
        _ => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "`sort` order must be a string or object",
            ));
        }
    };
    match order_string.as_str() {
        "asc" => Ok(SortOrder::Asc),
        "desc" => Ok(SortOrder::Desc),
        unknown => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("unknown `sort` order `{unknown}`"),
        )),
    }
}

fn parse_track_total_hits(value: &Value) -> Result<TrackTotalHits, OpenSearchError> {
    match value {
        Value::Bool(true) => Ok(TrackTotalHits::Exact),
        Value::Bool(false) => Ok(TrackTotalHits::Disabled),
        Value::Number(number) => {
            let limit = number.as_u64().ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "`track_total_hits` must be a non-negative integer or boolean",
                )
            })?;
            Ok(TrackTotalHits::UpTo(limit))
        }
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`track_total_hits` must be a boolean or non-negative integer",
        )),
    }
}

pub fn parse_source_filter(value: &Value) -> Result<SourceFilter, OpenSearchError> {
    match value {
        Value::Bool(false) => Ok(SourceFilter::Disabled),
        Value::Bool(true) => Ok(SourceFilter::IncludesExcludes {
            includes: Vec::new(),
            excludes: Vec::new(),
        }),
        Value::String(text) => Ok(SourceFilter::Includes(vec![text.clone()])),
        Value::Array(items) => Ok(SourceFilter::Includes(parse_source_field_array(
            items, "_source",
        )?)),
        Value::Object(object) => {
            for key in object.keys() {
                if !matches!(key.as_str(), "includes" | "excludes") {
                    return Err(OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        format!("unsupported `_source` field `{key}`"),
                    ));
                }
            }
            let includes = match object.get("includes") {
                Some(Value::Array(items)) => parse_source_field_array(items, "_source.includes")?,
                Some(Value::String(text)) => vec![text.clone()],
                Some(_) => {
                    return Err(OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        "`_source.includes` must be a string or array of strings",
                    ));
                }
                None => Vec::new(),
            };
            let excludes = match object.get("excludes") {
                Some(Value::Array(items)) => parse_source_field_array(items, "_source.excludes")?,
                Some(Value::String(text)) => vec![text.clone()],
                Some(_) => {
                    return Err(OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        "`_source.excludes` must be a string or array of strings",
                    ));
                }
                None => Vec::new(),
            };
            Ok(SourceFilter::IncludesExcludes { includes, excludes })
        }
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`_source` must be a boolean, string, array, or object",
        )),
    }
}

fn parse_source_field_array(
    items: &[Value],
    context: &str,
) -> Result<Vec<String>, OpenSearchError> {
    items
        .iter()
        .map(|item| match item {
            Value::String(text) if !text.is_empty() => Ok(text.clone()),
            Value::String(_) => Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("`{context}` field name must not be empty"),
            )),
            _ => Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("`{context}` entries must be strings"),
            )),
        })
        .collect()
}

fn parse_search_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "search query must be an object",
        )
    })?;

    if object.len() != 1 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "search query must contain exactly one query type",
        ));
    }

    let (query_type, query_body) = object.iter().next().expect("object has one query type");
    match query_type.as_str() {
        "match_all" if query_body.as_object().is_some_and(|body| body.is_empty()) => {
            Ok(SearchQuery::MatchAll)
        }
        "match_all" => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "match_all query body must be an empty object",
        )),
        "match" => parse_match_query(query_body),
        "match_phrase" => parse_match_phrase_query(query_body),
        "term" => parse_term_query(query_body),
        "bool" => parse_bool_query(query_body),
        "fuzzy" => parse_fuzzy_query(query_body),
        "range" => parse_range_query(query_body),
        "exists" => parse_exists_query(query_body),
        "terms" => parse_terms_query(query_body),
        "prefix" => parse_prefix_query(query_body),
        "wildcard" => parse_wildcard_query(query_body),
        "multi_match" => parse_multi_match_query(query_body),
        unknown => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("unsupported search query `{unknown}`"),
        )),
    }
}

fn parse_match_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, body) = parse_single_field_query("match", value)?;
    let query_text = parse_query_text(body, "match query")?;
    let operator = parse_match_operator(body, "match")?;

    Ok(SearchQuery::Match {
        field,
        value: query_text,
        operator,
    })
}

fn parse_match_operator(body: &Value, context: &str) -> Result<MatchOperator, OpenSearchError> {
    let Value::Object(object) = body else {
        return Ok(MatchOperator::default());
    };
    let Some(raw) = object.get("operator") else {
        return Ok(MatchOperator::default());
    };
    match raw {
        Value::String(text) => match text.to_ascii_lowercase().as_str() {
            "or" => Ok(MatchOperator::Or),
            "and" => Ok(MatchOperator::And),
            unknown => Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("{context} `operator` must be `OR` or `AND`, got `{unknown}`"),
            )),
        },
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{context} `operator` must be a string"),
        )),
    }
}

fn parse_match_phrase_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, value) = parse_single_field_query("match_phrase", value)?;
    let value = parse_query_text(value, "match_phrase query")?;

    Ok(SearchQuery::MatchPhrase { field, value })
}

fn parse_term_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, value) = parse_single_field_query("term", value)?;
    let value = parse_term_query_value(value)?;

    Ok(SearchQuery::Term { field, value })
}

fn parse_term_query_value(value: &Value) -> Result<String, OpenSearchError> {
    match value {
        Value::Object(object) => {
            let query_value = object.get("value").ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "term field query object must contain `value`",
                )
            })?;

            parse_scalar_query_text(query_value, "term query value")
        }
        _ => parse_scalar_query_text(value, "term query"),
    }
}

fn parse_bool_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "bool query body must be an object",
        )
    })?;

    let must = object.get("must").ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "bool query must contain `must`",
        )
    })?;
    let must = must.as_array().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "bool query `must` must be an array",
        )
    })?;

    let mut queries = Vec::with_capacity(must.len());
    for query in must {
        queries.push(parse_search_query(query)?);
    }

    Ok(SearchQuery::BoolMust(queries))
}

fn parse_fuzzy_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, value) = parse_single_field_query("fuzzy", value)?;
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "fuzzy field query body must be an object",
        )
    })?;
    let query_value = object.get("value").ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "fuzzy field query must contain `value`",
        )
    })?;
    let value = parse_query_text(query_value, "fuzzy query value")?;
    let fuzziness = object
        .get("fuzziness")
        .map(parse_fuzzy_query_fuzziness)
        .transpose()?
        .unwrap_or(Fuzziness::Edits(2));

    Ok(SearchQuery::Fuzzy {
        field,
        value,
        fuzziness,
    })
}

/// Parse the body of a `range` query into typed bounds.
pub fn parse_range_bounds(value: &Value) -> Result<RangeBounds, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "range field query body must be an object",
        )
    })?;

    let mut bounds = RangeBounds {
        gt: None,
        gte: None,
        lt: None,
        lte: None,
    };

    for (key, raw) in object {
        let parsed = parse_range_bound_value(raw)?;
        match key.as_str() {
            "gt" => bounds.gt = Some(parsed),
            "gte" => bounds.gte = Some(parsed),
            "lt" => bounds.lt = Some(parsed),
            "lte" => bounds.lte = Some(parsed),
            "boost" | "format" | "relation" | "time_zone" => {}
            unknown => {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    format!("unsupported range query field `{unknown}`"),
                ));
            }
        }
    }

    if bounds.gt.is_none() && bounds.gte.is_none() && bounds.lt.is_none() && bounds.lte.is_none() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "range query must contain at least one of `gt`, `gte`, `lt`, `lte`",
        ));
    }

    Ok(bounds)
}

fn parse_range_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, body) = parse_single_field_query("range", value)?;
    let bounds = parse_range_bounds(body)?;
    Ok(SearchQuery::Range { field, bounds })
}

/// Parse a `terms` query body and return `(field, values)`.
pub fn parse_terms_clause(value: &Value) -> Result<(String, Vec<String>), OpenSearchError> {
    let (field, body) = parse_single_field_query("terms", value)?;
    let array = body.as_array().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "terms query value must be an array",
        )
    })?;
    if array.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "terms query value array must not be empty",
        ));
    }
    let values = array
        .iter()
        .map(|item| parse_scalar_query_text(item, "terms query value"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((field, values))
}

fn parse_terms_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, values) = parse_terms_clause(value)?;
    Ok(SearchQuery::Terms { field, values })
}

/// Parse an `exists` query body and return the target field name.
pub fn parse_exists_clause(value: &Value) -> Result<String, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "exists query body must be an object",
        )
    })?;
    let field = object.get("field").ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "exists query must contain `field`",
        )
    })?;
    match field {
        Value::String(text) if !text.is_empty() => Ok(text.clone()),
        Value::String(_) => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "exists query `field` must not be empty",
        )),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "exists query `field` must be a string",
        )),
    }
}

fn parse_exists_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let field = parse_exists_clause(value)?;
    Ok(SearchQuery::Exists { field })
}

/// Parse a `prefix` query body and return `(field, value)`.
pub fn parse_prefix_clause(value: &Value) -> Result<(String, String), OpenSearchError> {
    let (field, body) = parse_single_field_query("prefix", value)?;
    let value = parse_term_query_value(body)?;
    Ok((field, value))
}

fn parse_prefix_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, value) = parse_prefix_clause(value)?;
    Ok(SearchQuery::Prefix { field, value })
}

/// Parse a `wildcard` query body and return `(field, pattern)`.
pub fn parse_wildcard_clause(value: &Value) -> Result<(String, String), OpenSearchError> {
    let (field, body) = parse_single_field_query("wildcard", value)?;
    let pattern = parse_term_query_value(body)?;
    if pattern.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "wildcard query value must not be empty",
        ));
    }
    Ok((field, pattern))
}

fn parse_wildcard_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, pattern) = parse_wildcard_clause(value)?;
    Ok(SearchQuery::Wildcard { field, pattern })
}

/// Parse a `multi_match` query body and return `(query_text, fields)`.
pub fn parse_multi_match_clause(value: &Value) -> Result<(String, Vec<String>), OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "multi_match query body must be an object",
        )
    })?;

    let query_value = object.get("query").ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "multi_match query must contain `query`",
        )
    })?;
    let query_text = parse_scalar_query_text(query_value, "multi_match query")?;

    let fields_value = object.get("fields").ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "multi_match query must contain `fields`",
        )
    })?;
    let fields_array = fields_value.as_array().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "multi_match `fields` must be an array",
        )
    })?;
    if fields_array.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "multi_match `fields` must not be empty",
        ));
    }
    let fields = fields_array
        .iter()
        .map(|item| match item {
            Value::String(text) if !text.is_empty() => Ok(text.clone()),
            Value::String(_) => Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "multi_match `fields` entries must not be empty",
            )),
            _ => Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "multi_match `fields` entries must be strings",
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;

    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "query" | "fields" | "type" | "operator" | "tie_breaker" | "boost"
        ) {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("unsupported multi_match field `{key}`"),
            ));
        }
    }

    Ok((query_text, fields))
}

fn parse_multi_match_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (query, fields) = parse_multi_match_clause(value)?;
    Ok(SearchQuery::MultiMatch { query, fields })
}

fn parse_range_bound_value(value: &Value) -> Result<RangeValue, OpenSearchError> {
    match value {
        Value::Number(number) => number.as_f64().map(RangeValue::Number).ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "range bound number must fit in a 64-bit float",
            )
        }),
        Value::String(text) => Ok(RangeValue::Text(text.clone())),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "range bound must be a number or string",
        )),
    }
}

fn parse_single_field_query<'a>(
    query_type: &str,
    value: &'a Value,
) -> Result<(String, &'a Value), OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{query_type} query body must be an object"),
        )
    })?;

    if object.len() != 1 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{query_type} query must contain exactly one field"),
        ));
    }

    let (field, value) = object.iter().next().expect("object has one field");
    Ok((field.clone(), value))
}

fn parse_query_text(value: &Value, context: &str) -> Result<String, OpenSearchError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Object(object) => object
            .get("query")
            .ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    format!("{context} object must contain `query`"),
                )
            })
            .and_then(|value| parse_query_text(value, context)),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{context} must be a scalar value"),
        )),
    }
}

fn parse_scalar_query_text(value: &Value, context: &str) -> Result<String, OpenSearchError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{context} must be a scalar value"),
        )),
    }
}

fn parse_fuzzy_query_fuzziness(value: &Value) -> Result<Fuzziness, OpenSearchError> {
    match value {
        Value::String(text) => fuzzy_result(parse_fuzziness(text)),
        Value::Number(number) => {
            let edits = number
                .as_u64()
                .map(|edits| edits.to_string())
                .ok_or_else(|| {
                    OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        "fuzzy query `fuzziness` must be a non-negative integer or string",
                    )
                })?;

            fuzzy_result(parse_fuzziness(&edits))
        }
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "fuzzy query `fuzziness` must be a non-negative integer or string",
        )),
    }
}

fn fuzzy_result<T>(result: Result<T, impl std::fmt::Display>) -> Result<T, OpenSearchError> {
    result.map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            error.to_string(),
        )
    })
}

fn parse_non_negative_integer(field: &str, value: &Value) -> Result<u64, OpenSearchError> {
    value.as_u64().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("search `{field}` must be a non-negative integer"),
        )
    })
}

fn query_matches(query: &SearchQuery, source: &Value) -> bool {
    match query {
        SearchQuery::MatchAll => true,
        SearchQuery::Match {
            field,
            value,
            operator,
        } => field_matches(source, field, value, *operator),
        SearchQuery::MatchPhrase { field, value } => {
            match_phrase_field_matches(source, field, value)
        }
        SearchQuery::Term { field, value } => term_field_matches(source, field, value),
        SearchQuery::BoolMust(queries) => queries.iter().all(|query| query_matches(query, source)),
        SearchQuery::Fuzzy {
            field,
            value,
            fuzziness,
        } => fuzzy_field_matches(source, field, value, *fuzziness),
        SearchQuery::Range { field, bounds } => range_field_matches(source, field, bounds),
        SearchQuery::Exists { field } => exists_field_matches(source, field),
        SearchQuery::Terms { field, values } => values
            .iter()
            .any(|value| term_field_matches(source, field, value)),
        SearchQuery::Prefix { field, value } => prefix_field_matches(source, field, value),
        SearchQuery::Wildcard { field, pattern } => wildcard_field_matches(source, field, pattern),
        SearchQuery::MultiMatch { query, fields } => multi_match_matches(source, fields, query),
    }
}

fn field_matches(source: &Value, field: &str, query: &str, operator: MatchOperator) -> bool {
    let query_tokens = tokenize_for_search(query);
    if query_tokens.is_empty() {
        return false;
    }
    let Some(text) = field_text(source, field) else {
        return false;
    };
    let field_tokens = tokenize_for_search(&text);
    if field_tokens.is_empty() {
        return false;
    }
    match operator {
        MatchOperator::Or => query_tokens
            .iter()
            .any(|token| field_tokens.iter().any(|field_token| field_token == token)),
        MatchOperator::And => query_tokens
            .iter()
            .all(|token| field_tokens.iter().any(|field_token| field_token == token)),
    }
}

fn match_phrase_field_matches(source: &Value, field: &str, query: &str) -> bool {
    let query_tokens = tokenize_for_search(query);
    if query_tokens.is_empty() {
        return false;
    }

    field_text(source, field)
        .map(|value| {
            tokenize_for_search(&value)
                .windows(query_tokens.len())
                .any(|field_window| field_window == query_tokens.as_slice())
        })
        .unwrap_or(false)
}

fn term_field_matches(source: &Value, field: &str, query: &str) -> bool {
    let query = normalize_text(query);
    if query.is_empty() {
        return false;
    }

    field_text(source, field)
        .map(|value| {
            tokenize_for_search(&value)
                .iter()
                .any(|field_token| field_token == &query)
        })
        .unwrap_or(false)
}

pub fn multi_match_matches(source: &Value, fields: &[String], query: &str) -> bool {
    fields
        .iter()
        .any(|field| field_matches(source, field, query, MatchOperator::Or))
}

pub fn prefix_field_matches(source: &Value, field: &str, prefix: &str) -> bool {
    let prefix = normalize_text(prefix);
    if prefix.is_empty() {
        return false;
    }
    let Some(text) = field_text(source, field) else {
        return false;
    };
    tokenize_for_search(&text)
        .iter()
        .any(|token| token.starts_with(&prefix))
}

pub fn wildcard_field_matches(source: &Value, field: &str, pattern: &str) -> bool {
    let pattern = normalize_wildcard_pattern(pattern);
    if pattern.is_empty() {
        return false;
    }
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let Some(text) = field_text(source, field) else {
        return false;
    };
    tokenize_for_search(&text).iter().any(|token| {
        let token_chars: Vec<char> = token.chars().collect();
        wildcard_pattern_matches(&pattern_chars, &token_chars)
    })
}

fn normalize_wildcard_pattern(pattern: &str) -> String {
    pattern
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            '*' | '?' => character,
            other => fold_search_char(other),
        })
        .collect()
}

fn wildcard_pattern_matches(pattern: &[char], text: &[char]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi: Option<usize> = None;
    let mut star_ti = 0;

    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }

    pi >= pattern.len()
}

pub fn exists_field_matches(source: &Value, field: &str) -> bool {
    match source.get(field) {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

pub fn range_field_matches(source: &Value, field: &str, bounds: &RangeBounds) -> bool {
    let Some(field_value) = source.get(field) else {
        return false;
    };
    if let Some(number) = field_value.as_f64() {
        return numeric_in_bounds(number, bounds);
    }
    if let Some(text) = field_value.as_str() {
        return text_in_bounds(text, bounds);
    }
    false
}

fn numeric_in_bounds(value: f64, bounds: &RangeBounds) -> bool {
    if let Some(RangeValue::Number(threshold)) = &bounds.gt {
        if value <= *threshold {
            return false;
        }
    }
    if let Some(RangeValue::Number(threshold)) = &bounds.gte {
        if value < *threshold {
            return false;
        }
    }
    if let Some(RangeValue::Number(threshold)) = &bounds.lt {
        if value >= *threshold {
            return false;
        }
    }
    if let Some(RangeValue::Number(threshold)) = &bounds.lte {
        if value > *threshold {
            return false;
        }
    }
    true
}

fn text_in_bounds(value: &str, bounds: &RangeBounds) -> bool {
    if let Some(RangeValue::Text(threshold)) = &bounds.gt {
        if value <= threshold.as_str() {
            return false;
        }
    }
    if let Some(RangeValue::Text(threshold)) = &bounds.gte {
        if value < threshold.as_str() {
            return false;
        }
    }
    if let Some(RangeValue::Text(threshold)) = &bounds.lt {
        if value >= threshold.as_str() {
            return false;
        }
    }
    if let Some(RangeValue::Text(threshold)) = &bounds.lte {
        if value > threshold.as_str() {
            return false;
        }
    }
    true
}

fn fuzzy_field_matches(source: &Value, field: &str, query: &str, fuzziness: Fuzziness) -> bool {
    let query_tokens = tokenize_for_search(query);
    if query_tokens.is_empty() {
        return false;
    }

    let Some(field_text) = field_text(source, field) else {
        return false;
    };
    let field_tokens = tokenize_for_search(&field_text);

    query_tokens.iter().all(|query_token| {
        field_tokens
            .iter()
            .any(|field_token| fuzzy_token_matches(query_token, field_token, fuzziness))
    })
}

fn fuzzy_token_matches(query: &str, candidate: &str, fuzziness: Fuzziness) -> bool {
    let Ok(max_edits) = edits_for_term_len(fuzziness, query.chars().count()) else {
        return false;
    };

    bounded_damerau_levenshtein(query, candidate, max_edits, true)
        .map(|distance| distance.is_some())
        .unwrap_or(false)
}

fn field_text(source: &Value, field: &str) -> Option<String> {
    match source.get(field)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn sort_scored_documents(
    documents: &mut [ScoredDocument],
    clauses: &[SortClause],
    scoring_enabled: bool,
) {
    if clauses.is_empty() {
        if scoring_enabled {
            documents.sort_by(|left, right| {
                right
                    .score
                    .partial_cmp(&left.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        return;
    }
    documents.sort_by(|left, right| {
        for clause in clauses {
            let ordering = compare_field(
                left.doc.source.get(&clause.field),
                right.doc.source.get(&clause.field),
                clause.order,
            );
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        std::cmp::Ordering::Equal
    });
}

fn compare_field(
    left: Option<&Value>,
    right: Option<&Value>,
    order: SortOrder,
) -> std::cmp::Ordering {
    match (left, right) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(a), Some(b)) => {
            let base = compare_values(a, b);
            match order {
                SortOrder::Asc => base,
                SortOrder::Desc => base.reverse(),
            }
        }
    }
}

fn compare_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        _ => std::cmp::Ordering::Equal,
    }
}

fn paginate_hits(
    request: &SearchRequest,
    documents: &[ScoredDocument],
    scoring_enabled: bool,
) -> Vec<Value> {
    let from = usize::try_from(request.from.unwrap_or(0)).unwrap_or(usize::MAX);
    let size = usize::try_from(request.size.unwrap_or(10)).unwrap_or(usize::MAX);

    documents
        .iter()
        .skip(from)
        .take(size)
        .map(|scored| {
            build_hit(
                &scored.doc,
                request.source.as_ref(),
                scoring_enabled.then_some(scored.score),
            )
        })
        .collect()
}

fn build_hit(
    document: &StoredDocument,
    filter: Option<&SourceFilter>,
    score: Option<f64>,
) -> Value {
    let mut hit = json!({
        "_index": document.index,
        "_id": document.id,
    });
    let object = hit.as_object_mut().expect("hit object");
    if let Some(score) = score {
        object.insert("_score".to_owned(), json!(score));
    }
    if let Some(source) = apply_source_filter(&document.source, filter) {
        object.insert("_source".to_owned(), source);
    }
    hit
}

pub fn apply_source_filter(source: &Value, filter: Option<&SourceFilter>) -> Option<Value> {
    match filter {
        None => Some(source.clone()),
        Some(SourceFilter::Disabled) => None,
        Some(SourceFilter::Includes(fields)) => Some(filter_source_fields(source, fields, &[])),
        Some(SourceFilter::IncludesExcludes { includes, excludes }) => {
            Some(filter_source_fields(source, includes, excludes))
        }
    }
}

fn filter_source_fields(source: &Value, includes: &[String], excludes: &[String]) -> Value {
    let Value::Object(object) = source else {
        return source.clone();
    };

    let mut filtered = serde_json::Map::new();
    for (key, value) in object {
        if !includes.is_empty() && !includes.iter().any(|field| field == key) {
            continue;
        }
        if excludes.iter().any(|field| field == key) {
            continue;
        }
        filtered.insert(key.clone(), value.clone());
    }

    Value::Object(filtered)
}

fn tokenize_for_search(value: &str) -> Vec<String> {
    normalize_text(value)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn normalize_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(fold_search_char)
        .collect()
}

fn fold_search_char(character: char) -> char {
    match character {
        '\u{00e0}' | '\u{00e1}' | '\u{00e2}' | '\u{00e3}' | '\u{00e4}' | '\u{00e5}' => 'a',
        '\u{00e7}' => 'c',
        '\u{00e8}' | '\u{00e9}' | '\u{00ea}' | '\u{00eb}' => 'e',
        '\u{00ec}' | '\u{00ed}' | '\u{00ee}' | '\u{00ef}' => 'i',
        '\u{00f1}' => 'n',
        '\u{00f2}' | '\u{00f3}' | '\u{00f4}' | '\u{00f5}' | '\u{00f6}' => 'o',
        '\u{00f9}' | '\u{00fa}' | '\u{00fb}' | '\u{00fc}' => 'u',
        '\u{00fd}' | '\u{00ff}' => 'y',
        character if character.is_alphanumeric() => character,
        _ => ' ',
    }
}
