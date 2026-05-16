use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;
use surch_index::mapping::IndexMapping;
use surch_search::fuzzy::{
    bounded_damerau_levenshtein, edits_for_term_len, parse_fuzziness, Fuzziness,
};
use surch_search::scoring::{bm25_score, Bm25Config};

use crate::{
    index::validate_index_name,
    scroll::{parse_scroll_ttl, ScrollContext},
    state::{AppState, FieldScoringStats, StoredDocument, TermScoringStats},
    topn::TopN,
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
    pub min_score: Option<f64>,
    /// A12.1: declared aggregations. Today only `terms` aggs are
    /// honoured; the parser rejects every other type with an explicit
    /// "phase 2" error so the wire shape stays diagnosable.
    pub aggs: BTreeMap<String, AggSpec>,
}

/// A12.1+A12.2+A12.3: aggregation specification subset supported by
/// Surch today. matchID's wire shape uses `terms`, `date_histogram`,
/// `composite` and `cardinality` (intake §2.10). `terms` ships under
/// A12.1, `date_histogram` under A12.2 and `cardinality` under A12.3;
/// `composite` + `after_key` are tracked under A12 phase 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AggSpec {
    /// `terms` aggregation: bucket documents by the distinct values of
    /// `field` and return the top `size` buckets ordered by descending
    /// `doc_count` (with ascending key as the deterministic tiebreak).
    Terms { field: String, size: usize },
    /// A12.2: `date_histogram` aggregation. matchID emits this against
    /// `DATE_NAISSANCE` / `DATE_NAISSANCE_NORM` keyword-encoded
    /// `YYYYMMDD` dates. Each matched doc is truncated according to
    /// `calendar_interval` (day → `YYYYMMDD`, month → `YYYYMM01`,
    /// year → `YYYY0101`, week → ISO week-monday `YYYYMMDD`) and
    /// counted into the bucket. Buckets are returned sorted by key
    /// ascending. `format` is forwarded to `key_as_string` when set.
    DateHistogram {
        field: String,
        calendar_interval: CalendarInterval,
        format: Option<String>,
    },
    /// A12.3: `cardinality` aggregation. Counts the number of distinct
    /// values of `field` over the matched documents. MVP: exact count
    /// (no HyperLogLog estimation); the wire shape is the ES-7.x
    /// `{ "value": N }` payload.
    Cardinality { field: String },
}

/// A12.2: calendar bucketing unit for `date_histogram`. matchID emits
/// `month` for the analytics tab and occasionally `day` / `year` for
/// the per-decade drill-down. `week` is included for parity with ES
/// 7.x (ISO week, Monday-anchored) even though deces-backend does not
/// emit it today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarInterval {
    Day,
    Week,
    Month,
    Year,
}

const DEFAULT_TERMS_AGG_SIZE: usize = 10;

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
    /// Matches every document. `boost` multiplies the constant
    /// `_score = 1.0`; OpenSearch defaults `boost` to `1.0` when the
    /// query body is `{}` and accepts `{ "boost": <number> }` to scale.
    MatchAll {
        boost: f64,
    },
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
    /// Compound `bool` query carrying every clause-bucket used by
    /// matchID's deces-backend (`must` + `filter` + `should`) plus
    /// `minimum_should_match` and a clause-level multiplicative `boost`.
    ///
    /// Semantics (mirroring ES 7.x):
    /// - a document matches when **all** `must` and `filter` clauses match,
    ///   **and** at least `minimum_should_match` of the `should` clauses
    ///   match (if `should` is non-empty and `minimum_should_match > 0`),
    /// - `filter` clauses do **not** contribute to `_score`,
    /// - `must` and matching `should` clauses sum into `_score`,
    /// - the result `_score` is multiplied by `boost`.
    Bool {
        must: Vec<SearchQuery>,
        filter: Vec<SearchQuery>,
        should: Vec<SearchQuery>,
        minimum_should_match: u32,
        boost: f64,
    },
    /// `function_score` wrapper around an inner query. matchID's
    /// deces-backend uses this shape today as a no-op (empty
    /// `functions`) to keep its codebase ready for future custom
    /// scoring. Surch supports the wrapper, the top-level `boost`
    /// multiplier, and an empty `functions` array. Non-empty
    /// `functions`, `score_mode`, `boost_mode` and friends are parsed
    /// for forward-compat but produce an error today — tracked as
    /// "function_score scoring phase 2" in `docs/wp-d-matchid/`.
    FunctionScore {
        inner: Box<SearchQuery>,
        boost: f64,
    },
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
    /// `geo_distance` filter (matchID intake §2.6). Matches documents
    /// whose `field` (a `geo_point`) is within `distance_meters` of the
    /// `(lat, lon)` target, measured with the haversine formula. The
    /// clause is non-scoring (constant `_score = 1.0`) — matchID always
    /// uses it inside `bool.filter`.
    GeoDistance {
        field: String,
        lat: f64,
        lon: f64,
        distance_meters: f64,
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
    /// Opaque cursor for `POST /_search/scroll`. Only present when the
    /// caller passed `?scroll=…` on the initial `_search` request and on
    /// each non-empty page returned by `scroll_handler`.
    #[serde(rename = "_scroll_id", skip_serializing_if = "Option::is_none")]
    pub scroll_id: Option<String>,
    /// A12.1: per-aggregation result, keyed by the aggregation name
    /// supplied in the request. Omitted when the request carries no
    /// `aggs` (or `aggregations`) block, so existing zero-agg callers
    /// continue to see the same response shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregations: Option<BTreeMap<String, AggResult>>,
}

/// A12.1+A12.2+A12.3: shape of the per-aggregation payload. `terms`
/// and `date_histogram` emit `{ buckets: [...] }`; `cardinality`
/// emits `{ value: N }`. Serialized untagged so the JSON envelope
/// stays ES-7.x identical.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AggResult {
    Buckets { buckets: Vec<AggBucket> },
    Value { value: Value },
}

/// A12.1+A12.2: ES-7.x compatible bucket payload. `terms` emits
/// `{ "key": …, "doc_count": N }`; `date_histogram` additionally
/// emits `{ "key_as_string": "…" }` when the agg body declared a
/// `format`. `key` carries the verbatim JSON type so numeric /
/// string keys round-trip through the response unchanged.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AggBucket {
    pub key: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_as_string: Option<String>,
    pub doc_count: u64,
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
        scroll_id: None,
        aggregations: None,
    }
}

/// OpenSearch 7+ default cap: a `_search` request that does not specify
/// `track_total_hits` reports an exact total only up to this number of
/// hits, otherwise it returns `relation = "gte"`.
const DEFAULT_TRACK_TOTAL_HITS_CAP: u64 = 10_000;

/// Mirrors ES 7.x `index.max_result_window` default: `from + size` must not
/// exceed this value or the search fails fast with HTTP 400.
const MAX_RESULT_WINDOW: u64 = 10_000;

