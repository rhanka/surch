//! Dataset manifest parsing for OpenSearch parity fixtures.

use serde::Deserialize;
use thiserror::Error;

/// Dataset replay manifest for OpenSearch compatibility fixtures.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DatasetManifest {
    pub name: String,
    pub description: String,
    pub operations: Vec<DatasetOperation>,
}

impl DatasetManifest {
    /// Parse and validate a JSON dataset manifest.
    pub fn from_json_str(json: &str) -> Result<Self, ManifestValidationError> {
        let manifest: Self = serde_json::from_str(json)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate manifest invariants that cannot be represented by serde alone.
    pub fn validate(&self) -> Result<(), ManifestValidationError> {
        if self.name.trim().is_empty() {
            return Err(ManifestValidationError::EmptyName);
        }

        if self.operations.is_empty() {
            return Err(ManifestValidationError::NoOperations);
        }

        for (index, operation) in self.operations.iter().enumerate() {
            if !operation.path.starts_with('/') {
                return Err(ManifestValidationError::InvalidOperationPath {
                    index,
                    path: operation.path.clone(),
                });
            }

            if operation
                .body
                .as_ref()
                .is_some_and(|body| body.trim().is_empty())
            {
                return Err(ManifestValidationError::InvalidBodyPath { index });
            }

            if let Some(status) = operation.expected_status {
                if !(100..=599).contains(&status) {
                    return Err(ManifestValidationError::InvalidExpectedStatus { index, status });
                }
            }
        }

        Ok(())
    }
}

/// One HTTP operation in a dataset replay sequence.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DatasetOperation {
    pub kind: DatasetOperationKind,
    pub path: String,
    pub body: Option<String>,
    pub expected_status: Option<u16>,
}

/// Supported dataset operation kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetOperationKind {
    CreateIndex,
    Bulk,
    Refresh,
    DeleteIndex,
}

/// Typed parse and validation errors for dataset manifests.
#[derive(Debug, Error)]
pub enum ManifestValidationError {
    #[error("invalid dataset manifest JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("dataset manifest name must not be empty")]
    EmptyName,
    #[error("dataset manifest must contain at least one operation")]
    NoOperations,
    #[error("dataset operation {index} path must start with '/': {path}")]
    InvalidOperationPath { index: usize, path: String },
    #[error("dataset operation {index} body path must not be empty")]
    InvalidBodyPath { index: usize },
    #[error("dataset operation {index} expected_status must be in 100..=599: {status}")]
    InvalidExpectedStatus { index: usize, status: u16 },
}
