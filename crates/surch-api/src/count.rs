use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use surch_index::mapping::IndexMapping;

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
            let plafond = plafond_de_comptage(request.track_total_hits.as_ref());
            // Plafonner CHAQUE index à `limit` conserve la valeur rapportée :
            // en notant `S = Σ c_i` et `S' = Σ min(c_i, limit)`, on a
            // `min(S', limit) == min(S, limit)` — si `S <= limit` aucun
            // plafond ne se déclenche et `S' == S`, sinon `S' >= limit` donc
            // les deux minimums valent `limit`.
            let count: u64 = indices
                .iter()
                .map(|index| count_matches(&state, index, &request, plafond))
                .sum();
            let count = valeur_rapportee(count, request.track_total_hits.as_ref());

            (StatusCode::OK, Json(build_count_response(count))).into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// Chantier C1 — plafond de comptage AUTORISÉ par un mode `track_total_hits`.
///
/// `None` = comptage EXHAUSTIF obligatoire. C'est le cas de `Exact`
/// (`track_total_hits: true`), où toute terminaison anticipée du comptage est
/// interdite, mais AUSSI du mode par défaut et de `Disabled` : contrairement
/// à `_search`, qui plafonne à 10 000 par défaut, `_count` rend un total
/// exact dans ces deux modes. S'y arrêter tôt changerait la réponse.
///
/// Seul `UpTo(limit)` plafonne la valeur rapportée
/// ([`valeur_rapportee`]) — c'est donc le seul mode où cesser de compter est
/// invisible du client.
fn plafond_de_comptage(mode: Option<&TrackTotalHits>) -> Option<u64> {
    match mode {
        Some(TrackTotalHits::UpTo(limit)) => Some(*limit),
        Some(TrackTotalHits::Disabled) | Some(TrackTotalHits::Exact) | None => None,
    }
}

/// Valeur `count` rapportée par `_count` à partir du compte accumulé.
/// Inchangée par C1 : c'est le contrat observable.
fn valeur_rapportee(count: u64, mode: Option<&TrackTotalHits>) -> u64 {
    match mode {
        Some(TrackTotalHits::UpTo(limit)) => count.min(*limit),
        Some(TrackTotalHits::Disabled) | Some(TrackTotalHits::Exact) | None => count,
    }
}

/// Compte les éléments d'un itérateur, en cessant de le tirer dès le plafond
/// atteint. `None` = comptage exhaustif, le seul comportement autorisé quand
/// la valeur rapportée n'est pas plafonnée.
fn compter_avec_plafond<I: Iterator>(iterateur: I, plafond: Option<u64>) -> u64 {
    match plafond {
        None => iterateur.count() as u64,
        Some(plafond) => {
            let plafond = usize::try_from(plafond).unwrap_or(usize::MAX);
            iterateur.take(plafond).count() as u64
        }
    }
}

fn count_matches(
    state: &AppState,
    index: &str,
    request: &CountRequest,
    plafond: Option<u64>,
) -> u64 {
    match request.query.as_ref() {
        None => state.count(index),
        Some(query) => count_query_matches(state, index, query, plafond),
    }
}

