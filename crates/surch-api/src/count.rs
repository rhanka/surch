use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

use crate::{
    index::validate_index_name,
    search::{
        exists_field_matches, multi_match_matches, parse_exists_clause, parse_multi_match_clause,
        parse_prefix_clause, parse_range_bounds, parse_terms_clause, parse_wildcard_clause,
        prefix_field_matches, range_field_matches, wildcard_field_matches, MatchOperator,
        RangeBounds, TrackTotalHits,
    },
    state::AppState,
    OpenSearchError,
};

/// OpenSearch-compatible `_count` request body for the P0 bootstrap surface.
#[derive(Clone, Debug, PartialEq)]
pub struct CountRequest {
    pub query: Option<CountQuery>,
    pub track_total_hits: Option<TrackTotalHits>,
}

/// Supported P0 `_count` queries.
#[derive(Clone, Debug, PartialEq)]
pub enum CountQuery {
    MatchAll,
    Term {
        field: String,
        value: String,
    },
    BoolMust(Vec<CountQuery>),
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
        operator: MatchOperator,
    },
}

/// OpenSearch-compatible `_count` response for the bootstrap engine-less API.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CountResponse {
    pub count: u64,
    #[serde(rename = "_shards")]
    pub shards: CountShards,
}

/// OpenSearch-compatible shard summary for `_count`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CountShards {
    pub total: u64,
    pub successful: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// Build a deterministic P0 OpenSearch-compatible `_count` response.
pub fn build_count_response(count: u64) -> CountResponse {
    CountResponse {
        count,
        shards: CountShards {
            total: 1,
            successful: 1,
            skipped: 0,
            failed: 0,
        },
    }
}

/// Axum handler for the OpenSearch-compatible `/{index}/_count` endpoint.
pub async fn count_handler(
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

    match parse_count_request(&body) {
        Ok(request) => {
            let count: u64 = indices
                .iter()
                .map(|index| count_matches(&state, index, &request))
                .sum();
            let count = match request.track_total_hits {
                Some(TrackTotalHits::UpTo(limit)) => count.min(limit),
                Some(TrackTotalHits::Disabled) | Some(TrackTotalHits::Exact) | None => count,
            };

            (StatusCode::OK, Json(build_count_response(count))).into_response()
        }
        Err(error) => error.into_response(),
    }
}

fn count_matches(state: &AppState, index: &str, request: &CountRequest) -> u64 {
    match request.query.as_ref() {
        None => state.count(index),
        Some(query) => count_query_matches(state, index, query),
    }
}

fn count_query_matches(state: &AppState, index: &str, query: &CountQuery) -> u64 {
    match query {
        CountQuery::MatchAll => state.count(index),
        CountQuery::Term { field, value } => state.term_matches_count(index, field, value) as u64,
        CountQuery::BoolMust(clauses) => {
            if let Some(documents) = intersect_term_clauses(state, index, clauses) {
                documents.len() as u64
            } else {
                let documents = state.documents(index);
                documents
                    .into_iter()
                    .filter(|document| query_matches(query, &document.source))
                    .count() as u64
            }
        }
        CountQuery::Range { .. }
        | CountQuery::Exists { .. }
        | CountQuery::Terms { .. }
        | CountQuery::Prefix { .. }
        | CountQuery::Wildcard { .. }
        | CountQuery::MultiMatch { .. } => state
            .documents(index)
            .into_iter()
            .filter(|document| query_matches(query, &document.source))
            .count() as u64,
    }
}

