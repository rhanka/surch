//! OpenSearch-compatible composable index template endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};
use surch_index::mapping::IndexMapping;

use crate::{
    index::AcknowledgedResponse,
    state::{AppState, StoredIndexTemplate},
    OpenSearchError,
};

#[derive(Clone, Debug, PartialEq, Serialize)]
struct IndexTemplatesResponse {
    pub index_templates: Vec<IndexTemplateEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct IndexTemplateEntry {
    pub name: String,
    pub index_template: Value,
}

pub async fn put_index_template_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_template_name(&name) {
        return error.into_response();
    }

    let template = match parse_index_template_request(&body) {
        Ok(template) => template,
        Err(error) => return error.into_response(),
    };

    state.put_index_template(&name, template);
    (
        StatusCode::OK,
        Json(AcknowledgedResponse { acknowledged: true }),
    )
        .into_response()
}

pub async fn get_index_template_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_template_name(&name) {
        return error.into_response();
    }

    let Some(template) = state.index_template(&name) else {
        return missing_index_template_error(&name).into_response();
    };

    let response = IndexTemplatesResponse {
        index_templates: vec![IndexTemplateEntry {
            name,
            index_template: template.index_template,
        }],
    };
    (StatusCode::OK, Json(response)).into_response()
}

pub async fn list_index_templates_handler(State(state): State<AppState>) -> impl IntoResponse {
    let index_templates = state
        .all_index_templates()
        .into_iter()
        .map(|(name, template)| IndexTemplateEntry {
            name,
            index_template: template.index_template,
        })
        .collect();

    (
        StatusCode::OK,
        Json(IndexTemplatesResponse { index_templates }),
    )
        .into_response()
}

pub async fn delete_index_template_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_template_name(&name) {
        return error.into_response();
    }

    if !state.delete_index_template(&name) {
        return missing_index_template_error(&name).into_response();
    }

    (
        StatusCode::OK,
        Json(AcknowledgedResponse { acknowledged: true }),
    )
        .into_response()
}

fn parse_index_template_request(body: &str) -> Result<StoredIndexTemplate, OpenSearchError> {
    if body.trim().is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template request body must be an object",
        ));
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
            "_index_template request body must be an object",
        )
    })?;

    let index_patterns = parse_index_patterns(object.get("index_patterns"))?;
    let priority = parse_priority(object.get("priority"))?;
    let template = parse_template_object(object.get("template"))?;
    let mapping = parse_template_mapping(template)?;
    let aliases = parse_template_aliases(template)?;

    Ok(StoredIndexTemplate {
        index_template: value,
        index_patterns,
        mapping,
        aliases,
        priority,
    })
}

fn parse_index_patterns(value: Option<&Value>) -> Result<Vec<String>, OpenSearchError> {
    let Some(value) = value else {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template request must contain `index_patterns`",
        ));
    };
    let patterns = value.as_array().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template `index_patterns` must be an array",
        )
    })?;
    if patterns.is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template `index_patterns` must not be empty",
        ));
    }

    patterns
        .iter()
        .map(|pattern| {
            let pattern = pattern.as_str().ok_or_else(|| {
                OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "_index_template `index_patterns` entries must be strings",
                )
            })?;
            if pattern.trim().is_empty() {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "_index_template `index_patterns` entries must not be empty",
                ));
            }
            Ok(pattern.to_owned())
        })
        .collect()
}

fn parse_priority(value: Option<&Value>) -> Result<i64, OpenSearchError> {
    let Some(value) = value else {
        return Ok(0);
    };
    let Some(priority) = value.as_i64() else {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template `priority` must be a non-negative integer",
        ));
    };
    if priority < 0 {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template `priority` must be a non-negative integer",
        ));
    }
    Ok(priority)
}

fn parse_template_object(
    template: Option<&Value>,
) -> Result<Option<&serde_json::Map<String, Value>>, OpenSearchError> {
    let Some(template) = template else {
        return Ok(None);
    };
    template.as_object().map(Some).ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template `template` must be an object",
        )
    })
}

fn parse_template_mapping(
    template: Option<&serde_json::Map<String, Value>>,
) -> Result<IndexMapping, OpenSearchError> {
    let Some(template) = template else {
        return Ok(IndexMapping::default());
    };

    for (field, reason) in [
        (
            "settings",
            "_index_template `template.settings` must be an object",
        ),
        (
            "aliases",
            "_index_template `template.aliases` must be an object",
        ),
    ] {
        if let Some(value) = template.get(field) {
            if !value.is_object() {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    reason,
                ));
            }
        }
    }

    let Some(mappings) = template.get("mappings") else {
        return Ok(IndexMapping::default());
    };
    parse_template_mappings_value(mappings)
}

fn parse_template_aliases(
    template: Option<&serde_json::Map<String, Value>>,
) -> Result<Vec<String>, OpenSearchError> {
    let Some(template) = template else {
        return Ok(Vec::new());
    };
    let Some(aliases) = template.get("aliases") else {
        return Ok(Vec::new());
    };
    let aliases = aliases.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template `template.aliases` must be an object",
        )
    })?;

    let mut names = Vec::new();
    for (alias, body) in aliases {
        if alias.trim().is_empty() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_index_template `template.aliases` names must not be empty",
            ));
        }
        if !body.is_object() {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("_index_template `template.aliases.{alias}` must be an object"),
            ));
        }
        names.push(alias.clone());
    }
    Ok(names)
}

fn parse_template_mappings_value(mappings: &Value) -> Result<IndexMapping, OpenSearchError> {
    let mappings = mappings.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template `template.mappings` must be an object",
        )
    })?;

    let properties = if let Some(properties) = mappings.get("properties") {
        parse_properties_object(Some(properties), "template.mappings.properties")?
    } else if matches!(mappings.get("_doc"), Some(Value::Object(_))) {
        parse_properties_object(
            mappings
                .get("_doc")
                .and_then(|doc_mapping| doc_mapping.get("properties")),
            "template.mappings._doc.properties",
        )?
    } else if mappings.is_empty() {
        json!({})
    } else {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_index_template `template.mappings` body is invalid",
        ));
    };

    IndexMapping::from_properties_value(&properties).map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "mapper_parsing_exception",
            error.to_string(),
        )
    })
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

fn validate_index_template_name(name: &str) -> Result<(), OpenSearchError> {
    if name.trim().is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "index template name must not be empty",
        ));
    }
    Ok(())
}

fn missing_index_template_error(name: &str) -> OpenSearchError {
    OpenSearchError::new(
        StatusCode::NOT_FOUND,
        "index_template_missing_exception",
        format!("index_template [{name}] missing"),
    )
}
