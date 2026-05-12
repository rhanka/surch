//! OpenSearch-compatible component template endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use serde_json::Value;

use crate::{
    index::AcknowledgedResponse,
    index_template::{
        parse_template_aliases, parse_template_mapping, parse_template_object,
        parse_template_settings, validate_template_name,
    },
    state::{AppState, StoredComponentTemplate},
    OpenSearchError,
};

#[derive(Clone, Debug, PartialEq, Serialize)]
struct ComponentTemplatesResponse {
    pub component_templates: Vec<ComponentTemplateEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct ComponentTemplateEntry {
    pub name: String,
    pub component_template: Value,
}

pub async fn put_component_template_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_component_template_name(&name) {
        return error.into_response();
    }

    let template = match parse_component_template_request(&body) {
        Ok(template) => template,
        Err(error) => return error.into_response(),
    };

    state.put_component_template(&name, template);
    (
        StatusCode::OK,
        Json(AcknowledgedResponse { acknowledged: true }),
    )
        .into_response()
}

pub async fn get_component_template_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_component_template_name(&name) {
        return error.into_response();
    }

    let Some(template) = state.component_template(&name) else {
        return missing_component_template_error(&name).into_response();
    };

    let response = ComponentTemplatesResponse {
        component_templates: vec![ComponentTemplateEntry {
            name,
            component_template: template.component_template,
        }],
    };
    (StatusCode::OK, Json(response)).into_response()
}

pub async fn list_component_templates_handler(State(state): State<AppState>) -> impl IntoResponse {
    let component_templates = state
        .all_component_templates()
        .into_iter()
        .map(|(name, template)| ComponentTemplateEntry {
            name,
            component_template: template.component_template,
        })
        .collect();

    (
        StatusCode::OK,
        Json(ComponentTemplatesResponse {
            component_templates,
        }),
    )
        .into_response()
}

pub async fn delete_component_template_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_component_template_name(&name) {
        return error.into_response();
    }

    if !state.delete_component_template(&name) {
        return missing_component_template_error(&name).into_response();
    }

    (
        StatusCode::OK,
        Json(AcknowledgedResponse { acknowledged: true }),
    )
        .into_response()
}

fn parse_component_template_request(
    body: &str,
) -> Result<StoredComponentTemplate, OpenSearchError> {
    if body.trim().is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_component_template request body must be an object",
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
            "_component_template request body must be an object",
        )
    })?;
    let template = parse_template_object(object.get("template"))?;
    let mapping = parse_template_mapping(template)?;
    let settings = parse_template_settings(template)?;
    let aliases = parse_template_aliases(template)?;

    Ok(StoredComponentTemplate {
        component_template: value,
        mapping,
        settings,
        aliases,
    })
}

fn validate_component_template_name(name: &str) -> Result<(), OpenSearchError> {
    validate_template_name(name, "component")
}

fn missing_component_template_error(name: &str) -> OpenSearchError {
    OpenSearchError::new(
        StatusCode::NOT_FOUND,
        "component_template_missing_exception",
        format!("component_template [{name}] missing"),
    )
}