fn intersect_term_clauses(
    state: &AppState,
    index: &str,
    clauses: &[CountQuery],
) -> Option<Vec<String>> {
    let mut matches: Option<BTreeSet<String>> = None;
    for clause in clauses {
        match clause {
            CountQuery::Term { field, value } => {
                let ids = state.documents_for_term(index, field, value);
                let current = ids.into_iter().collect::<BTreeSet<_>>();
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

fn parse_count_request(body: &str) -> Result<CountRequest, OpenSearchError> {
    if body.trim().is_empty() {
        return Ok(CountRequest {
            query: None,
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
            "count request body must be an object",
        )
    })?;

    let query = object.get("query").map(parse_count_query).transpose()?;
    let track_total_hits = object
        .get("track_total_hits")
        .map(parse_track_total_hits)
        .transpose()?;

    Ok(CountRequest {
        query,
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

fn parse_count_query(value: &Value) -> Result<CountQuery, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "count query must be an object",
        )
    })?;

    if object.len() != 1 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "count query must contain exactly one query type",
        ));
    }

    let (query_type, query_body) = object.iter().next().expect("object has one query type");
    match query_type.as_str() {
        "match_all" if query_body.as_object().is_some_and(|body| body.is_empty()) => {
            Ok(CountQuery::MatchAll)
        }
        "match_all" => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "match_all query body must be an empty object",
        )),
        "term" => parse_term_query(query_body),
        "bool" => parse_bool_query(query_body),
        "range" => parse_range_count_query(query_body),
        "exists" => {
            let field = parse_exists_clause(query_body)?;
            Ok(CountQuery::Exists { field })
        }
        "terms" => {
            let (field, values) = parse_terms_clause(query_body)?;
            Ok(CountQuery::Terms { field, values })
        }
        "prefix" => {
            let (field, value) = parse_prefix_clause(query_body)?;
            Ok(CountQuery::Prefix { field, value })
        }
        "wildcard" => {
            let (field, pattern) = parse_wildcard_clause(query_body)?;
            Ok(CountQuery::Wildcard { field, pattern })
        }
        "multi_match" => {
            let (query, fields, operator) = parse_multi_match_clause(query_body)?;
            Ok(CountQuery::MultiMatch {
                query,
                fields,
                operator,
            })
        }
        unknown => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("unsupported count query `{unknown}`"),
        )),
    }
}

fn parse_range_count_query(value: &Value) -> Result<CountQuery, OpenSearchError> {
    let (field, body) = parse_single_field_query("range", value)?;
    let bounds = parse_range_bounds(body)?;
    Ok(CountQuery::Range { field, bounds })
}

fn parse_term_query(value: &Value) -> Result<CountQuery, OpenSearchError> {
    let (field, value) = parse_single_field_query("term", value)?;
    let value = parse_term_value(value)?;

    Ok(CountQuery::Term { field, value })
}

fn parse_bool_query(value: &Value) -> Result<CountQuery, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "bool query body must be an object",
        )
    })?;

    let must = object
        .get("must")
        .and_then(Value::as_array)
        .filter(|must| !must.is_empty())
        .ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "bool.must must be a non-empty array",
            )
        })?;

    let clauses = must
        .iter()
        .map(parse_count_query)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CountQuery::BoolMust(clauses))
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

fn parse_term_value(value: &Value) -> Result<String, OpenSearchError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Number(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Object(object) => object
            .get("value")
            .ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "term field query object must contain `value`",
                )
            })
            .and_then(parse_term_value),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "term query value must be a scalar value",
        )),
    }
}

fn query_matches(query: &CountQuery, source: &Value) -> bool {
    match query {
        CountQuery::MatchAll => true,
        CountQuery::Term { field, value } => term_field_matches(source, field, value),
        CountQuery::BoolMust(clauses) => clauses.iter().all(|clause| query_matches(clause, source)),
        CountQuery::Range { field, bounds } => range_field_matches(source, field, bounds),
        CountQuery::Exists { field } => exists_field_matches(source, field),
        CountQuery::Terms { field, values } => values
            .iter()
            .any(|value| term_field_matches(source, field, value)),
        CountQuery::Prefix { field, value } => prefix_field_matches(source, field, value),
        CountQuery::Wildcard { field, pattern } => wildcard_field_matches(source, field, pattern),
        CountQuery::MultiMatch {
            query,
            fields,
            operator,
        } => multi_match_matches(source, fields, query, *operator),
    }
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

fn field_text(source: &Value, field: &str) -> Option<String> {
    match source.get(field)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
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
