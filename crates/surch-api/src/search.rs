use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;
use surch_index::mapping::IndexMapping;
use surch_search::fuzzy::{
    bounded_damerau_levenshtein, edits_for_term_len, parse_fuzziness, Fuzziness,
};
use surch_search::scoring::{bm25_score, Bm25Config};

use crate::{
    index::validate_index_name,
    state::{AppState, FieldScoringStats, StoredDocument, TermScoringStats},
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
    pub highlight: Option<HighlightRequest>,
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

/// OpenSearch-compatible highlight request subset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightRequest {
    pub fields: Vec<String>,
    pub pre_tag: String,
    pub post_tag: String,
    pub fragment_size: Option<usize>,
    pub number_of_fragments: Option<usize>,
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
        operator: MatchOperator,
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

const DEFAULT_HIGHLIGHT_FRAGMENT_SIZE: usize = 100;
const DEFAULT_HIGHLIGHT_FRAGMENT_COUNT: usize = 5;
const MAX_HIGHLIGHT_FRAGMENT_SIZE: usize = 50_000;
const MAX_HIGHLIGHT_FRAGMENT_COUNT: usize = 1_000;

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
    state.documents_by_ids(index, ids)
}

fn posting_candidate_ids(
    state: &AppState,
    index: &str,
    query: &SearchQuery,
) -> Option<BTreeSet<String>> {
    match query {
        SearchQuery::Term { field, value } => Some(
            state
                .documents_for_term(index, field, value)
                .into_iter()
                .collect(),
        ),
        SearchQuery::Match {
            field,
            value,
            operator,
        } => Some(
            state
                .documents_for_match(index, field, value, *operator == MatchOperator::And)
                .into_iter()
                .collect(),
        ),
        SearchQuery::MultiMatch {
            query,
            fields,
            operator,
        } => {
            let mut matches = BTreeSet::new();
            for field in fields {
                matches.extend(state.documents_for_match(
                    index,
                    field,
                    query,
                    *operator == MatchOperator::And,
                ));
            }
            Some(matches)
        }
        SearchQuery::BoolMust(queries) => {
            let mut matches: Option<BTreeSet<String>> = None;
            let mut used_postings = false;
            for query in queries {
                let Some(current) = posting_candidate_ids(state, index, query) else {
                    continue;
                };
                used_postings = true;
                matches = Some(match matches {
                    Some(previous) => previous.intersection(&current).cloned().collect(),
                    None => current,
                });
                if matches.as_ref().is_some_and(BTreeSet::is_empty) {
                    break;
                }
            }
            if used_postings {
                matches
            } else {
                None
            }
        }
        _ => None,
    }
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
    let scoring_enabled = request.query.as_ref().is_some_and(is_scoring_query);

    if let Some(response) = run_topk_search(state, indices, request, scoring_enabled, started_at) {
        return response;
    }

    let mut matched_documents: Vec<ScoredDocument> = Vec::new();
    for index in indices {
        matched_documents.extend(match_documents_for_index(
            state,
            index,
            request.query.as_ref(),
        ));
    }
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

fn run_topk_search(
    state: &AppState,
    indices: &[String],
    request: &SearchRequest,
    scoring_enabled: bool,
    started_at: Instant,
) -> Option<SearchResponse> {
    if !scoring_enabled || indices.len() != 1 || !is_default_score_sort(&request.sort) {
        return None;
    }
    if request.highlight.is_some() {
        return None;
    }
    let query = request.query.as_ref()?;
    if !matches!(
        query,
        SearchQuery::Match { .. } | SearchQuery::MultiMatch { .. }
    ) {
        return None;
    }

    let from = usize::try_from(request.from.unwrap_or(0)).unwrap_or(usize::MAX);
    let size = usize::try_from(request.size.unwrap_or(10)).unwrap_or(usize::MAX);
    let limit = from.saturating_add(size);

    let (scored, total) = topk_scored_documents(state, &indices[0], query, limit)?;

    let max_score = compute_max_score(&scored, scoring_enabled);
    let hits = scored
        .iter()
        .skip(from)
        .take(size)
        .map(|doc| {
            build_hit(
                &doc.doc,
                request.source.as_ref(),
                scoring_enabled.then_some(doc.score),
                request.highlight.as_ref(),
                request.query.as_ref(),
            )
        })
        .collect::<Vec<_>>();
    let total_summary = resolve_total_hits(total, request.track_total_hits.as_ref());

    let mut response = build_search_response_with_total(
        hits,
        total_summary,
        started_at.elapsed().as_millis() as u64,
    );
    response.hits.max_score = max_score;
    Some(response)
}

fn is_default_score_sort(sort: &[SortClause]) -> bool {
    sort.is_empty()
        || (sort.len() == 1 && sort[0].field == "_score" && sort[0].order == SortOrder::Desc)
}

fn topk_scored_documents(
    state: &AppState,
    index: &str,
    query: &SearchQuery,
    limit: usize,
) -> Option<(Vec<ScoredDocument>, u64)> {
    let candidates: Vec<u32> = match query {
        SearchQuery::Match {
            field,
            value,
            operator,
        } => state.documents_for_match_internal(
            index,
            field,
            value,
            *operator == MatchOperator::And,
        ),
        SearchQuery::MultiMatch {
            query,
            fields,
            operator,
        } => {
            let mut matches = BTreeSet::new();
            for field in fields {
                matches.extend(state.documents_for_match_internal(
                    index,
                    field,
                    query,
                    *operator == MatchOperator::And,
                ));
            }
            matches.into_iter().collect()
        }
        _ => return None,
    };

    let total = candidates.len() as u64;
    if limit == 0 || candidates.is_empty() {
        return Some((Vec::new(), total));
    }

    let scoring_context = SearchScoringContext::new(state, index, query);

    let mut scored: Vec<(f64, u32)> = candidates
        .iter()
        .map(|internal_id| {
            let score = score_for_query(query, *internal_id, &scoring_context);
            (score, *internal_id)
        })
        .collect();

    let cmp = |a: &(f64, u32), b: &(f64, u32)| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    };

    let k = limit.min(scored.len());
    if k < scored.len() {
        scored.select_nth_unstable_by(k - 1, cmp);
        scored.truncate(k);
    }
    scored.sort_by(cmp);

    let winner_ids: Vec<u32> = scored.iter().map(|(_, id)| *id).collect();
    let hydrated = state.documents_by_internal_ids(index, &winner_ids);

    let result = scored
        .into_iter()
        .zip(hydrated)
        .map(|((score, _), doc)| ScoredDocument { doc, score })
        .collect();

    Some((result, total))
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
    let mapping = state.index_mapping(index).unwrap_or_default();
    let plain = match query {
        None => state.documents(index),
        Some(query) => match query {
            SearchQuery::Term { field, value } => documents_by_term(state, index, field, value),
            SearchQuery::BoolMust(_) => {
                if let Some(ids) = posting_candidate_ids(state, index, query) {
                    let ids = ids.into_iter().collect::<Vec<_>>();
                    documents_for_ids(state, index, &ids)
                        .into_iter()
                        .filter(|document| query_matches(query, &document.source, &mapping))
                        .collect()
                } else {
                    state
                        .documents(index)
                        .into_iter()
                        .filter(|document| query_matches(query, &document.source, &mapping))
                        .collect()
                }
            }
            SearchQuery::Match { .. } | SearchQuery::MultiMatch { .. } => {
                if let Some(ids) = posting_candidate_ids(state, index, query) {
                    let ids = ids.into_iter().collect::<Vec<_>>();
                    documents_for_ids(state, index, &ids)
                } else {
                    state
                        .documents(index)
                        .into_iter()
                        .filter(|document| query_matches(query, &document.source, &mapping))
                        .collect()
                }
            }
            SearchQuery::MatchAll => state.documents(index),
            _ => state
                .documents(index)
                .into_iter()
                .filter(|document| query_matches(query, &document.source, &mapping))
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

    let scoring_context = SearchScoringContext::new(state, index, query);
    let public_ids = documents.iter().map(|doc| doc.id.as_str()).collect::<Vec<_>>();
    let internal_ids = state.internal_doc_ids(index, &public_ids);

    documents
        .into_iter()
        .zip(internal_ids)
        .map(|(doc, internal_id)| {
            let score = internal_id
                .map(|id| score_for_query(query, id, &scoring_context))
                .unwrap_or(1.0);
            ScoredDocument { doc, score }
        })
        .collect()
}

fn score_for_query(
    query: &SearchQuery,
    internal_doc_id: u32,
    scoring_context: &SearchScoringContext,
) -> f64 {
    match query {
        SearchQuery::Match { field, value, .. } => {
            bm25_field_score(scoring_context, field, value, internal_doc_id).unwrap_or(1.0)
        }
        SearchQuery::MultiMatch { query, fields, .. } => fields
            .iter()
            .map(|field| {
                bm25_field_score(scoring_context, field, query, internal_doc_id).unwrap_or(0.0)
            })
            .fold(0.0_f64, f64::max)
            .max(1.0 / 1e9_f64.max(1.0)),
        SearchQuery::MatchPhrase { field, value } => {
            bm25_field_score(scoring_context, field, value, internal_doc_id).unwrap_or(1.0)
        }
        SearchQuery::Fuzzy { field, value, .. } => {
            bm25_field_score(scoring_context, field, value, internal_doc_id).unwrap_or(1.0)
        }
        SearchQuery::BoolMust(clauses) => clauses
            .iter()
            .map(|clause| score_for_query(clause, internal_doc_id, scoring_context))
            .sum(),
        _ => 1.0,
    }
}

#[derive(Debug, Default)]
struct SearchScoringContext {
    mapping: IndexMapping,
    field_stats_by_field: BTreeMap<String, FieldScoringStats>,
    term_stats_by_field: BTreeMap<String, BTreeMap<String, TermScoringStats>>,
}

impl SearchScoringContext {
    fn new(state: &AppState, index: &str, query: &SearchQuery) -> Self {
        let mapping = state.index_mapping(index).unwrap_or_default();
        let mut field_tokens = BTreeMap::<String, BTreeSet<String>>::new();
        collect_scoring_field_tokens(query, &mapping, &mut field_tokens);
        if field_tokens.is_empty() {
            return Self {
                mapping,
                ..Self::default()
            };
        }

        let mut field_stats_by_field = BTreeMap::new();
        for field in field_tokens.keys() {
            if let Some(stats) = state.field_scoring_stats(index, field) {
                field_stats_by_field.insert(field.clone(), stats);
            }
        }

        let mut term_stats_by_field = BTreeMap::<String, BTreeMap<String, TermScoringStats>>::new();
        for (field, tokens) in field_tokens {
            let token_stats = term_stats_by_field.entry(field.clone()).or_default();
            for token in tokens {
                let stats = state.term_scoring_stats(index, &field, &token);
                token_stats.insert(token, stats);
            }
        }

        Self {
            mapping,
            field_stats_by_field,
            term_stats_by_field,
        }
    }

    fn field_stats(&self, field: &str) -> Option<&FieldScoringStats> {
        self.field_stats_by_field.get(field)
    }

    fn term_stats(&self, field: &str, token: &str) -> Option<&TermScoringStats> {
        self.term_stats_by_field
            .get(field)
            .and_then(|tokens| tokens.get(token))
    }
}

fn collect_scoring_field_tokens(
    query: &SearchQuery,
    mapping: &IndexMapping,
    field_tokens: &mut BTreeMap<String, BTreeSet<String>>,
) {
    match query {
        SearchQuery::Match { field, value, .. }
        | SearchQuery::MatchPhrase { field, value }
        | SearchQuery::Fuzzy { field, value, .. } => {
            insert_scoring_tokens(field_tokens, mapping, field, value);
        }
        SearchQuery::MultiMatch { query, fields, .. } => {
            for field in fields {
                insert_scoring_tokens(field_tokens, mapping, field, query);
            }
        }
        SearchQuery::BoolMust(clauses) => {
            for clause in clauses {
                collect_scoring_field_tokens(clause, mapping, field_tokens);
            }
        }
        _ => {}
    }
}

fn insert_scoring_tokens(
    field_tokens: &mut BTreeMap<String, BTreeSet<String>>,
    mapping: &IndexMapping,
    field: &str,
    query: &str,
) {
    let tokens = mapping.analyzer(field).terms(query);
    if tokens.is_empty() {
        return;
    }
    field_tokens
        .entry(field.to_owned())
        .or_default()
        .extend(tokens);
}

fn bm25_field_score(
    scoring_context: &SearchScoringContext,
    field: &str,
    query: &str,
    internal_doc_id: u32,
) -> Option<f64> {
    let query_tokens = scoring_context.mapping.analyzer(field).terms(query);
    let field_stats = scoring_context.field_stats(field)?;
    if query_tokens.is_empty() || field_stats.doc_count == 0 {
        return None;
    }

    let doc_len = if field_stats.norms_enabled {
        field_stats.doc_len_by_doc_id.get(&internal_doc_id).copied()?
    } else {
        1
    };
    let avg_doc_len = field_stats.avg_doc_len;

    let config = Bm25Config::default();
    let mut total = 0.0_f64;
    for query_token in &query_tokens {
        let term_stats = scoring_context.term_stats(field, query_token)?;
        let term_freq = term_stats
            .term_freq_by_doc_id
            .get(&internal_doc_id)
            .copied()
            .unwrap_or(0);
        if term_freq == 0 {
            continue;
        }
        let doc_freq = term_stats.doc_freq;
        if doc_freq == 0 || doc_freq > field_stats.doc_count {
            continue;
        }
        if let Ok(score) = bm25_score(
            config,
            field_stats.doc_count,
            doc_freq,
            term_freq,
            doc_len,
            avg_doc_len,
        ) {
            total += score;
        }
    }
    if total > 0.0 {
        Some(total)
    } else {
        None
    }
}

fn field_tokens_for_source(source: &Value, field: &str, mapping: &IndexMapping) -> Vec<String> {
    field_text(source, field)
        .map(|text| mapping.analyzer(field).terms(&text))
        .unwrap_or_default()
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
            highlight: None,
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
    let highlight = object.get("highlight").map(parse_highlight).transpose()?;

    Ok(SearchRequest {
        query,
        from,
        size,
        source,
        track_total_hits,
        sort,
        highlight,
    })
}

fn parse_highlight(value: &Value) -> Result<HighlightRequest, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`highlight` must be an object",
        )
    })?;

    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "fields" | "pre_tags" | "post_tags" | "fragment_size" | "number_of_fragments"
        ) {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("unsupported `highlight` field `{key}`"),
            ));
        }
    }

    let fields_value = object.get("fields").ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`highlight` must contain `fields`",
        )
    })?;
    let fields_object = fields_value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`highlight.fields` must be an object",
        )
    })?;
    if fields_object.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`highlight.fields` must not be empty",
        ));
    }

    let mut fields = Vec::with_capacity(fields_object.len());
    for (field, config) in fields_object {
        if field.is_empty() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "`highlight.fields` entries must not be empty",
            ));
        }
        if !config.is_object() && !config.is_null() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("`highlight.fields.{field}` must be an object or null"),
            ));
        }
        fields.push(field.clone());
    }

    let pre_tag = object
        .get("pre_tags")
        .map(|value| parse_highlight_tag(value, "pre_tags"))
        .transpose()?
        .unwrap_or_else(|| "<em>".to_owned());
    let post_tag = object
        .get("post_tags")
        .map(|value| parse_highlight_tag(value, "post_tags"))
        .transpose()?
        .unwrap_or_else(|| "</em>".to_owned());
    let fragment_size = object
        .get("fragment_size")
        .map(|value| {
            parse_highlight_positive_integer(value, "fragment_size", MAX_HIGHLIGHT_FRAGMENT_SIZE)
        })
        .transpose()?;
    let number_of_fragments = object
        .get("number_of_fragments")
        .map(|value| {
            parse_highlight_positive_integer(
                value,
                "number_of_fragments",
                MAX_HIGHLIGHT_FRAGMENT_COUNT,
            )
        })
        .transpose()?;

    Ok(HighlightRequest {
        fields,
        pre_tag,
        post_tag,
        fragment_size,
        number_of_fragments,
    })
}