/// Resolve the OpenSearch `hits.total` field shape from a `track_total_hits` mode.
pub fn resolve_total_hits(total: u64, mode: Option<&TrackTotalHits>) -> Option<SearchHitsTotal> {
    match mode {
        None => {
            if total <= DEFAULT_TRACK_TOTAL_HITS_CAP {
                Some(SearchHitsTotal {
                    value: total,
                    relation: "eq",
                })
            } else {
                Some(SearchHitsTotal {
                    value: DEFAULT_TRACK_TOTAL_HITS_CAP,
                    relation: "gte",
                })
            }
        }
        Some(TrackTotalHits::Exact) => Some(SearchHitsTotal {
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
        // A6 phase 2: `index_prefixes` write-time prefix postings turn a
        // length-bounded `prefix` query into a direct lookup. Outside the
        // declared `[min_chars..=max_chars]` window (or absent mapping)
        // we return `None` so the candidate-set path falls back to the
        // full-scan `query_matches` (which still uses `prefix_field_matches`).
        SearchQuery::Prefix { field, value } => state
            .documents_for_prefix(index, field, value)
            .map(|ids| ids.into_iter().collect()),
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
        SearchQuery::Bool {
            must,
            filter,
            should,
            minimum_should_match,
            ..
        } => {
            // Pre-compute candidate sets for every postings-backed clause and
            // intersect in ascending-size order. Starting with the smallest
            // set keeps the running intersection small and turns
            // `BTreeSet::intersection` (O(N + M)) into roughly O(K * size_min)
            // instead of O(K * size_first_seen).
            //
            // `must` and `filter` both restrict the candidate space via
            // intersection (only their scoring contribution differs, which
            // is handled in `score_for_query`). `should` only restricts the
            // candidate space when `minimum_should_match >= 1`, in which
            // case the union of postings-backed `should` clauses limits the
            // candidate space.
            let mut sets: Vec<BTreeSet<String>> = must
                .iter()
                .chain(filter.iter())
                .filter_map(|q| posting_candidate_ids(state, index, q))
                .collect();
            let mut used_postings = !sets.is_empty();

            // `should` with MSM >= 1: union of postings-backed clauses
            // restricts the candidate pool. If any `should` clause cannot
            // be answered from postings we conservatively skip the union
            // contribution (must/filter still restricts; the final
            // `query_matches` pass enforces MSM).
            if !should.is_empty() && *minimum_should_match >= 1 {
                let union_sets: Vec<BTreeSet<String>> = should
                    .iter()
                    .filter_map(|q| posting_candidate_ids(state, index, q))
                    .collect();
                if union_sets.len() == should.len() {
                    let mut union = BTreeSet::new();
                    for s in union_sets {
                        union.extend(s);
                    }
                    sets.push(union);
                    used_postings = true;
                }
            }

            sets.sort_by_key(|s| s.len());
            let mut matches: Option<BTreeSet<String>> = None;
            for current in sets {
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
    Query(params): Query<HashMap<String, String>>,
    body: String,
) -> impl IntoResponse {
    let started_at = Instant::now();
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

    // `?scroll=1m` triggers the stateful scan path: skip the search
    // cache (each scroll session is logically unique), install a
    // ScrollContext, and decorate the response with `_scroll_id`.
    let scroll_keepalive = params.get("scroll").map(String::as_str);
    let cache_eligible = indices.len() == 1 && scroll_keepalive.is_none();
    let cache_key = if cache_eligible {
        Some(hash_search_body(&body))
    } else {
        None
    };

    if let (Some(key), Some(index)) = (cache_key, indices.first()) {
        if let Some(bytes) = state.search_cache_get(index, key) {
            metrics::counter!("surch_search_cache_hit_total").increment(1);
            let query_type = classify_search_body(&body);
            metrics::counter!(
                "surch_search_total",
                "index" => target.clone(),
                "query_type" => query_type,
            )
            .increment(1);
            metrics::histogram!("surch_search_duration_seconds")
                .record(started_at.elapsed().as_secs_f64());
            return axum::http::Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(bytes))
                .expect("cached response should build")
                .into_response();
        }
    }

    match parse_search_request(&body) {
        Ok(request) => {
            let query_type = classify_query(request.query.as_ref());
            let mut response = run_search(&state, &indices, &request);
            if let Some(keepalive) = scroll_keepalive {
                // Attach a `_scroll_id` so the client can paginate via
                // `POST /_search/scroll`. The cursor starts where the
                // initial page ended; total is reported back by
                // `hits.total.value` so we can stop when the cursor
                // catches up. Single-index only — matches ES behaviour.
                if let Some(index) = indices.first() {
                    let from = usize::try_from(request.from.unwrap_or(0)).unwrap_or(usize::MAX);
                    let size = usize::try_from(request.size.unwrap_or(10)).unwrap_or(usize::MAX);
                    let ttl = parse_scroll_ttl(keepalive);
                    let ctx = ScrollContext {
                        index: index.clone(),
                        request: request.clone(),
                        cursor: from.saturating_add(size),
                        size,
                        expires_at: Instant::now() + ttl,
                    };
                    let id = state.scroll_table.insert(ctx);
                    response.scroll_id = Some(id);
                }
            }
            let bytes = match serde_json::to_vec(&response) {
                Ok(bytes) => bytes,
                Err(_) => {
                    metrics::counter!(
                        "surch_search_total",
                        "index" => target.clone(),
                        "query_type" => query_type,
                    )
                    .increment(1);
                    metrics::histogram!("surch_search_duration_seconds")
                        .record(started_at.elapsed().as_secs_f64());
                    return (StatusCode::OK, Json(response)).into_response();
                }
            };
            if let (Some(key), Some(index)) = (cache_key, indices.first()) {
                // Cache the response with took=0 so cache hits report ~zero
                // server time and stay distinguishable from cold paths.
                let mut cached = response.clone();
                cached.took = 0;
                if let Ok(cache_bytes) = serde_json::to_vec(&cached) {
                    state.search_cache_put(index, key, cache_bytes);
                }
            }
            metrics::counter!(
                "surch_search_total",
                "index" => target.clone(),
                "query_type" => query_type,
            )
            .increment(1);
            metrics::histogram!("surch_search_duration_seconds")
                .record(started_at.elapsed().as_secs_f64());
            axum::http::Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(bytes))
                .expect("response should build")
                .into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Axum handler for `POST /_search/scroll`. Looks up a scroll context
/// by id, returns the next page (offset `cursor`, length `size`),
/// advances the cursor, and reinstalls the context unless the cursor
/// has caught up to the total. Unknown scroll ids return HTTP 404
/// with an OpenSearch-shaped error envelope.
pub async fn scroll_handler(
    State(state): State<AppState>,
    body: String,
) -> axum::response::Response {
    let started_at = Instant::now();
    let request = match parse_scroll_request(&body) {
        Ok(req) => req,
        Err(error) => return error.into_response(),
    };

    let Some(mut ctx) = state.scroll_table.take(&request.scroll_id) else {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "search_context_missing_exception",
            format!("No search context found for id [{}]", request.scroll_id),
        )
        .into_response();
    };

    // Build a one-shot SearchRequest at offset `cursor` and size
    // `ctx.size`. We deliberately keep every other knob from the
    // original request (query, source, sort, …) so the scrolled
    // pages stay consistent with the initial `_search`.
    let mut paged = ctx.request.clone();
    paged.from = Some(ctx.cursor as u64);
    paged.size = Some(ctx.size as u64);
    let indices = vec![ctx.index.clone()];
    let mut response = run_search(&state, &indices, &paged);

    // Advance the cursor by the number of hits actually returned (the
    // scoring path may filter via `min_score`, so `size` is an upper
    // bound, not an exact count).
    let returned = response.hits.hits.len();
    ctx.cursor = ctx.cursor.saturating_add(returned);

    // Re-install the context when we still have something to serve;
    // an empty page or one where `from + size` overshoots the total
    // ends the scroll without a re-install (matches ES — the next
    // `_search/scroll` then 404s like an unknown id).
    if returned > 0 {
        let ttl = parse_scroll_ttl(&request.scroll);
        ctx.expires_at = Instant::now() + ttl;
        let new_id = state.scroll_table.insert(ctx);
        response.scroll_id = Some(new_id);
    }
    // Always overwrite `took` so the scroll response measures the
    // scroll fetch, not the upstream `_search` walk.
    response.took = started_at.elapsed().as_millis() as u64;
    (StatusCode::OK, Json(response)).into_response()
}

/// Body of a `POST /_search/scroll` request.
#[derive(Clone, Debug)]
struct ScrollRequestBody {
    scroll: String,
    scroll_id: String,
}

fn parse_scroll_request(body: &str) -> Result<ScrollRequestBody, OpenSearchError> {
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
            "scroll request body must be an object",
        )
    })?;
    let scroll_id = object
        .get("scroll_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "scroll request requires `scroll_id`",
            )
        })?
        .to_string();
    let scroll = object
        .get("scroll")
        .and_then(Value::as_str)
        .unwrap_or("1m")
        .to_string();
    Ok(ScrollRequestBody { scroll, scroll_id })
}

/// Stable label value for the `query_type` dimension of
/// `surch_search_total`. Anything outside the explicit set folds into
/// `"other"` so the cardinality of the metric stays bounded.
fn classify_query(query: Option<&SearchQuery>) -> &'static str {
    match query {
        Some(SearchQuery::Match { .. }) => "match",
        Some(SearchQuery::MultiMatch { .. }) => "multi_match",
        Some(SearchQuery::Term { .. }) => "term",
        Some(SearchQuery::Bool { .. }) => "bool",
        _ => "other",
    }
}

