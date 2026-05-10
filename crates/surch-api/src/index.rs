use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::iter::once;

use surch_index::mapping::IndexMapping;

use crate::{state::AppState, OpenSearchError};

const INDEX_NAME_FORBIDDEN_CHARACTERS: [char; 14] = [
    ':', '"', '*', '+', '/', '\\', '|', '?', '#', '>', '<', ',', ' ', '\t',
];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateIndexResponse {
    pub acknowledged: bool,
    pub shards_acknowledged: bool,
    pub index: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AcknowledgedResponse {
    pub acknowledged: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RefreshResponse {
    #[serde(rename = "_shards")]
    pub shards: RefreshShards,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RefreshShards {
    pub total: u64,
    pub successful: u64,
    pub failed: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct MappingsResponse {
    #[serde(rename = "mappings")]
    pub mappings: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct IndexMappingsResponse {
    #[serde(flatten)]
    pub entries: BTreeMap<String, MappingsResponse>,
}

#[derive(Clone, Debug)]
struct CreateIndexRequest {
    pub mapping: IndexMapping,
}

pub async fn create_index_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }

    let request = match parse_create_index_request(&body) {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };

    state.create_index(&index, Some(request.mapping));

    (
        StatusCode::OK,
        Json(CreateIndexResponse {
            acknowledged: true,
            shards_acknowledged: true,
            index,
        }),
    )
        .into_response()
}

pub async fn delete_index_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }

    state.delete_index(&index);

    (
        StatusCode::OK,
        Json(AcknowledgedResponse { acknowledged: true }),
    )
        .into_response()
}

pub async fn refresh_index_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }

    state.refresh_index(&index);

    (
        StatusCode::OK,
        Json(RefreshResponse {
            shards: RefreshShards {
                total: 1,
                successful: 1,
                failed: 0,
            },
        }),
    )
        .into_response()
}

pub async fn mapping_handler(
    State(state): State<AppState>,
    Path(index): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }

    match state.mapping(&index) {
        Some(mapping) => (
            StatusCode::OK,
            Json(IndexMappingsResponse {
                entries: BTreeMap::from_iter(once((index, MappingsResponse { mappings: mapping }))),
            }),
        )
            .into_response(),
        None => OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{index}] missing"),
        )
        .into_response(),
    }
}

pub async fn mappings_handler(State(state): State<AppState>) -> impl IntoResponse {
    let mappings = state.all_mappings();
    if mappings.is_empty() {
        return (StatusCode::OK, Json(json!({}))).into_response();
    }

    let entries = mappings
        .into_iter()
        .map(|(index, mapping)| (index, MappingsResponse { mappings: mapping }))
        .collect();

    (StatusCode::OK, Json(IndexMappingsResponse { entries })).into_response()
}

fn parse_create_index_request(body: &str) -> Result<CreateIndexRequest, OpenSearchError> {
    if body.trim().is_empty() {
        return Ok(CreateIndexRequest {
            mapping: IndexMapping::default(),
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
            "index request body must be an object",
        )
    })?;

    for key in object.keys() {
        if !matches!(key.as_str(), "settings" | "mappings" | "aliases") {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("create index body contains unsupported field `{key}`"),
            ));
        }
    }

    let mapping = object
        .get("mappings")
        .and_then(Value::as_object)
        .and_then(|mappings| mappings.get("properties"))
        .and_then(Value::as_object)
        .map(|properties| Value::Object(properties.clone()))
        .unwrap_or_else(|| json!({}));
    let mapping = IndexMapping::from_properties_value(&mapping).map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "mapper_parsing_exception",
            error.to_string(),
        )
    })?;

    Ok(CreateIndexRequest { mapping })
}

pub fn validate_index_name(index: &str) -> Result<(), OpenSearchError> {
    let trimmed = index.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('_')
        || trimmed.starts_with('-')
        || trimmed.contains(',')
        || trimmed
            .chars()
            .any(|character| INDEX_NAME_FORBIDDEN_CHARACTERS.contains(&character))
        || trimmed
            .chars()
            .any(|character| character.is_ascii_uppercase())
    {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "invalid index name",
        ));
    }

    Ok(())
}