fn parse_highlight_tag(value: &Value, field: &str) -> Result<String, OpenSearchError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Array(items) if !items.is_empty() => {
            let mut tags = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(text) => tags.push(text.clone()),
                    _ => {
                        return Err(OpenSearchError::new(
                            StatusCode::BAD_REQUEST,
                            "parsing_exception",
                            format!("`highlight.{field}` entries must be strings"),
                        ));
                    }
                }
            }
            Ok(tags[0].clone())
        }
        Value::Array(_) => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`highlight.{field}` must not be empty"),
        )),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`highlight.{field}` must be a string or array of strings"),
        )),
    }
}

fn parse_highlight_positive_integer(
    value: &Value,
    field: &str,
    max_value: usize,
) -> Result<usize, OpenSearchError> {
    let value = value.as_u64().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`highlight.{field}` must be a positive integer"),
        )
    })?;
    if value == 0 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`highlight.{field}` must be greater than zero"),
        ));
    }
    if value > max_value as u64 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`highlight.{field}` must not exceed {max_value}"),
        ));
    }
    usize::try_from(value).map_err(|_| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`highlight.{field}` must be a positive integer"),
        )
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

/// Parse a `multi_match` query body and return `(query_text, fields, operator)`.
pub fn parse_multi_match_clause(
    value: &Value,
) -> Result<(String, Vec<String>, MatchOperator), OpenSearchError> {
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
    let operator = parse_match_operator(value, "multi_match")?;

    Ok((query_text, fields, operator))
}