/// Best-effort classification of a raw search body for the cache-hit
/// path, where the body has not been parsed yet. Falls back to
/// `"other"` whenever the JSON is missing or malformed.
fn classify_search_body(body: &str) -> &'static str {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return "other";
    };
    let Some(query) = value.get("query").and_then(Value::as_object) else {
        return "other";
    };
    if query.contains_key("match") {
        "match"
    } else if query.contains_key("multi_match") {
        "multi_match"
    } else if query.contains_key("term") {
        "term"
    } else if query.contains_key("bool") {
        "bool"
    } else {
        "other"
    }
}

fn hash_search_body(body: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    body.hash(&mut hasher);
    hasher.finish()
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

    // A12.1: when aggregations are requested we must walk every matched
    // document (not just the top-K window) so the bucket counts cover
    // the full match set. Skip the top-K shortcut in that case.
    if request.aggs.is_empty() {
        if let Some(response) =
            run_topk_search(state, indices, request, scoring_enabled, started_at)
        {
            return response;
        }
    }

    let mut matched_documents: Vec<ScoredDocument> = Vec::new();
    for index in indices {
        matched_documents.extend(match_documents_for_index(
            state,
            index,
            request.query.as_ref(),
        ));
    }
    if scoring_enabled {
        if let Some(min) = request.min_score {
            matched_documents.retain(|doc| doc.score >= min);
        }
    }
    sort_scored_documents(&mut matched_documents, &request.sort, scoring_enabled);
    let max_score = compute_max_score(&matched_documents, scoring_enabled);
    let total = matched_documents.len() as u64;
    let aggregations = compute_aggregations(&request.aggs, &matched_documents);
    let hits = paginate_hits(request, &matched_documents, scoring_enabled);
    let total_summary = resolve_total_hits(total, request.track_total_hits.as_ref());

    let mut response = build_search_response_with_total(
        hits,
        total_summary,
        started_at.elapsed().as_millis() as u64,
    );
    response.hits.max_score = max_score;
    response.aggregations = aggregations;
    response
}

/// A12.1+A12.2+A12.3: dispatch every declared aggregation against the
/// post-filter matched-document set. Returns `None` when no aggs are
/// declared so the response stays shape-compatible with zero-agg
/// callers.
fn compute_aggregations(
    specs: &BTreeMap<String, AggSpec>,
    matched_documents: &[ScoredDocument],
) -> Option<BTreeMap<String, AggResult>> {
    if specs.is_empty() {
        return None;
    }
    let mut out: BTreeMap<String, AggResult> = BTreeMap::new();
    for (name, spec) in specs {
        let result = match spec {
            AggSpec::Terms { field, size } => {
                compute_terms_aggregation(matched_documents, field, *size)
            }
            AggSpec::DateHistogram {
                field,
                calendar_interval,
                format,
            } => compute_date_histogram_aggregation(
                matched_documents,
                field,
                *calendar_interval,
                format.as_deref(),
            ),
            AggSpec::Cardinality { field } => {
                compute_cardinality_aggregation(matched_documents, field)
            }
        };
        out.insert(name.clone(), result);
    }
    Some(out)
}

/// A12.1: `terms` aggregation executor. Iterates the matched documents,
/// reads `_source[field]` (with the `.raw` / `.norm` sub-field alias
/// already handled by `lookup_sort_value`), counts buckets, and returns
/// the top `size` ordered by `doc_count` desc + key asc tiebreak.
///
/// Behaviour notes:
/// - `null` and missing values are skipped (matches ES default — a
///   `missing` bucket would need an explicit `missing: "…"` option,
///   which is out of scope here),
/// - array-valued fields contribute one increment per element (matches
///   ES `keyword[]` semantics),
/// - `size = 0` returns an empty bucket list (ES rejects this with 400,
///   but Surch treats it as "no buckets requested" today; tighten when
///   matchID's UI starts emitting it).
fn compute_terms_aggregation(
    matched_documents: &[ScoredDocument],
    field: &str,
    size: usize,
) -> AggResult {
    let mut counts: BTreeMap<TermsKey, (Value, u64)> = BTreeMap::new();
    for scored in matched_documents {
        let Some(value) = lookup_sort_value(&scored.doc.source, field) else {
            continue;
        };
        match value {
            Value::Array(items) => {
                for item in items {
                    record_terms_value(&mut counts, item);
                }
            }
            other => record_terms_value(&mut counts, other),
        }
    }

    // Two-stage sort: doc_count desc, then key asc as tiebreak.
    let mut sorted: Vec<(Value, u64)> = counts.into_values().collect();
    sorted.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| compare_values(&left.0, &right.0))
    });

    let buckets = sorted
        .into_iter()
        .take(size)
        .map(|(key, doc_count)| AggBucket {
            key,
            key_as_string: None,
            doc_count,
        })
        .collect();
    AggResult::Buckets { buckets }
}

/// A12.2: `date_histogram` aggregation executor. Each matched document
/// contributes its `field` value (string `YYYYMMDD`, as stored by the
/// A7 `date{format:yyyyMMdd}` field type) to the bucket obtained by
/// truncating the date to the requested `calendar_interval`. Buckets
/// are returned sorted by key ascending — matchID's analytics tab
/// renders the timeline left-to-right and depends on that order.
///
/// Behaviour notes:
/// - Values that do not parse as `YYYYMMDD` are skipped (no synthetic
///   bucket — matches ES default when no `missing` option is set),
/// - array-valued fields contribute one increment per element,
/// - `format` is forwarded verbatim into `key_as_string` when set; we
///   do not re-render the date through a different pattern in this
///   MVP (matchID only ever emits `yyyyMMdd`, which happens to match
///   the storage shape).
fn compute_date_histogram_aggregation(
    matched_documents: &[ScoredDocument],
    field: &str,
    calendar_interval: CalendarInterval,
    format: Option<&str>,
) -> AggResult {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for scored in matched_documents {
        let Some(value) = lookup_sort_value(&scored.doc.source, field) else {
            continue;
        };
        match value {
            Value::Array(items) => {
                for item in items {
                    if let Some(bucket_key) =
                        bucket_key_for_date(item, calendar_interval)
                    {
                        *counts.entry(bucket_key).or_insert(0) += 1;
                    }
                }
            }
            other => {
                if let Some(bucket_key) = bucket_key_for_date(other, calendar_interval)
                {
                    *counts.entry(bucket_key).or_insert(0) += 1;
                }
            }
        }
    }

    // BTreeMap iteration already yields keys ascending — but we still
    // map through `Vec` so the response shape is identical to
    // `terms`.
    let buckets = counts
        .into_iter()
        .map(|(key, doc_count)| AggBucket {
            key: Value::String(key.clone()),
            key_as_string: format.map(|_| key),
            doc_count,
        })
        .collect();
    AggResult::Buckets { buckets }
}

/// A12.2: truncate a `YYYYMMDD` JSON value to the requested calendar
/// interval. Returns `None` for any value that does not parse.
fn bucket_key_for_date(value: &Value, interval: CalendarInterval) -> Option<String> {
    let s = value.as_str()?;
    if s.len() != 8 || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[4..6].parse().ok()?;
    let day: u32 = s[6..8].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(match interval {
        CalendarInterval::Day => format!("{year:04}{month:02}{day:02}"),
        CalendarInterval::Month => format!("{year:04}{month:02}01"),
        CalendarInterval::Year => format!("{year:04}0101"),
        CalendarInterval::Week => {
            // Truncate to the Monday of the ISO week containing the
            // input date. We compute the day-of-year via the Gregorian
            // calendar (no chrono dependency), then anchor on Monday
            // using Zeller's congruence-style weekday derivation.
            let (anchor_year, anchor_month, anchor_day) =
                week_anchor_monday(year, month, day);
            format!("{anchor_year:04}{anchor_month:02}{anchor_day:02}")
        }
    })
}

