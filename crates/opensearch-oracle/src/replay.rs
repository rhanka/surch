//! Replay manifest parsing for OpenSearch parity requests.

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReplayManifest {
    pub name: String,
    pub dataset: String,
    pub requests: Vec<ReplayRequest>,
}

impl ReplayManifest {
    pub fn from_json_str(json: &str) -> Result<Self, ReplayManifestError> {
        let manifest = serde_json::from_str(json)?;
        validate_manifest(manifest)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReplayRequest {
    pub name: String,
    pub method: HttpMethod,
    pub path: String,
    pub body: Option<Value>,
    pub expected_status: u16,
    pub expected_response: Option<Value>,
}

#[derive(Debug, Error, PartialEq)]
pub enum ReplayManifestError {
    #[error("invalid replay manifest json: {0}")]
    Json(String),
    #[error("replay manifest name must not be empty")]
    EmptyName,
    #[error("replay manifest dataset must not be empty")]
    EmptyDataset,
    #[error("replay manifest must contain at least one request")]
    EmptyRequests,
    #[error("replay request at index {index} name must not be empty")]
    EmptyRequestName { index: usize },
    #[error("replay request at index {index} path must start with '/'")]
    InvalidPath { index: usize },
    #[error("replay request at index {index} expected_status must be in 100..=599, got {status}")]
    InvalidExpectedStatus { index: usize, status: u16 },
    #[error("replay request at index {index} expected_response must not be empty when present")]
    EmptyExpectedResponse { index: usize },
}

impl From<serde_json::Error> for ReplayManifestError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

fn validate_manifest(manifest: ReplayManifest) -> Result<ReplayManifest, ReplayManifestError> {
    if manifest.name.trim().is_empty() {
        return Err(ReplayManifestError::EmptyName);
    }

    if manifest.dataset.trim().is_empty() {
        return Err(ReplayManifestError::EmptyDataset);
    }

    if manifest.requests.is_empty() {
        return Err(ReplayManifestError::EmptyRequests);
    }

    for (index, request) in manifest.requests.iter().enumerate() {
        if request.name.trim().is_empty() {
            return Err(ReplayManifestError::EmptyRequestName { index });
        }

        if !request.path.starts_with('/') {
            return Err(ReplayManifestError::InvalidPath { index });
        }

        if !(100..=599).contains(&request.expected_status) {
            return Err(ReplayManifestError::InvalidExpectedStatus {
                index,
                status: request.expected_status,
            });
        }

        if request
            .expected_response
            .as_ref()
            .is_some_and(is_empty_json_value)
        {
            return Err(ReplayManifestError::EmptyExpectedResponse { index });
        }
    }

    Ok(manifest)
}

fn is_empty_json_value(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.is_empty(),
        Value::Object(fields) => fields.is_empty(),
        Value::String(text) => text.is_empty(),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}