fn parse_multi_match_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (query, fields, operator) = parse_multi_match_clause(value)?;
    Ok(SearchQuery::MultiMatch {
        query,
        fields,
        operator,
    })
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

fn query_matches(query: &SearchQuery, source: &Value, mapping: &IndexMapping) -> bool {
    match query {
        SearchQuery::MatchAll => true,
        SearchQuery::Match {
            field,
            value,
            operator,
        } => field_matches_with_mapping(source, field, value, *operator, mapping),
        SearchQuery::MatchPhrase { field, value } => {
            match_phrase_field_matches_with_mapping(source, field, value, mapping)
        }
        SearchQuery::Term { field, value } => {
            term_field_matches_with_mapping(source, field, value, mapping)
        }
        SearchQuery::BoolMust(queries) => queries
            .iter()
            .all(|query| query_matches(query, source, mapping)),
        SearchQuery::Fuzzy {
            field,
            value,
            fuzziness,
        } => fuzzy_field_matches(source, field, value, *fuzziness),
        SearchQuery::Range { field, bounds } => range_field_matches(source, field, bounds),
        SearchQuery::Exists { field } => exists_field_matches(source, field),
        SearchQuery::Terms { field, values } => values
            .iter()
            .any(|value| term_field_matches_with_mapping(source, field, value, mapping)),
        SearchQuery::Prefix { field, value } => prefix_field_matches(source, field, value),
        SearchQuery::Wildcard { field, pattern } => wildcard_field_matches(source, field, pattern),
        SearchQuery::MultiMatch {
            query,
            fields,
            operator,
        } => multi_match_matches_with_mapping(source, fields, query, *operator, mapping),
    }
}