/// A12.2 helper: return the (year, month, day) of the Monday of the
/// ISO week containing the given Gregorian date. Pure-Rust, no
/// dependency.
fn week_anchor_monday(year: i32, month: u32, day: u32) -> (i32, u32, u32) {
    // Convert to Rata Die (day number since 0000-12-31). Algorithm
    // from Howard Hinnant's date library, transcribed for u32 / i32.
    fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
        let y = if m <= 2 { y - 1 } else { y } as i64;
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as u64; // [0, 399]
        let m = m as i64;
        let d = d as i64;
        let doy = ((153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1) as u64;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe as i64 - 719468
    }
    fn civil_from_days(z: i64) -> (i32, u32, u32) {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = (z - era * 146097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = if m <= 2 { y + 1 } else { y } as i32;
        (y, m, d)
    }
    let z = days_from_civil(year, month, day);
    // 1970-01-01 was a Thursday → weekday(Thu) = 3 in Mon=0 system.
    // weekday = (z + 3).rem_euclid(7)
    let weekday = (z + 3).rem_euclid(7); // 0=Mon..6=Sun
    civil_from_days(z - weekday)
}

/// A12.3: `cardinality` aggregation executor. Counts the number of
/// distinct values of `field` across the matched-document set. The
/// `.raw` / `.norm` sub-field alias is handled by `lookup_sort_value`
/// (same path as terms / sort). MVP: exact count, no HyperLogLog
/// estimation — matchID's analytics tab consumes the exact value
/// today.
fn compute_cardinality_aggregation(
    matched_documents: &[ScoredDocument],
    field: &str,
) -> AggResult {
    let mut distinct: BTreeSet<TermsKey> = BTreeSet::new();
    for scored in matched_documents {
        let Some(value) = lookup_sort_value(&scored.doc.source, field) else {
            continue;
        };
        match value {
            Value::Array(items) => {
                for item in items {
                    if !matches!(item, Value::Null) {
                        distinct.insert(TermsKey::from_value(item));
                    }
                }
            }
            other => {
                if !matches!(other, Value::Null) {
                    distinct.insert(TermsKey::from_value(other));
                }
            }
        }
    }
    AggResult::Value {
        value: Value::from(distinct.len() as u64),
    }
}

fn record_terms_value(counts: &mut BTreeMap<TermsKey, (Value, u64)>, value: &Value) {
    if matches!(value, Value::Null) {
        return;
    }
    let key = TermsKey::from_value(value);
    let entry = counts.entry(key).or_insert_with(|| (value.clone(), 0));
    entry.1 += 1;
}

/// A12.1: dedup key for the `terms` aggregation. We cannot hash a
/// `serde_json::Value` directly (NaN poisons `Number` equality), so we
/// project to a stable byte-shape that round-trips through `BTreeMap`
/// without losing JSON type fidelity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum TermsKey {
    Bool(bool),
    Number(String),
    String(String),
    Other(String),
}

impl TermsKey {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Bool(b) => TermsKey::Bool(*b),
            Value::Number(n) => TermsKey::Number(n.to_string()),
            Value::String(s) => TermsKey::String(s.clone()),
            other => TermsKey::Other(other.to_string()),
        }
    }
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
    // min_score requires scoring every candidate to know the post-filter
    // total; the top-K shortcut can't deliver that cheaply, so we hand
    // off to the full-scan path.
    if request.min_score.is_some() {
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
        } => {
            state.documents_for_match_internal(index, field, value, *operator == MatchOperator::And)
        }
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

    // MaxScore-style skipping for OR-Match queries: iterate tokens from
    // highest to lowest max BM25 contribution. Once the top-K threshold
    // exceeds a token's max contribution, only docs already scored from
    // a rarer token can still make it; new docs from that token are
    // skipped. For BAN-style queries where one query token is rare and
    // the other is common, this collapses scoring work from "all
    // candidates" to "candidates of the rare token(s)".
    match query {
        SearchQuery::Match {
            field,
            value,
            operator,
        } if *operator != MatchOperator::And => {
            if let Some(scored_pairs) = maxscore_match(field, value, limit, &scoring_context, total)
            {
                return finalize_topk(state, index, scored_pairs, total, limit);
            }
        }
        SearchQuery::MultiMatch {
            query: value,
            fields,
            operator,
        } if *operator != MatchOperator::And => {
            if let Some(scored_pairs) = maxscore_multi_match(fields, value, limit, &scoring_context)
            {
                return finalize_topk(state, index, scored_pairs, total, limit);
            }
        }
        _ => {}
    }

    let scored: Vec<(f64, u32)> = candidates
        .iter()
        .map(|internal_id| {
            let score = score_for_query(query, *internal_id, &scoring_context);
            (score, *internal_id)
        })
        .collect();

    finalize_topk(state, index, scored, total, limit)
}

fn finalize_topk(
    state: &AppState,
    index: &str,
    scored: Vec<(f64, u32)>,
    total: u64,
    limit: usize,
) -> Option<(Vec<ScoredDocument>, u64)> {
    let cmp = |a: &(f64, u32), b: &(f64, u32)| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    };

    // Scalar top-K: K-sized sorted array, O(1) compare against the
    // current worst on the hot path. Replaces the prior
    // select_nth_unstable_by + sort_by pair (Tantivy 0.22 optim #2).
    let mut topn = TopN::new(limit, cmp);
    for entry in scored {
        topn.push(entry);
    }
    let scored = topn.into_sorted_vec();

    let winner_ids: Vec<u32> = scored.iter().map(|(_, id)| *id).collect();
    let hydrated = state.documents_by_internal_ids(index, &winner_ids);

    let result = scored
        .into_iter()
        .zip(hydrated)
        .map(|((score, _), doc)| ScoredDocument { doc, score })
        .collect();

    Some((result, total))
}

/// MaxScore-style top-K scoring for a `MultiMatch` over several fields
/// (OR semantics across fields and across tokens within each field). The
/// per-doc score follows the existing fallback semantics: take the max
/// across fields, then floor at `1e-9`. Each field runs the same
/// `maxscore_match` path independently — a doc that ends up in overall
/// top-K is necessarily in at least one field's scored set, because the
/// field giving it its final score must have scored it.
fn maxscore_multi_match(
    fields: &[String],
    value: &str,
    limit: usize,
    ctx: &SearchScoringContext,
) -> Option<Vec<(f64, u32)>> {
    if fields.is_empty() {
        return None;
    }

    let mut combined: BTreeMap<u32, f64> = BTreeMap::new();
    let mut any_field = false;
    for field in fields {
        let Some(field_scores) = maxscore_match(field, value, limit, ctx, 0) else {
            continue;
        };
        any_field = true;
        for (score, doc_id) in field_scores {
            let entry = combined.entry(doc_id).or_insert(0.0);
            if score > *entry {
                *entry = score;
            }
        }
    }

    if !any_field {
        return None;
    }

    let floor = 1.0 / 1e9_f64.max(1.0);
    Some(
        combined
            .into_iter()
            .map(|(id, score)| (score.max(floor), id))
            .collect(),
    )
}

