//! Replay runner primitives for offline OpenSearch oracle checks.

use serde_json::Value;
use thiserror::Error;

use crate::{
    normalize::{compare_json, JsonCompareError},
    replay::{ReplayManifest, ReplayRequest},
};

#[derive(Debug, Clone, PartialEq)]
pub struct OracleResponse {
    pub status: u16,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayRunReport {
    pub manifest_name: String,
    pub dataset: String,
    pub steps: Vec<ReplayStepReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayStepReport {
    pub request_name: String,
    pub status: u16,
}

#[derive(Debug, Error, PartialEq)]
pub enum ReplayRunError {
    #[error("executor failed for replay request `{request_name}`: {message}")]
    Executor {
        request_name: String,
        message: String,
    },
    #[error(
        "status mismatch for replay request `{request_name}`: expected {expected}, got {actual}"
    )]
    StatusMismatch {
        request_name: String,
        expected: u16,
        actual: u16,
    },
    #[error("replay request `{request_name}` expected a response body")]
    MissingResponseBody { request_name: String },
    #[error("response mismatch for replay request `{request_name}`: {source}")]
    ResponseMismatch {
        request_name: String,
        #[source]
        source: JsonCompareError,
    },
}

pub fn run_replay<F>(
    manifest: &ReplayManifest,
    mut execute: F,
) -> Result<ReplayRunReport, ReplayRunError>
where
    F: FnMut(&ReplayRequest) -> Result<OracleResponse, String>,
{
    let mut steps = Vec::with_capacity(manifest.requests.len());
    let normalize_config = manifest.comparison.to_normalize_config();

    for request in &manifest.requests {
        let response = execute(request).map_err(|error| ReplayRunError::Executor {
            request_name: request.name.clone(),
            message: error,
        })?;

        if response.status != request.expected_status {
            return Err(ReplayRunError::StatusMismatch {
                request_name: request.name.clone(),
                expected: request.expected_status,
                actual: response.status,
            });
        }

        if let Some(expected_response) = &request.expected_response {
            let actual_response =
                response
                    .body
                    .as_ref()
                    .ok_or_else(|| ReplayRunError::MissingResponseBody {
                        request_name: request.name.clone(),
                    })?;
            compare_json(expected_response, actual_response, &normalize_config).map_err(
                |source| ReplayRunError::ResponseMismatch {
                    request_name: request.name.clone(),
                    source,
                },
            )?;
        }

        steps.push(ReplayStepReport {
            request_name: request.name.clone(),
            status: response.status,
        });
    }

    Ok(ReplayRunReport {
        manifest_name: manifest.name.clone(),
        dataset: manifest.dataset.clone(),
        steps,
    })
}