fn field_matches_with_mapping(
    source: &Value,
    field: &str,
    query: &str,
    operator: MatchOperator,
    mapping: &IndexMapping,
) -> bool {
    let query_tokens = mapping.analyzer(field).terms(query);
    if query_tokens.is_empty() {
        return false;
    }
    let field_tokens = field_tokens_for_source(source, field, mapping);
    if field_tokens.is_empty() {
        return false;
    }
    tokens_match(&query_tokens, &field_tokens, operator)
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
    tokens_match(&query_tokens, &field_tokens, operator)
}

fn tokens_match(query_tokens: &[String], field_tokens: &[String], operator: MatchOperator) -> bool {
    match operator {
        MatchOperator::Or => query_tokens
            .iter()
            .any(|token| field_tokens.iter().any(|field_token| field_token == token)),
        MatchOperator::And => query_tokens
            .iter()
            .all(|token| field_tokens.iter().any(|field_token| field_token == token)),
    }
}

fn match_phrase_field_matches_with_mapping(
    source: &Value,
    field: &str,
    query: &str,
    mapping: &IndexMapping,
) -> bool {
    let query_tokens = mapping.analyzer(field).terms(query);
    if query_tokens.is_empty() {
        return false;
    }

    let field_tokens = field_tokens_for_source(source, field, mapping);
    field_tokens
        .windows(query_tokens.len())
        .any(|field_window| field_window == query_tokens.as_slice())
}