/// MaxScore-style top-K scoring for a single-field `Match` (OR semantics):
/// iterate query tokens from highest to lowest max BM25 contribution, and
/// once the running top-K threshold exceeds a token's max contribution,
/// only update docs already scored from a rarer token. Returns the full
/// list of scored (score, internal doc id) pairs, or `None` if the path
/// cannot be used (no field stats, no scorable tokens, etc.).
fn maxscore_match(
    field: &str,
    value: &str,
    limit: usize,
    ctx: &SearchScoringContext,
    total_hint: u64,
) -> Option<Vec<(f64, u32)>> {
    let field_stats = ctx.field_stats(field)?;
    if field_stats.doc_count == 0 {
        return None;
    }
    let tokens = ctx.mapping.analyzer(field).terms(value);
    if tokens.is_empty() {
        return None;
    }

    let avg_doc_len = field_stats.avg_doc_len;
    let config = Bm25Config::default();
    let norms_enabled = field_stats.norms_enabled;

    let min_doc_len: u64 = if norms_enabled {
        field_stats.min_doc_len().unwrap_or(1)
    } else {
        1
    };

    // Deduplicate repeated tokens (e.g. analyzer-emitted duplicates from
    // queries like "Paris Paris Paris") and turn the count into a boost.
    // Equivalent to Lucene's "to be or not to be" -> "to^2 be^2 or not"
    // rewrite, but applied at scoring time so each posting list is walked
    // once per distinct token.
    let mut token_boosts: BTreeMap<String, u32> = BTreeMap::new();
    for token in tokens {
        *token_boosts.entry(token).or_insert(0) += 1;
    }

    struct TokenInfo<'a> {
        stats: &'a TermScoringStats,
        max_contrib: f64,
        boost: f64,
    }

    let mut token_infos: Vec<TokenInfo<'_>> = token_boosts
        .iter()
        .filter_map(|(token, count)| {
            let stats = ctx.term_stats(field, token)?;
            if stats.doc_freq == 0 || stats.doc_freq > field_stats.doc_count {
                return None;
            }
            let max_tf = stats.max_term_freq().max(1);
            let boost = *count as f64;
            let single = bm25_score(
                config,
                field_stats.doc_count,
                stats.doc_freq,
                max_tf,
                min_doc_len,
                avg_doc_len,
            )
            .ok()?;
            Some(TokenInfo {
                stats,
                max_contrib: single * boost,
                boost,
            })
        })
        .collect();

    if token_infos.is_empty() {
        return None;
    }
    token_infos.sort_by(|a, b| {
        b.max_contrib
            .partial_cmp(&a.max_contrib)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    const BLOCK_SIZE: usize = 128;

    // Precompute the per-block upper bound contribution for every token
    // (Block-Max WAND, à la Tantivy `BlockWAND` and Lucene block-max
    // postings). Per-block max term frequency is read directly from
    // `BlockMeta::max_term_freq`, which was computed once at
    // `PostingsBuilder::build()` time when the postings were folded
    // into 128-entry chunks — so this loop is O(num_blocks) BM25 calls
    // and no longer iterates the postings themselves. The block max
    // contrib is `bm25(max_tf_in_block, min_doc_len, …)` times the
    // token's repeat boost.
    let block_max_contribs: Vec<Vec<f64>> = token_infos
        .iter()
        .map(|token| {
            token
                .stats
                .block_metas
                .iter()
                .map(|meta| {
                    let block_max_tf = u64::from(meta.max_term_freq).max(1);
                    let block_score = bm25_score(
                        config,
                        field_stats.doc_count,
                        token.stats.doc_freq,
                        block_max_tf,
                        min_doc_len,
                        avg_doc_len,
                    )
                    .unwrap_or(0.0);
                    block_score * token.boost
                })
                .collect()
        })
        .collect();

    let mut scored: BTreeMap<u32, f64> = BTreeMap::new();
    let mut threshold = f64::NEG_INFINITY;

    for (token_idx, token) in token_infos.iter().enumerate() {
        let allow_new_docs = token.max_contrib >= threshold;
        let token_blocks = &block_max_contribs[token_idx];

        for (block_idx, block) in token
            .stats
            .term_freq_by_doc_id
            .chunks(BLOCK_SIZE)
            .enumerate()
        {
            if block.is_empty() {
                continue;
            }
            let block_max = token_blocks[block_idx];

            // Block-level skip: if the block's tightest upper bound is
            // below the running threshold AND no doc in this block is
            // already scored from a rarer token (so we cannot increase
            // its score either), the whole block contributes nothing
            // to top-K and can be skipped without touching it.
            if !allow_new_docs && block_max < threshold {
                let block_first = block[0].0;
                let block_last = block[block.len() - 1].0;
                if scored.range(block_first..=block_last).next().is_none() {
                    continue;
                }
            }

            for (doc_id, tf) in block {
                if *tf == 0 {
                    continue;
                }
                let in_scored = scored.contains_key(doc_id);
                if !in_scored && !allow_new_docs {
                    continue;
                }

                let doc_len = if norms_enabled {
                    match field_stats.doc_len(*doc_id) {
                        Some(len) if len > 0 => len,
                        _ => continue,
                    }
                } else {
                    1
                };

                let contrib = match bm25_score(
                    config,
                    field_stats.doc_count,
                    token.stats.doc_freq,
                    *tf,
                    doc_len,
                    avg_doc_len,
                ) {
                    Ok(score) => score * token.boost,
                    Err(_) => continue,
                };

                let entry = scored.entry(*doc_id).or_insert(0.0);
                *entry += contrib;
            }
        }

        if scored.len() >= limit {
            let mut values: Vec<f64> = scored.values().copied().collect();
            let k = values.len() - limit;
            values.select_nth_unstable_by(k, |a, b| {
                a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
            });
            threshold = values[k];
        }
    }

    let _ = total_hint;
    Some(scored.into_iter().map(|(id, score)| (score, id)).collect())
}

fn is_scoring_query(query: &SearchQuery) -> bool {
    match query {
        SearchQuery::Match { .. }
        | SearchQuery::MatchPhrase { .. }
        | SearchQuery::MultiMatch { .. }
        | SearchQuery::Fuzzy { .. }
        | SearchQuery::Bool { .. } => true,
        SearchQuery::FunctionScore { inner, .. } => is_scoring_query(inner),
        // `geo_distance` is a constant-score filter — never the scoring driver.
        SearchQuery::GeoDistance { .. } => false,
        _ => false,
    }
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
            SearchQuery::Bool { .. } => {
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
            SearchQuery::MatchAll { .. } => state.documents(index),
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
    let public_ids = documents
        .iter()
        .map(|doc| doc.id.as_str())
        .collect::<Vec<_>>();
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
        SearchQuery::Bool {
            must,
            should,
            minimum_should_match,
            boost,
            ..
        } => {
            // `filter` clauses are excluded — they restrict candidacy, not score.
            let must_score: f64 = must
                .iter()
                .map(|clause| score_for_query(clause, internal_doc_id, scoring_context))
                .sum();

            // The wildcard arm at the bottom returns 1.0 for non-scoring
            // sub-queries; filter that placeholder out so it doesn't inflate
            // the sum. BM25 always emits > 1.0 for real matches.
            let should_score: f64 = should
                .iter()
                .map(|clause| score_for_query(clause, internal_doc_id, scoring_context))
                .filter(|score| *score != 1.0)
                .sum();

            // `minimum_should_match` gates candidacy in query_matches; it
            // does not affect scoring once we are in this arm.
            let _ = minimum_should_match;
            let combined = must_score + should_score;
            let base = if combined > 0.0 { combined } else { 1.0 };
            base * *boost
        }
        SearchQuery::MatchAll { boost } => *boost,
        SearchQuery::FunctionScore { inner, boost } => {
            score_for_query(inner, internal_doc_id, scoring_context) * *boost
        }
        // `geo_distance` is a filter — constant score so it does not
        // perturb BM25 ranking (matchID always wraps it in `bool.filter`).
        SearchQuery::GeoDistance { .. } => 1.0,
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
        SearchQuery::Bool {
            must,
            filter,
            should,
            ..
        } => {
            // `filter` clauses do not score but we still need their term
            // statistics for correct posting candidate evaluation when
            // they wrap scoring sub-queries indirectly (cheap, idempotent).
            for clause in must.iter().chain(filter.iter()).chain(should.iter()) {
                collect_scoring_field_tokens(clause, mapping, field_tokens);
            }
        }
        SearchQuery::FunctionScore { inner, .. } => {
            collect_scoring_field_tokens(inner, mapping, field_tokens);
        }
        // `geo_distance` does not contribute term statistics.
        SearchQuery::GeoDistance { .. } => {}
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
        field_stats.doc_len(internal_doc_id)?
    } else {
        1
    };
    let avg_doc_len = field_stats.avg_doc_len;

    // Deduplicate repeated tokens; each repeat boosts the same posting
    // lookup instead of walking the same term_freq twice.
    let mut token_boosts: BTreeMap<&str, u32> = BTreeMap::new();
    for token in &query_tokens {
        *token_boosts.entry(token.as_str()).or_insert(0) += 1;
    }

    let config = Bm25Config::default();
    let mut total = 0.0_f64;
    for (query_token, boost) in &token_boosts {
        let term_stats = scoring_context.term_stats(field, query_token)?;
        let term_freq = term_stats.term_freq(internal_doc_id);
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
            total += score * (*boost as f64);
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
            min_score: None,
            aggs: BTreeMap::new(),
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
    let min_score = object.get("min_score").map(parse_min_score).transpose()?;
    // ES accepts both `aggs` and `aggregations` — matchID's UI emits
    // the short form, the analytics tab emits the long form.
    let aggs_value = object.get("aggs").or_else(|| object.get("aggregations"));
    let aggs = aggs_value.map(parse_aggs).transpose()?.unwrap_or_default();

    // ES 7.x `index.max_result_window` defaults to 10 000. Surch refuses
    // pagination requests that would force scoring beyond that window —
    // matches what deces-backend sees today.
    let window = from.unwrap_or(0).saturating_add(size.unwrap_or(0));
    if window > MAX_RESULT_WINDOW {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "search_phase_execution_exception",
            format!(
                "Result window is too large, from + size must be less than or equal to: [{MAX_RESULT_WINDOW}] but was [{window}]"
            ),
        ));
    }

    Ok(SearchRequest {
        query,
        from,
        size,
        source,
        track_total_hits,
        sort,
        highlight,
        min_score,
        aggs,
    })
}

