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

    if state.index_exists(&index) {
        return OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "resource_already_exists_exception",
            format!("index [{index}] already exists"),
        )
        .into_response();
    }

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

    if !state.index_exists(&index) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{index}] missing"),
        )
        .into_response();
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

pub async fn put_mapping_handler(
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

    let properties = match parse_put_mapping_request(&body) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let mapping = match IndexMapping::from_properties_value(&properties) {
        Ok(mapping) => mapping,
        Err(error) => {
            return OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "mapper_parsing_exception",
                error.to_string(),
            )
            .into_response();
        }
    };

    let fields: Vec<_> = mapping
        .fields()
        .map(|(field, mapping)| (field.to_owned(), *mapping))
        .collect();
    if let Err(reason) = state.merge_field_mappings(&index, &fields) {
        return OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "illegal_argument_exception",
            reason,
        )
        .into_response();
    }

    (
        StatusCode::OK,
        Json(AcknowledgedResponse { acknowledged: true }),
    )
        .into_response()
}

fn parse_put_mapping_request(body: &str) -> Result<Value, OpenSearchError> {
    if body.trim().is_empty() {
        return Ok(json!({}));
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
            "_mapping request body must be an object",
        )
    })?;
    if let Some(properties) = object.get("properties") {
        parse_properties_object(Some(properties), "properties")
    } else {
        // Allow the request body to be the properties map directly.
        Ok(Value::Object(object.clone()))
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

    if let Some(settings) = object.get("settings") {
        if !settings.is_object() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "index request `settings` must be an object",
            ));
        }
    }

    if let Some(aliases) = object.get("aliases") {
        if !aliases.is_object() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "index request `aliases` must be an object",
            ));
        }
    }

    let mapping = match object.get("mappings") {
        Some(Value::Object(mappings)) => {
            if let Some(properties) = mappings.get("properties") {
                parse_properties_object(Some(properties), "mappings.properties")?
            } else if matches!(mappings.get("_doc"), Some(Value::Object(_doc_mapping))) {
                parse_properties_object(
                    mappings
                        .get("_doc")
                        .and_then(|doc_mapping| doc_mapping.get("properties")),
                    "mappings._doc.properties",
                )?
            } else if mappings.is_empty() {
                json!({})
            } else {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "index request `mappings` body is invalid",
                ));
            }
        }
        Some(_) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "index request `mappings` must be an object",
            ));
        }
        None => json!({}),
    };
    let mapping = IndexMapping::from_properties_value(&mapping).map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "mapper_parsing_exception",
            error.to_string(),
        )
    })?;

    Ok(CreateIndexRequest { mapping })
}

fn parse_properties_object(
    properties: Option<&Value>,
    context: &str,
) -> Result<Value, OpenSearchError> {
    if let Some(properties) = properties {
        if !properties.is_object() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("{context} must be an object"),
            ));
        }

        return Ok(properties.clone());
    }

    Ok(json!({}))
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