fn term_field_matches_with_mapping(
    source: &Value,
    field: &str,
    query: &str,
    mapping: &IndexMapping,
) -> bool {
    let query = mapping.analyzer(field).first_term(query);
    if query.is_empty() {
        return false;
    }

    field_tokens_for_source(source, field, mapping)
        .iter()
        .any(|field_token| field_token == &query)
}

pub fn multi_match_matches(
    source: &Value,
    fields: &[String],
    query: &str,
    operator: MatchOperator,
) -> bool {
    fields
        .iter()
        .any(|field| field_matches(source, field, query, operator))
}

fn multi_match_matches_with_mapping(
    source: &Value,
    fields: &[String],
    query: &str,
    operator: MatchOperator,
    mapping: &IndexMapping,
) -> bool {
    fields
        .iter()
        .any(|field| field_matches_with_mapping(source, field, query, operator, mapping))
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
            let ordering = compare_sort_clause(left, right, clause);
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        }
        std::cmp::Ordering::Equal
    });
}

fn compare_sort_clause(
    left: &ScoredDocument,
    right: &ScoredDocument,
    clause: &SortClause,
) -> std::cmp::Ordering {
    if clause.field == "_score" {
        return compare_score(left.score, right.score, clause.order);
    }
    compare_field(
        left.doc.source.get(&clause.field),
        right.doc.source.get(&clause.field),
        clause.order,
    )
}