/// A12.1: parse the top-level `aggs` / `aggregations` block. Only `terms`
/// is honoured today — every other agg type returns a deterministic
/// "phase 2" parse error so matchID's analytics tab sees a parseable
/// 400 instead of a silent zero-bucket response.
fn parse_aggs(value: &Value) -> Result<BTreeMap<String, AggSpec>, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`aggs` must be an object",
        )
    })?;
    let mut aggs = BTreeMap::new();
    for (name, body) in object {
        if name.is_empty() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "`aggs` entries must have non-empty names",
            ));
        }
        let agg_body = body.as_object().ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("`aggs.{name}` must be an object"),
            )
        })?;
        // Reject sub-aggregations (`aggs` nested inside an agg body) —
        // composite already implies sub-source semantics and we only
        // ship terms here.
        if agg_body.contains_key("aggs") || agg_body.contains_key("aggregations") {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!(
                    "agg `{name}`: nested sub-aggregations not implemented yet, tracked in A12 phase 2"
                ),
            ));
        }
        let mut spec: Option<AggSpec> = None;
        for (agg_type, agg_options) in agg_body {
            let parsed = match agg_type.as_str() {
                "terms" => parse_terms_agg(name, agg_options)?,
                "date_histogram" => parse_date_histogram_agg(name, agg_options)?,
                "cardinality" => parse_cardinality_agg(name, agg_options)?,
                "composite" => {
                    return Err(OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        "agg type `composite` not implemented yet, tracked in A12 phase 2",
                    ));
                }
                other => {
                    return Err(OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        format!(
                            "agg type `{other}` not implemented yet, tracked in A12 phase 2"
                        ),
                    ));
                }
            };
            if spec.is_some() {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    format!("agg `{name}` must declare exactly one agg type"),
                ));
            }
            spec = Some(parsed);
        }
        let spec = spec.ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("agg `{name}` must declare an agg type"),
            )
        })?;
        aggs.insert(name.clone(), spec);
    }
    Ok(aggs)
}

fn parse_terms_agg(name: &str, value: &Value) -> Result<AggSpec, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("agg `{name}`: `terms` body must be an object"),
        )
    })?;
    let field = object
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("agg `{name}`: `terms.field` is required"),
            )
        })?
        .to_string();
    let size = match object.get("size") {
        None => DEFAULT_TERMS_AGG_SIZE,
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    format!("agg `{name}`: `terms.size` must be a non-negative integer"),
                )
            })?;
            usize::try_from(n).unwrap_or(usize::MAX)
        }
    };
    Ok(AggSpec::Terms { field, size })
}

/// A12.2: parse a `date_histogram` agg body. `calendar_interval` is
/// mandatory (matchID always sets it); `format` is optional and gets
/// echoed back into the bucket's `key_as_string` when set.
fn parse_date_histogram_agg(name: &str, value: &Value) -> Result<AggSpec, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("agg `{name}`: `date_histogram` body must be an object"),
        )
    })?;
    let field = object
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("agg `{name}`: `date_histogram.field` is required"),
            )
        })?
        .to_string();
    let interval_str = object
        .get("calendar_interval")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("agg `{name}`: `date_histogram.calendar_interval` is required"),
            )
        })?;
    let calendar_interval = match interval_str {
        "day" | "1d" => CalendarInterval::Day,
        "week" | "1w" => CalendarInterval::Week,
        "month" | "1M" => CalendarInterval::Month,
        "year" | "1y" => CalendarInterval::Year,
        other => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!(
                    "agg `{name}`: unsupported `date_histogram.calendar_interval` `{other}` \
                     (day|week|month|year only, tracked in A12 phase 2)"
                ),
            ));
        }
    };
    let format = match object.get("format") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| {
                    OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        format!("agg `{name}`: `date_histogram.format` must be a string"),
                    )
                })?
                .to_string(),
        ),
    };
    Ok(AggSpec::DateHistogram {
        field,
        calendar_interval,
        format,
    })
}

/// A12.3: parse a `cardinality` agg body. `field` is the only
/// required option; `precision_threshold` (HLL knob) is accepted and
/// silently ignored because the MVP returns the exact count.
fn parse_cardinality_agg(name: &str, value: &Value) -> Result<AggSpec, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("agg `{name}`: `cardinality` body must be an object"),
        )
    })?;
    let field = object
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("agg `{name}`: `cardinality.field` is required"),
            )
        })?
        .to_string();
    Ok(AggSpec::Cardinality { field })
}

fn parse_min_score(value: &Value) -> Result<f64, OpenSearchError> {
    let n = value.as_f64().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`min_score` must be a number",
        )
    })?;
    if !n.is_finite() || n.is_sign_negative() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`min_score` must be a non-negative finite number",
        ));
    }
    Ok(n)
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
        "match_all" => parse_match_all_query(query_body),
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
        "function_score" => parse_function_score_query(query_body),
        "geo_distance" => parse_geo_distance_query(query_body),
        unknown => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("unsupported search query `{unknown}`"),
        )),
    }
}

/// Parse `match_all` query body. Accepts `{}` (boost=1.0) and
/// `{ "boost": <number> }`. Boost must be a finite non-negative
/// number — OpenSearch rejects negative boosts at parse time.
fn parse_match_all_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "match_all query body must be an object",
        )
    })?;

    let mut boost: f64 = 1.0;
    for (key, raw) in object {
        match key.as_str() {
            "boost" => {
                boost = parse_boost_value("match_all", raw)?;
            }
            unknown => {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    format!("match_all query does not support `{unknown}`"),
                ));
            }
        }
    }

    Ok(SearchQuery::MatchAll { boost })
}

fn parse_boost_value(context: &str, value: &Value) -> Result<f64, OpenSearchError> {
    let number = value.as_f64().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{context} `boost` must be a number"),
        )
    })?;
    if !number.is_finite() || number < 0.0 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("{context} `boost` must be a finite, non-negative number"),
        ));
    }
    Ok(number)
}

fn parse_match_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let (field, body) = parse_single_field_query("match", value)?;
    let query_text = parse_query_text(body, "match query")?;
    let operator = parse_match_operator(body, "match")?;

    // Object form: `{ "match": { "F": { "query": "…", "fuzziness": "AUTO|N" } } }`.
    // When `fuzziness` is present, route to the fuzzy executor so that
    // bounded Damerau-Levenshtein is applied per analyzed token.
    if let Some(object) = body.as_object() {
        if let Some(raw) = object.get("fuzziness") {
            let fuzziness = parse_fuzzy_query_fuzziness(raw)?;
            return Ok(SearchQuery::Fuzzy {
                field,
                value: query_text,
                fuzziness,
            });
        }
    }

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

    let must = parse_bool_clause_bucket(object, "must")?;
    let filter = parse_bool_clause_bucket(object, "filter")?;
    let should = parse_bool_clause_bucket(object, "should")?;

    // Each bucket-key is optional individually but at least one must be
    // present, otherwise the body is empty and the request is malformed.
    if must.is_empty() && filter.is_empty() && should.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "bool query must contain at least one of `must`, `filter`, or `should`",
        ));
    }

    let minimum_should_match = match object.get("minimum_should_match") {
        Some(raw) => parse_minimum_should_match(raw, should.len())?,
        None => {
            // ES 7.x rule: when the only non-empty bucket is `should`,
            // MSM defaults to 1; otherwise it defaults to 0.
            if must.is_empty() && filter.is_empty() && !should.is_empty() {
                1
            } else {
                0
            }
        }
    };

    let boost = match object.get("boost") {
        Some(raw) => parse_boost(raw)?,
        None => 1.0,
    };

    Ok(SearchQuery::Bool {
        must,
        filter,
        should,
        minimum_should_match,
        boost,
    })
}