fn count_query_matches(
    state: &AppState,
    index: &str,
    query: &CountQuery,
    plafond: Option<u64>,
) -> u64 {
    let mapping = state.index_mapping(index).unwrap_or_default();
    match query {
        // Ces deux formes lisent un compteur déjà tenu : il n'y a aucun
        // parcours à interrompre, le plafond n'aurait rien à économiser.
        CountQuery::MatchAll => state.count(index),
        CountQuery::Term { field, value } => state.term_matches_count(index, field, value) as u64,
        CountQuery::BoolMust(clauses) => {
            if let Some(documents) = intersect_term_clauses(state, index, clauses) {
                documents.len() as u64
            } else {
                let documents = state.documents(index);
                compter_avec_plafond(
                    documents
                        .into_iter()
                        .filter(|document| query_matches(query, &document.source, &mapping)),
                    plafond,
                )
            }
        }
        CountQuery::Range { .. }
        | CountQuery::Exists { .. }
        | CountQuery::Terms { .. }
        | CountQuery::Prefix { .. }
        | CountQuery::Wildcard { .. }
        | CountQuery::MultiMatch { .. } => compter_avec_plafond(
            state
                .documents(index)
                .into_iter()
                .filter(|document| query_matches(query, &document.source, &mapping)),
            plafond,
        ),
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

fn query_matches(query: &CountQuery, source: &Value, mapping: &IndexMapping) -> bool {
    match query {
        CountQuery::MatchAll => true,
        CountQuery::Term { field, value } => term_field_matches(source, field, value),
        CountQuery::BoolMust(clauses) => clauses
            .iter()
            .all(|clause| query_matches(clause, source, mapping)),
        CountQuery::Range { field, bounds } => range_field_matches(source, field, bounds, mapping),
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

/// Chantier C1 — terminaison anticipée du COMPTAGE de `_count`.
///
/// Deux invariants, verrouillés contre le comptage exhaustif comme oracle :
///
/// 1. la valeur RAPPORTÉE est identique avec et sans plafond, dans tous les
///    modes de `track_total_hits` ;
/// 2. le mode `Exact` — comme le mode par défaut et `Disabled`, qui rendent
///    eux aussi un total exact sur `_count` — n'obtient JAMAIS de plafond,
///    donc compte toujours exhaustivement.
#[cfg(test)]
mod c1_comptage_tests {
    use serde_json::json;

    use super::{count_query_matches, plafond_de_comptage, valeur_rapportee, CountQuery};
    use crate::search::TrackTotalHits;
    use crate::state::{AppState, DocumentWriteOperation};

    fn etat(index: &str, docs: usize) -> AppState {
        let state = AppState::default();
        state.create_index(index, None, json!({}), Default::default());
        let operations = (0..docs)
            .map(|doc_id| DocumentWriteOperation::Index {
                index: index.to_owned(),
                id: doc_id.to_string(),
                source: json!({ "nom": "martin", "rang": doc_id.to_string() }),
                status: 201,
            })
            .collect();
        state.apply_document_writes(operations);
        state.refresh_index(index);
        state
    }

    fn modes(total: u64) -> Vec<Option<TrackTotalHits>> {
        vec![
            None,
            Some(TrackTotalHits::Exact),
            Some(TrackTotalHits::Disabled),
            Some(TrackTotalHits::UpTo(0)),
            Some(TrackTotalHits::UpTo(1)),
            Some(TrackTotalHits::UpTo(total.saturating_sub(1))),
            Some(TrackTotalHits::UpTo(total)),
            Some(TrackTotalHits::UpTo(total + 1)),
            Some(TrackTotalHits::UpTo(total * 10)),
        ]
    }

    /// `exists` passe par le balayage plafonnable ; `term` et `match_all`
    /// lisent un compteur déjà tenu et ne sont jamais plafonnés.
    fn requetes() -> Vec<CountQuery> {
        vec![
            CountQuery::MatchAll,
            CountQuery::Term {
                field: "nom".to_owned(),
                value: "martin".to_owned(),
            },
            CountQuery::Exists {
                field: "nom".to_owned(),
            },
            CountQuery::Prefix {
                field: "nom".to_owned(),
                value: "mar".to_owned(),
            },
        ]
    }

    #[test]
    fn la_valeur_rapportee_est_identique_avec_et_sans_plafond() {
        let index = "c1-count";
        let total = 250u64;
        let state = etat(index, total as usize);
        for query in requetes() {
            // Oracle : le comptage exhaustif, celui d'avant C1.
            let exhaustif = count_query_matches(&state, index, &query, None);
            assert!(exhaustif > 0, "oracle exhaustif vide pour {query:?}");
            for mode in modes(total) {
                let plafond = plafond_de_comptage(mode.as_ref());
                let plafonne = count_query_matches(&state, index, &query, plafond);
                assert_eq!(
                    valeur_rapportee(plafonne, mode.as_ref()),
                    valeur_rapportee(exhaustif, mode.as_ref()),
                    "valeur rapportée modifiée par le plafond ({mode:?}, {query:?})"
                );
            }
        }
    }

    #[test]
    fn les_modes_exhaustifs_n_obtiennent_jamais_de_plafond() {
        for mode in [
            None,
            Some(TrackTotalHits::Exact),
            Some(TrackTotalHits::Disabled),
        ] {
            assert_eq!(
                plafond_de_comptage(mode.as_ref()),
                None,
                "aucun plafond n'est licite dans ce mode : {mode:?}"
            );
        }
        // Seul `UpTo` autorise l'arrêt.
        assert_eq!(
            plafond_de_comptage(Some(&TrackTotalHits::UpTo(42))),
            Some(42)
        );
    }

    #[test]
    fn le_mode_exact_compte_exhaustivement() {
        let index = "c1-count-exact";
        let total = 250u64;
        let state = etat(index, total as usize);
        let query = CountQuery::Exists {
            field: "nom".to_owned(),
        };
        let plafond = plafond_de_comptage(Some(&TrackTotalHits::Exact));
        let compte = count_query_matches(&state, index, &query, plafond);
        assert_eq!(compte, total, "le mode Exact ne doit jamais s'arrêter tôt");
        assert_eq!(
            valeur_rapportee(compte, Some(&TrackTotalHits::Exact)),
            total
        );
    }

    /// Le plafond doit RÉELLEMENT interrompre le balayage, sinon la
    /// terminaison anticipée est prouvée à vide.
    #[test]
    fn le_plafond_interrompt_reellement_le_balayage() {
        let index = "c1-count-plafond";
        let total = 250u64;
        let state = etat(index, total as usize);
        let query = CountQuery::Exists {
            field: "nom".to_owned(),
        };
        let compte = count_query_matches(&state, index, &query, Some(10));
        assert_eq!(
            compte, 10,
            "le balayage doit cesser au plafond, pas compter les {total} documents"
        );
    }
}