fn compare_score(left: f64, right: f64, order: SortOrder) -> std::cmp::Ordering {
    let base = left
        .partial_cmp(&right)
        .unwrap_or(std::cmp::Ordering::Equal);
    match order {
        SortOrder::Asc => base,
        SortOrder::Desc => base.reverse(),
    }
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
                request.highlight.as_ref(),
                request.query.as_ref(),
            )
        })
        .collect()
}

fn build_hit(
    document: &StoredDocument,
    filter: Option<&SourceFilter>,
    score: Option<f64>,
    highlight: Option<&HighlightRequest>,
    query: Option<&SearchQuery>,
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
    if let Some(highlight) =
        highlight.and_then(|request| build_highlight(&document.source, query, request))
    {
        object.insert("highlight".to_owned(), highlight);
    }
    hit
}

fn build_highlight(
    source: &Value,
    query: Option<&SearchQuery>,
    request: &HighlightRequest,
) -> Option<Value> {
    let query = query?;
    let mut highlighted_fields = serde_json::Map::new();

    for field in &request.fields {
        let Some(text) = field_text(source, field) else {
            continue;
        };
        let Some(fragment) = highlight_field_fragments(&text, query, field, request) else {
            continue;
        };
        highlighted_fields.insert(field.clone(), json!(fragment));
    }

    if highlighted_fields.is_empty() {
        None
    } else {
        Some(Value::Object(highlighted_fields))
    }
}

fn highlight_field_fragments(
    text: &str,
    query: &SearchQuery,
    field: &str,
    request: &HighlightRequest,
) -> Option<Vec<String>> {
    let spans = highlight_spans_for_query(text, query, field);
    if spans.is_empty() {
        return None;
    }

    if request.fragment_size.is_none() && request.number_of_fragments.is_none() {
        return Some(vec![render_highlight_fragment(
            text,
            &spans,
            &request.pre_tag,
            &request.post_tag,
        )]);
    }

    let fragment_size = request
        .fragment_size
        .unwrap_or(DEFAULT_HIGHLIGHT_FRAGMENT_SIZE);
    let number_of_fragments = request
        .number_of_fragments
        .unwrap_or(DEFAULT_HIGHLIGHT_FRAGMENT_COUNT);

    Some(build_highlight_fragments(
        text,
        &spans,
        fragment_size,
        number_of_fragments,
        &request.pre_tag,
        &request.post_tag,
    ))
}

fn build_highlight_fragments(
    text: &str,
    spans: &[(usize, usize)],
    fragment_size: usize,
    max_fragments: usize,
    pre_tag: &str,
    post_tag: &str,
) -> Vec<String> {
    let mut fragments = Vec::new();
    if text.is_empty() || spans.is_empty() || max_fragments == 0 {
        return fragments;
    }

    let mut current_cluster: Vec<(usize, usize)> = Vec::new();
    let mut cluster_start = 0usize;
    let mut cluster_end = 0usize;

    let flush_cluster = |cluster: &mut Vec<(usize, usize)>,
                         start: &mut usize,
                         end: &mut usize,
                         out: &mut Vec<String>| {
        if cluster.is_empty() {
            return;
        }
        let fragment = render_cluster_fragment(
            text,
            cluster,
            *start,
            *end,
            fragment_size,
            pre_tag,
            post_tag,
        );
        out.push(fragment);
        cluster.clear();
        *start = 0;
        *end = 0;
    };

    for &(span_start, span_end) in spans {
        if current_cluster.is_empty() {
            current_cluster.push((span_start, span_end));
            cluster_start = span_start;
            cluster_end = span_end;
            continue;
        }

        if span_start <= cluster_end || span_end.saturating_sub(cluster_start) <= fragment_size {
            current_cluster.push((span_start, span_end));
            cluster_end = cluster_end.max(span_end);
            continue;
        }

        flush_cluster(
            &mut current_cluster,
            &mut cluster_start,
            &mut cluster_end,
            &mut fragments,
        );
        if fragments.len() >= max_fragments {
            return fragments;
        }

        current_cluster.push((span_start, span_end));
        cluster_start = span_start;
        cluster_end = span_end;
    }

    if !current_cluster.is_empty() && fragments.len() < max_fragments {
        flush_cluster(
            &mut current_cluster,
            &mut cluster_start,
            &mut cluster_end,
            &mut fragments,
        );
    }

    fragments.into_iter().take(max_fragments).collect()
}