/// Parse one of `must` / `filter` / `should` from a `bool` body. Each
/// bucket accepts a single object **or** an array of objects per ES 7.x
/// conventions; missing keys yield an empty `Vec`.
fn parse_bool_clause_bucket(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Vec<SearchQuery>, OpenSearchError> {
    let Some(raw) = object.get(key) else {
        return Ok(Vec::new());
    };
    match raw {
        Value::Array(items) => {
            let mut clauses = Vec::with_capacity(items.len());
            for item in items {
                clauses.push(parse_search_query(item)?);
            }
            Ok(clauses)
        }
        Value::Object(_) => Ok(vec![parse_search_query(raw)?]),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("bool query `{key}` must be an object or an array of objects"),
        )),
    }
}

/// Parse a `minimum_should_match` token. Supports the integer form
/// (positive or negative; negative means `should.len() - |n|`) and the
/// simple percentage form `"N%"` (`"50%"` → `ceil(0.5 * should.len())`).
/// `should_len == 0` clamps the result to 0.
fn parse_minimum_should_match(value: &Value, should_len: usize) -> Result<u32, OpenSearchError> {
    match value {
        Value::Number(num) => {
            let signed = num.as_i64().ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "`minimum_should_match` must be an integer",
                )
            })?;
            Ok(resolve_msm_integer(signed, should_len))
        }
        Value::String(text) => parse_msm_string(text, should_len),
        _ => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`minimum_should_match` must be an integer or a percentage string",
        )),
    }
}

fn resolve_msm_integer(signed: i64, should_len: usize) -> u32 {
    if should_len == 0 {
        return 0;
    }
    let len_i = should_len as i64;
    let resolved = if signed < 0 {
        (len_i + signed).max(0)
    } else {
        signed.min(len_i)
    };
    resolved.max(0) as u32
}

fn parse_msm_string(text: &str, should_len: usize) -> Result<u32, OpenSearchError> {
    let trimmed = text.trim();
    if let Some(percent_str) = trimmed.strip_suffix('%') {
        let percent: f64 = percent_str.trim().parse().map_err(|_| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("`minimum_should_match` percentage `{text}` must be numeric"),
            )
        })?;
        if should_len == 0 {
            return Ok(0);
        }
        let raw = (percent / 100.0) * should_len as f64;
        // ES rounds toward zero on negative percentages (`-25%` means
        // "drop 25 % of clauses"); we follow that convention.
        let resolved = if percent < 0.0 {
            should_len as f64 + raw
        } else {
            raw
        };
        let clamped = resolved.floor().clamp(0.0, should_len as f64) as i64;
        return Ok(clamped.max(0) as u32);
    }
    let signed: i64 = trimmed.parse().map_err(|_| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`minimum_should_match` value `{text}` is not a supported form"),
        )
    })?;
    Ok(resolve_msm_integer(signed, should_len))
}

fn parse_boost(value: &Value) -> Result<f64, OpenSearchError> {
    let raw = value.as_f64().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`boost` must be a number",
        )
    })?;
    if !raw.is_finite() || raw < 0.0 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`boost` must be a finite non-negative number",
        ));
    }
    Ok(raw)
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

/// Parse a `function_score` query body.
///
/// matchID's deces-backend uses `function_score` today as a pure
/// wrapper (no `functions` declared) to keep the codebase ready for
/// future custom scoring. This implementation accepts:
///
/// - `query` (required) — the inner query to wrap;
/// - `boost` (optional, non-negative finite) — multiplicative scaling
///   applied to the inner `_score`;
/// - `functions: []` (optional empty array) — accepted for
///   forward-compat, no scoring effect today;
/// - `score_mode` / `boost_mode` (optional strings) — accepted only
///   when there are no functions, since they would otherwise have
///   no semantics to drive;
/// - any unknown top-level key returns HTTP 400 so wire-shape drift
///   is caught early.
fn parse_function_score_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`function_score` must be an object",
        )
    })?;

    let mut inner: Option<SearchQuery> = None;
    let mut boost: f64 = 1.0;

    for (key, body) in object {
        match key.as_str() {
            "query" => {
                inner = Some(parse_search_query(body)?);
            }
            "boost" => boost = parse_boost(body)?,
            "functions" => {
                let arr = body.as_array().ok_or_else(|| {
                    OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        "`function_score.functions` must be an array",
                    )
                })?;
                if !arr.is_empty() {
                    return Err(OpenSearchError::new(
                        StatusCode::BAD_REQUEST,
                        "parsing_exception",
                        "`function_score.functions` is parsed but scoring functions are not implemented yet (tracked under matchID gap A5 phase 2)",
                    ));
                }
            }
            "score_mode" | "boost_mode" | "max_boost" | "min_score" => {
                // Accepted for forward-compat; ignored when `functions` is
                // empty since none of these have semantics without it.
            }
            unknown => {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    format!("unsupported `function_score` field `{unknown}`"),
                ));
            }
        }
    }

    let inner = inner.ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`function_score` must contain a `query`",
        )
    })?;

    Ok(SearchQuery::FunctionScore {
        inner: Box::new(inner),
        boost,
    })
}

/// Parse a `geo_distance` query body (matchID intake §2.6).
///
/// Wire shape, verbatim from deces-backend:
///
/// ```json
/// {
///   "geo_distance": {
///     "distance": "1km",
///     "GEOPOINT_NAISSANCE": { "lat": 48.85, "lon": 2.35 }
///   }
/// }
/// ```
///
/// The pivot field is named freely (matchID uses `GEOPOINT_NAISSANCE` and
/// `GEOPOINT_DECES`); the parser pulls the first non-`distance`,
/// non-`validation_method` key as the field.
///
/// Accepted source representations for the target point (and for the
/// indexed geo_point at executor time, see `parse_geo_point_source`):
/// - `{ "lat": <num>, "lon": <num> }` (matchID shape),
/// - the string `"lat,lon"` (ES compat),
/// - the GeoJSON-style array `[lon, lat]` (ES compat).
///
/// Distance units recognised (regex from `queries.ts:229`): `km`, `m`,
/// `mi`, `yd`, `ft`, `NM`. Unit comparison is case-insensitive.
fn parse_geo_distance_query(value: &Value) -> Result<SearchQuery, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`geo_distance` query body must be an object",
        )
    })?;

    let distance_raw = object.get("distance").ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "`geo_distance` query must contain `distance`",
        )
    })?;
    let distance_meters = parse_geo_distance_meters(distance_raw)?;

    // The pivot field is named freely. Skip ES-7.x bookkeeping keys
    // (`distance`, `distance_type`, `validation_method`, `ignore_unmapped`)
    // and take the first remaining key as the field.
    let (field, point_value) = object
        .iter()
        .find(|(key, _)| {
            !matches!(
                key.as_str(),
                "distance"
                    | "distance_type"
                    | "validation_method"
                    | "ignore_unmapped"
                    | "_name"
                    | "boost",
            )
        })
        .ok_or_else(|| {
            OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "`geo_distance` query must contain a geo_point field",
            )
        })?;

    let (lat, lon) = parse_geo_point_source(point_value).map_err(|reason| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`geo_distance` field `{field}` has invalid geo_point: {reason}"),
        )
    })?;

    Ok(SearchQuery::GeoDistance {
        field: field.clone(),
        lat,
        lon,
        distance_meters,
    })
}

