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
    Path(index): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }
    if !state.index_exists(&index) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{index}] missing"),
        )
        .into_response();
    }

    let started_at = Instant::now();
    match parse_search_request(&body) {
        Ok(request) => {
            let matched_documents: Vec<StoredDocument> = match request.query.as_ref() {
                None => state.documents(&index),
                Some(query) => match query {
                    SearchQuery::Term { field, value } => {
                        documents_by_term(&state, &index, field, value)
                    }
                    SearchQuery::BoolMust(queries) => {
                        if let Some(ids) = intersect_term_clauses(&state, &index, queries) {
                            documents_for_ids(&state, &index, &ids)
                        } else {
                            state
                                .documents(&index)
                                .into_iter()
                                .filter(|document| query_matches(query, &document.source))
                                .collect()
                        }
                    }
                    SearchQuery::MatchAll => state.documents(&index),
                    _ => state
                        .documents(&index)
                        .into_iter()
                        .filter(|document| query_matches(query, &document.source))
                        .collect(),
                },
            };
            let total = matched_documents.len() as u64;
            let hits = paginate_hits(&request, &matched_documents);
            let total_summary = resolve_total_hits(total, request.track_total_hits.as_ref());

            (
                StatusCode::OK,
                Json(build_search_response_with_total(
                    hits,
                    total_summary,
                    started_at.elapsed().as_millis() as u64,
                )),
            )
                .into_response()
        }
        Err(error) => error.into_response(),
    }
}

fn parse_search_request(body: &str) -> Result<SearchRequest, OpenSearchError> {
    if body.trim().is_empty() {
        return Ok(SearchRequest {
            query: None,
            from: None,
            size: None,
            source: None,
            track_total_hits: None,
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

    Ok(SearchRequest {
        query,
        from,
        size,
        source,
        track_total_hits,
    })
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

fn parse_source_filter(value: &Value) -> Result<SourceFilter, OpenSearchError> {
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
        unknown => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("unsupported search query `{unknown}`"),
        )),
    }
}

fn parse_match_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, value) = parse_single_field_query("match", value)?;
    let value = parse_query_text(value, "match query")?;

    Ok(SearchQuery::Match { field, value })
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
        SearchQuery::Match { field, value } => field_matches(source, field, value),
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
    }
}

fn field_matches(source: &Value, field: &str, query: &str) -> bool {
    let query = normalize_text(query);
    !query.is_empty()
        && field_text(source, field)
            .map(|value| normalize_text(&value).contains(&query))
            .unwrap_or(false)
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

fn paginate_hits(request: &SearchRequest, documents: &[StoredDocument]) -> Vec<Value> {
    let from = usize::try_from(request.from.unwrap_or(0)).unwrap_or(usize::MAX);
    let size = usize::try_from(request.size.unwrap_or(10)).unwrap_or(usize::MAX);

    documents
        .iter()
        .skip(from)
        .take(size)
        .map(|document| build_hit(document, request.source.as_ref()))
        .collect()
}

fn build_hit(document: &StoredDocument, filter: Option<&SourceFilter>) -> Value {
    let mut hit = json!({
        "_index": document.index,
        "_id": document.id,
    });
    if let Some(source) = apply_source_filter(&document.source, filter) {
        hit.as_object_mut()
            .expect("hit object")
            .insert("_source".to_owned(), source);
    }
    hit
}

fn apply_source_filter(source: &Value, filter: Option<&SourceFilter>) -> Option<Value> {
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