fn render_cluster_fragment(
    text: &str,
    spans: &[(usize, usize)],
    cluster_start: usize,
    cluster_end: usize,
    fragment_size: usize,
    pre_tag: &str,
    post_tag: &str,
) -> String {
    let span_center = (cluster_start + cluster_end) / 2;
    let half_window = fragment_size / 2;
    let mut fragment_start = span_center.saturating_sub(half_window);
    let fragment_end = (fragment_start + fragment_size).min(text.len());
    if fragment_end == text.len() {
        fragment_start = text.len().saturating_sub(fragment_size);
    }
    let fragment_start = previous_char_boundary(text, fragment_start);
    let fragment_end = next_char_boundary(text, fragment_end);

    let mut fragment_spans = Vec::with_capacity(spans.len());
    for &(span_start, span_end) in spans {
        let clipped_start = span_start.max(fragment_start);
        let clipped_end = span_end.min(fragment_end);
        if clipped_start < clipped_end {
            fragment_spans.push((clipped_start, clipped_end));
        }
    }
    if fragment_spans.is_empty() {
        fragment_spans.push((fragment_start, fragment_start));
    }

    render_highlight_region(
        text,
        fragment_start,
        fragment_end,
        &fragment_spans,
        pre_tag,
        post_tag,
    )
}