/// Parse a `distance` value into metres. matchID emits the string form
/// `"<number><unit>"` (e.g. `"1km"`); ES also accepts a bare number,
/// which we treat as metres.
fn parse_geo_distance_meters(value: &Value) -> Result<f64, OpenSearchError> {
    let bad = |reason: &str| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("`geo_distance.distance` {reason}"),
        )
    };

    let number_meters = |meters: f64| -> Result<f64, OpenSearchError> {
        if !meters.is_finite() || meters < 0.0 {
            return Err(bad("must be a finite non-negative number"));
        }
        Ok(meters)
    };

    match value {
        Value::Number(number) => number_meters(number.as_f64().ok_or_else(|| {
            bad("must fit in a 64-bit float")
        })?),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Err(bad("must not be empty"));
            }
            // Walk from the start while we see digits, dot, sign, or
            // exponent characters; the remainder is the unit suffix.
            let split_at = trimmed
                .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | 'e' | 'E')))
                .unwrap_or(trimmed.len());
            let (head, tail) = trimmed.split_at(split_at);
            let value: f64 = head.parse().map_err(|_| {
                bad(&format!("`{trimmed}` is not a valid number with optional unit suffix"))
            })?;
            let unit = tail.trim();
            let multiplier = geo_distance_unit_meters(unit).ok_or_else(|| {
                bad(&format!("unsupported unit `{unit}` (expected one of km|m|mi|yd|ft|NM)"))
            })?;
            number_meters(value * multiplier)
        }
        _ => Err(bad("must be a string with a unit suffix or a number of metres")),
    }
}

/// Map a distance unit suffix to a metre multiplier. Empty suffix means
/// metres. Accepted units mirror the regex used by matchID's
/// `queries.ts:229`. Comparison is case-insensitive except for `NM`
/// which is the canonical ES form for nautical miles.
fn geo_distance_unit_meters(unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "" | "m" => Some(1.0),
        "km" => Some(1_000.0),
        "mi" | "miles" => Some(1_609.344),
        "yd" | "yards" => Some(0.9144),
        "ft" | "feet" => Some(0.3048),
        "nm" | "nmi" => Some(1_852.0),
        _ => None,
    }
}

/// Parse a single geo_point value into `(lat, lon)`.
///
/// Accepted forms (all three are documented in the §2.6 / §2.12
/// matchID intake):
/// - `{ "lat": <number>, "lon": <number> }` — matchID's canonical wire shape,
/// - `"<lat>,<lon>"` (string, ES compat),
/// - `[<lon>, <lat>]` (GeoJSON array, ES compat).
///
/// Returns a static-friendly `&'static str` reason on parse failure so
/// callers can wrap it in their own `OpenSearchError` context.
pub fn parse_geo_point_source(value: &Value) -> Result<(f64, f64), String> {
    let finite_coord = |label: &str, raw: f64| -> Result<f64, String> {
        if !raw.is_finite() {
            return Err(format!("{label} must be a finite number"));
        }
        Ok(raw)
    };

    match value {
        Value::Object(object) => {
            let lat = object
                .get("lat")
                .and_then(Value::as_f64)
                .ok_or_else(|| "object form requires numeric `lat`".to_owned())?;
            let lon = object
                .get("lon")
                .and_then(Value::as_f64)
                .ok_or_else(|| "object form requires numeric `lon`".to_owned())?;
            let lat = finite_coord("`lat`", lat)?;
            let lon = finite_coord("`lon`", lon)?;
            validate_geo_point_bounds(lat, lon)?;
            Ok((lat, lon))
        }
        Value::String(text) => {
            let parts: Vec<&str> = text.split(',').collect();
            if parts.len() != 2 {
                return Err("string form must be `\"<lat>,<lon>\"`".to_owned());
            }
            let lat: f64 = parts[0].trim().parse().map_err(|_| {
                format!("string `lat` part `{}` is not numeric", parts[0])
            })?;
            let lon: f64 = parts[1].trim().parse().map_err(|_| {
                format!("string `lon` part `{}` is not numeric", parts[1])
            })?;
            let lat = finite_coord("`lat`", lat)?;
            let lon = finite_coord("`lon`", lon)?;
            validate_geo_point_bounds(lat, lon)?;
            Ok((lat, lon))
        }
        Value::Array(items) => {
            // GeoJSON convention: `[lon, lat]` (longitude first). This is
            // the source of so many production bugs that we name the
            // axes loudly in error messages.
            if items.len() != 2 {
                return Err("array form must be `[<lon>, <lat>]`".to_owned());
            }
            let lon = items[0]
                .as_f64()
                .ok_or_else(|| "array form requires numeric `lon` at index 0".to_owned())?;
            let lat = items[1]
                .as_f64()
                .ok_or_else(|| "array form requires numeric `lat` at index 1".to_owned())?;
            let lon = finite_coord("`lon`", lon)?;
            let lat = finite_coord("`lat`", lat)?;
            validate_geo_point_bounds(lat, lon)?;
            Ok((lat, lon))
        }
        _ => Err("expected object, string, or [lon, lat] array".to_owned()),
    }
}

fn validate_geo_point_bounds(lat: f64, lon: f64) -> Result<(), String> {
    if !(-90.0..=90.0).contains(&lat) {
        return Err(format!("`lat` {lat} out of range [-90, 90]"));
    }
    if !(-180.0..=180.0).contains(&lon) {
        return Err(format!("`lon` {lon} out of range [-180, 180]"));
    }
    Ok(())
}

/// Great-circle distance in metres between two `(lat, lon)` points,
/// computed with the haversine formula on a spherical earth of radius
/// 6 371 008.8 m (mean WGS-84 radius — the value ES uses for
/// `geo_distance`).
fn haversine_distance_meters(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_METERS: f64 = 6_371_008.8;
    let to_rad = std::f64::consts::PI / 180.0;
    let dlat = (lat2 - lat1) * to_rad;
    let dlon = (lon2 - lon1) * to_rad;
    let lat1 = lat1 * to_rad;
    let lat2 = lat2 * to_rad;
    let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    EARTH_RADIUS_METERS * c
}

/// Returns `true` when the geo_point stored under `field` in `source`
/// is within `distance_meters` of `(lat, lon)`. Documents whose field
/// is missing or in an unrecognised shape do not match.
pub fn geo_distance_field_matches(
    source: &Value,
    field: &str,
    target_lat: f64,
    target_lon: f64,
    distance_meters: f64,
) -> bool {
    let Some(point) = source.get(field) else {
        return false;
    };
    let Ok((lat, lon)) = parse_geo_point_source(point) else {
        return false;
    };
    haversine_distance_meters(target_lat, target_lon, lat, lon) <= distance_meters
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
        SearchQuery::MatchAll { .. } => true,
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
        SearchQuery::Bool {
            must,
            filter,
            should,
            minimum_should_match,
            ..
        } => {
            // `must` and `filter` clauses must all match.
            if !must.iter().all(|q| query_matches(q, source, mapping)) {
                return false;
            }
            if !filter.iter().all(|q| query_matches(q, source, mapping)) {
                return false;
            }
            // `should` clauses: count matches and compare against MSM.
            // If `should` is empty, MSM is effectively zero and the
            // bucket is satisfied.
            if should.is_empty() {
                return true;
            }
            // ES 7.x default: when only `should` is present, MSM is 1.
            // When `must`/`filter` are present and MSM was not set, MSM
            // is 0 and `should` only contributes scoring. Both behaviours
            // are encoded in `parse_bool_query` so we trust the field.
            let matched = should
                .iter()
                .filter(|q| query_matches(q, source, mapping))
                .count() as u32;
            matched >= *minimum_should_match
        }
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
        SearchQuery::FunctionScore { inner, .. } => query_matches(inner, source, mapping),
        SearchQuery::GeoDistance {
            field,
            lat,
            lon,
            distance_meters,
        } => geo_distance_field_matches(source, field, *lat, *lon, *distance_meters),
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
        lookup_sort_value(&left.doc.source, &clause.field),
        lookup_sort_value(&right.doc.source, &clause.field),
        clause.order,
    )
}

/// Resolve a sort field against the stored `_source` map, with a fallback
/// for ES multi-field sub-fields like `NOM.raw` or `DATE_NAISSANCE.norm`.
/// matchID emits these because their mapping declares
/// `NOM: { type: text, fields: { raw: { type: keyword } } }` — until A13
/// ships the real multi-field types, we alias the sub-field to its
/// parent so the sort order is still deterministic.
fn lookup_sort_value<'a>(source: &'a Value, field: &str) -> Option<&'a Value> {
    let object = source.as_object()?;
    if let Some(value) = object.get(field) {
        return Some(value);
    }
    field.rsplit_once('.').and_then(|(parent, _)| object.get(parent))
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
        SearchQuery::Bool {
            must,
            filter,
            should,
            ..
        } => must
            .iter()
            .chain(filter.iter())
            .chain(should.iter())
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
