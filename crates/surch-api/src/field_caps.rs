//! OpenSearch-compatible `/_field_caps` and `/{index}/_field_caps` endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

use crate::{index::validate_index_name, state::AppState, OpenSearchError};

#[derive(Debug, Clone)]
struct FieldCapsRequest {
    fields: Option<Vec<String>>,
}

/// Axum handler for `GET|POST /_field_caps` (no path index).
pub async fn field_caps_state_handler(
    State(state): State<AppState>,
    body: String,
) -> impl IntoResponse {
    handle_field_caps(&state, None, &body)
}

/// Axum handler for `GET|POST /{index}/_field_caps`.
pub async fn index_field_caps_state_handler(
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
    handle_field_caps(&state, Some(index.as_str()), &body)
}

fn handle_field_caps(
    state: &AppState,
    requested_index: Option<&str>,
    body: &str,
) -> axum::response::Response {
    let request = match parse_field_caps_request(body) {
        Ok(request) => request,
        Err(error) => return error.into_response(),
    };

    let mappings = match requested_index {
        Some(index) => match state.mapping(index) {
            Some(mapping) => BTreeMap::from_iter([(index.to_owned(), mapping)]),
            None => BTreeMap::new(),
        },
        None => state.all_mappings(),
    };

    let indices: Vec<String> = mappings.keys().cloned().collect();
    let fields = collect_field_caps(&mappings, request.fields.as_deref());

    (
        StatusCode::OK,
        Json(json!({
            "indices": indices,
            "fields": fields,
        })),
    )
        .into_response()
}

fn parse_field_caps_request(body: &str) -> Result<FieldCapsRequest, OpenSearchError> {
    if body.trim().is_empty() {
        return Ok(FieldCapsRequest { fields: None });
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
            "_field_caps request body must be an object",
        )
    })?;

    let fields = match object.get("fields") {
        None => None,
        Some(Value::Array(items)) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(text) if !text.is_empty() => names.push(text.clone()),
                    Value::String(_) => {
                        return Err(OpenSearchError::new(
                            StatusCode::BAD_REQUEST,
                            "parsing_exception",
                            "_field_caps `fields` entries must not be empty",
                        ));
                    }
                    _ => {
                        return Err(OpenSearchError::new(
                            StatusCode::BAD_REQUEST,
                            "parsing_exception",
                            "_field_caps `fields` entries must be strings",
                        ));
                    }
                }
            }
            Some(names)
        }
        Some(Value::String(text)) if !text.is_empty() => Some(vec![text.clone()]),
        _ => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_field_caps `fields` must be a string or array of strings",
            ));
        }
    };

    Ok(FieldCapsRequest { fields })
}

fn collect_field_caps(
    mappings: &BTreeMap<String, Value>,
    requested_fields: Option<&[String]>,
) -> Map<String, Value> {
    let want_all = requested_fields
        .map(|fields| fields.iter().any(|field| field == "*"))
        .unwrap_or(true);
    let wanted: Option<BTreeSet<&str>> = if want_all {
        None
    } else {
        requested_fields.map(|fields| fields.iter().map(String::as_str).collect())
    };

    let mut result: Map<String, Value> = Map::new();
    for mapping in mappings.values() {
        let Some(properties) = mapping.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for (field, definition) in properties {
            if let Some(filter) = wanted.as_ref() {
                if !filter.contains(field.as_str()) {
                    continue;
                }
            }
            let field_type = definition
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("keyword");
            let entry = result
                .entry(field.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            let entry_object = entry
                .as_object_mut()
                .expect("field caps entry should be an object");
            entry_object
                .entry(field_type.to_owned())
                .or_insert_with(|| build_field_cap_entry(field_type));
        }
    }
    result
}

fn build_field_cap_entry(field_type: &str) -> Value {
    let aggregatable = !matches!(field_type, "text" | "object" | "array");
    json!({
        "type": field_type,
        "metadata_field": false,
        "searchable": true,
        "aggregatable": aggregatable,
    })
}