fn previous_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn next_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn render_highlight_region(
    text: &str,
    fragment_start: usize,
    fragment_end: usize,
    spans: &[(usize, usize)],
    pre_tag: &str,
    post_tag: &str,
) -> String {
    if fragment_end < fragment_start {
        return String::new();
    }
    let mut fragment = String::new();
    let mut cursor = fragment_start;
    for (start, end) in spans {
        if *start < cursor {
            continue;
        }
        fragment.push_str(&text[cursor..*start]);
        fragment.push_str(pre_tag);
        fragment.push_str(&text[*start..*end]);
        fragment.push_str(post_tag);
        cursor = *end;
    }
    fragment.push_str(&text[cursor..fragment_end]);
    fragment
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextTokenSpan {
    start: usize,
    end: usize,
    normalized: String,
}

fn highlight_spans_for_query(text: &str, query: &SearchQuery, field: &str) -> Vec<(usize, usize)> {
    let tokens = text_token_spans(text);
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut spans = match query {
        SearchQuery::Match {
            field: query_field,
            value,
            ..
        } if query_field == field => exact_token_spans(&tokens, &tokenize_for_search(value)),
        SearchQuery::MatchPhrase {
            field: query_field,
            value,
        } if query_field == field => phrase_token_spans(&tokens, &tokenize_for_search(value)),
        SearchQuery::Term {
            field: query_field,
            value,
        } if query_field == field => exact_token_spans(&tokens, &[normalize_text(value)]),
        SearchQuery::Terms {
            field: query_field,
            values,
        } if query_field == field => {
            let query_tokens = values
                .iter()
                .map(|value| normalize_text(value))
                .collect::<Vec<_>>();
            exact_token_spans(&tokens, &query_tokens)
        }
        SearchQuery::Fuzzy {
            field: query_field,
            value,
            fuzziness,
        } if query_field == field => {
            fuzzy_token_spans(&tokens, &tokenize_for_search(value), *fuzziness)
        }
        SearchQuery::Prefix {
            field: query_field,
            value,
        } if query_field == field => prefix_token_spans(&tokens, value),
        SearchQuery::Wildcard {
            field: query_field,
            pattern,
        } if query_field == field => wildcard_token_spans(&tokens, pattern),
        SearchQuery::MultiMatch { query, fields, .. }
            if fields.iter().any(|candidate| candidate == field) =>
        {
            exact_token_spans(&tokens, &tokenize_for_search(query))
        }
        SearchQuery::BoolMust(clauses) => clauses
            .iter()
            .flat_map(|clause| highlight_spans_for_query(text, clause, field))
            .collect(),
        _ => Vec::new(),
    };

    spans.sort_unstable();
    spans.dedup();
    spans
}

fn text_token_spans(text: &str) -> Vec<TextTokenSpan> {
    let mut spans = Vec::new();
    let mut start = None;
    let mut normalized = String::new();

    for (index, character) in text.char_indices() {
        let folded = normalized_token_piece(character);
        if folded.is_empty() {
            if let Some(span_start) = start.take() {
                spans.push(TextTokenSpan {
                    start: span_start,
                    end: index,
                    normalized: std::mem::take(&mut normalized),
                });
            }
            continue;
        }

        if start.is_none() {
            start = Some(index);
        }
        normalized.push_str(&folded);
    }

    if let Some(span_start) = start {
        spans.push(TextTokenSpan {
            start: span_start,
            end: text.len(),
            normalized,
        });
    }

    spans
}

fn normalized_token_piece(character: char) -> String {
    character
        .to_lowercase()
        .map(fold_search_char)
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn exact_token_spans(tokens: &[TextTokenSpan], query_tokens: &[String]) -> Vec<(usize, usize)> {
    let wanted = query_tokens
        .iter()
        .filter(|token| !token.is_empty())
        .collect::<BTreeSet<_>>();
    if wanted.is_empty() {
        return Vec::new();
    }

    tokens
        .iter()
        .filter(|token| wanted.contains(&token.normalized))
        .map(|token| (token.start, token.end))
        .collect()
}

fn phrase_token_spans(tokens: &[TextTokenSpan], query_tokens: &[String]) -> Vec<(usize, usize)> {
    if query_tokens.is_empty() || query_tokens.len() > tokens.len() {
        return Vec::new();
    }

    let mut spans = Vec::new();
    for window in tokens.windows(query_tokens.len()) {
        if window
            .iter()
            .map(|token| token.normalized.as_str())
            .eq(query_tokens.iter().map(String::as_str))
        {
            spans.extend(window.iter().map(|token| (token.start, token.end)));
        }
    }
    spans
}

fn fuzzy_token_spans(
    tokens: &[TextTokenSpan],
    query_tokens: &[String],
    fuzziness: Fuzziness,
) -> Vec<(usize, usize)> {
    if query_tokens.is_empty() {
        return Vec::new();
    }

    tokens
        .iter()
        .filter(|token| {
            query_tokens.iter().any(|query_token| {
                !query_token.is_empty()
                    && fuzzy_token_matches(query_token, &token.normalized, fuzziness)
            })
        })
        .map(|token| (token.start, token.end))
        .collect()
}

fn prefix_token_spans(tokens: &[TextTokenSpan], prefix: &str) -> Vec<(usize, usize)> {
    let prefix = normalize_text(prefix);
    if prefix.is_empty() {
        return Vec::new();
    }

    tokens
        .iter()
        .filter(|token| token.normalized.starts_with(&prefix))
        .map(|token| (token.start, token.end))
        .collect()
}

fn wildcard_token_spans(tokens: &[TextTokenSpan], pattern: &str) -> Vec<(usize, usize)> {
    let pattern = normalize_wildcard_pattern(pattern);
    if pattern.is_empty() {
        return Vec::new();
    }
    let pattern_chars = pattern.chars().collect::<Vec<_>>();

    tokens
        .iter()
        .filter(|token| {
            let token_chars = token.normalized.chars().collect::<Vec<_>>();
            wildcard_pattern_matches(&pattern_chars, &token_chars)
        })
        .map(|token| (token.start, token.end))
        .collect()
}

fn render_highlight_fragment(
    text: &str,
    spans: &[(usize, usize)],
    pre_tag: &str,
    post_tag: &str,
) -> String {
    let mut fragment = String::new();
    let mut cursor = 0;

    for (start, end) in spans {
        if *start < cursor {
            continue;
        }
        fragment.push_str(&text[cursor..*start]);
        fragment.push_str(pre_tag);
        fragment.push_str(&text[*start..*end]);
        fragment.push_str(post_tag);
        cursor = *end;
    }
    fragment.push_str(&text[cursor..]);
    fragment
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
