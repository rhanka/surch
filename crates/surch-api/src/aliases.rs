//! OpenSearch-compatible `_aliases` and `/{index}/_alias[/{name}]` endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Map, Value};

use crate::{index::validate_index_name, state::AppState, OpenSearchError};

#[derive(Debug, Clone)]
enum AliasAction {
    Add {
        index: String,
        alias: String,
        definition: Value,
    },
    Remove {
        index: String,
        alias: String,
    },
}

/// Axum handler for `POST /_aliases` (atomic add/remove batch).
pub async fn aliases_state_handler(
    State(state): State<AppState>,
    body: String,
) -> impl IntoResponse {
    let actions = match parse_aliases_request(&body) {
        Ok(actions) => actions,
        Err(error) => return error.into_response(),
    };

    for action in &actions {
        match action {
            AliasAction::Add { index, .. } => {
                if !state.index_exists(index) {
                    return OpenSearchError::new(
                        StatusCode::NOT_FOUND,
                        "index_not_found_exception",
                        format!("index [{index}] missing"),
                    )
                    .into_response();
                }
            }
            AliasAction::Remove { .. } => {}
        }
    }

    for action in actions {
        match action {
            AliasAction::Add {
                index,
                alias,
                definition,
            } => {
                state.add_alias_with_definition(&index, &alias, definition);
            }
            AliasAction::Remove { index, alias } => {
                state.remove_alias(&index, &alias);
            }
        }
    }

    (StatusCode::OK, Json(json!({ "acknowledged": true }))).into_response()
}

/// Axum handler for `GET /_alias`.
pub async fn list_all_aliases_handler(State(state): State<AppState>) -> impl IntoResponse {
    let response = build_indices_alias_map(&state, &state.index_names(), None);
    (StatusCode::OK, Json(response)).into_response()
}

/// Axum handler for `GET /_alias/{name}`.
pub async fn list_alias_by_name_handler(
    State(state): State<AppState>,
    Path(alias): Path<String>,
) -> impl IntoResponse {
    if !state.alias_exists(&alias) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "aliases_not_found_exception",
            format!("alias [{alias}] missing"),
        )
        .into_response();
    }
    let indices = state.indices_for_alias(&alias);
    let response = build_indices_alias_map(&state, &indices, Some(alias.as_str()));
    (StatusCode::OK, Json(response)).into_response()
}

/// Axum handler for `GET /{index}/_alias`.
pub async fn list_index_aliases_handler(
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
    let response = build_indices_alias_map(&state, &[index], None);
    (StatusCode::OK, Json(response)).into_response()
}

/// Axum handler for `GET /{index}/_alias/{name}`.
pub async fn get_index_alias_handler(
    State(state): State<AppState>,
    Path((index, alias)): Path<(String, String)>,
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
    if !state.aliases_for_index(&index).iter().any(|a| a == &alias) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "aliases_not_found_exception",
            format!("alias [{alias}] missing"),
        )
        .into_response();
    }
    let response = build_indices_alias_map(&state, &[index], Some(alias.as_str()));
    (StatusCode::OK, Json(response)).into_response()
}

/// Axum handler for `PUT /{index}/_alias/{name}`.
pub async fn put_index_alias_handler(
    State(state): State<AppState>,
    Path((index, alias)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }
    if alias.is_empty() {
        return OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "alias name must not be empty",
        )
        .into_response();
    }
    if !state.index_exists(&index) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{index}] missing"),
        )
        .into_response();
    }
    state.add_alias(&index, &alias);
    (StatusCode::OK, Json(json!({ "acknowledged": true }))).into_response()
}

/// Axum handler for `DELETE /{index}/_alias/{name}`.
pub async fn delete_index_alias_handler(
    State(state): State<AppState>,
    Path((index, alias)): Path<(String, String)>,
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
    if !state.remove_alias(&index, &alias) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "aliases_not_found_exception",
            format!("alias [{alias}] missing on index [{index}]"),
        )
        .into_response();
    }
    (StatusCode::OK, Json(json!({ "acknowledged": true }))).into_response()
}

fn build_indices_alias_map(
    state: &AppState,
    indices: &[String],
    filter_alias: Option<&str>,
) -> Value {
    let mut root = Map::new();
    for index in indices {
        let mut alias_map = Map::new();
        for (alias, definition) in state.alias_definitions_for_index(index) {
            if let Some(name) = filter_alias {
                if alias != name {
                    continue;
                }
            }
            alias_map.insert(alias, definition);
        }
        let mut entry = Map::new();
        entry.insert("aliases".to_owned(), Value::Object(alias_map));
        root.insert(index.clone(), Value::Object(entry));
    }
    Value::Object(root)
}

fn parse_aliases_request(body: &str) -> Result<Vec<AliasAction>, OpenSearchError> {
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
            "_aliases request body must be an object",
        )
    })?;
    let actions = object.get("actions").ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_aliases request must contain `actions`",
        )
    })?;
    let array = actions.as_array().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_aliases `actions` must be an array",
        )
    })?;
    if array.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_aliases `actions` must not be empty",
        ));
    }
    array.iter().map(parse_alias_action).collect()
}

fn parse_alias_action(value: &Value) -> Result<AliasAction, OpenSearchError> {
    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_aliases action must be an object",
        )
    })?;
    if object.len() != 1 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_aliases action must contain exactly one operation",
        ));
    }
    let (op, body) = object.iter().next().expect("object has one entry");
    let inner = body.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_aliases action body must be an object",
        )
    })?;
    let index = match inner.get("index") {
        Some(Value::String(text)) if !text.is_empty() => text.clone(),
        Some(Value::String(_)) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_aliases action `index` must not be empty",
            ));
        }
        _ => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_aliases action must contain `index`",
            ));
        }
    };
    validate_index_name(&index)?;
    let alias = match inner.get("alias") {
        Some(Value::String(text)) if !text.is_empty() => text.clone(),
        Some(Value::String(_)) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_aliases action `alias` must not be empty",
            ));
        }
        _ => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_aliases action must contain `alias`",
            ));
        }
    };
    match op.as_str() {
        "add" => Ok(AliasAction::Add {
            index,
            alias,
            definition: alias_definition_from_action(inner),
        }),
        "remove" => Ok(AliasAction::Remove { index, alias }),
        unknown => Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            format!("unsupported alias action `{unknown}`"),
        )),
    }
}

fn alias_definition_from_action(inner: &Map<String, Value>) -> Value {
    let mut definition = Map::new();
    for (key, value) in inner {
        if !matches!(key.as_str(), "index" | "alias") {
            definition.insert(key.clone(), value.clone());
        }
    }
    Value::Object(definition)
}
