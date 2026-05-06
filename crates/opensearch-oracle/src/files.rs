//! Fixture file loading helpers for OpenSearch parity fixtures.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::de::DeserializeOwned;
use thiserror::Error;

/// Root directory used to resolve OpenSearch oracle fixture files.
#[derive(Debug, Clone)]
pub struct FixtureRoot {
    root: PathBuf,
}

/// Errors returned while validating and loading fixture files.
#[derive(Debug, Error)]
pub enum FixtureFileError {
    #[error("invalid fixture path `{path}`: {reason}")]
    InvalidPath { path: String, reason: &'static str },
    #[error("failed to read fixture `{path}`")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse fixture JSON `{path}`")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl FixtureRoot {
    /// Create a fixture root from a directory path.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve a relative fixture path while rejecting traversal outside the root.
    pub fn resolve_relative(&self, path: impl AsRef<Path>) -> Result<PathBuf, FixtureFileError> {
        let path = path.as_ref();
        validate_relative_path(path)?;

        let root = self.normalized_root();
        let resolved = root.join(path);
        if !resolved.starts_with(&root) {
            return Err(invalid_path(path, "resolved path escapes fixture root"));
        }

        Ok(resolved)
    }

    /// Read a UTF-8 text fixture.
    pub fn read_text(&self, path: impl AsRef<Path>) -> Result<String, FixtureFileError> {
        let resolved = self.resolve_relative(path)?;
        fs::read_to_string(&resolved).map_err(|source| FixtureFileError::Io {
            path: resolved,
            source,
        })
    }

    /// Read and deserialize a JSON fixture.
    pub fn read_json<T>(&self, path: impl AsRef<Path>) -> Result<T, FixtureFileError>
    where
        T: DeserializeOwned,
    {
        let resolved = self.resolve_relative(path)?;
        let text = fs::read_to_string(&resolved).map_err(|source| FixtureFileError::Io {
            path: resolved.clone(),
            source,
        })?;

        serde_json::from_str(&text).map_err(|source| FixtureFileError::Json {
            path: resolved,
            source,
        })
    }

    fn normalized_root(&self) -> PathBuf {
        normalize_components(&self.root)
    }
}

fn validate_relative_path(path: &Path) -> Result<(), FixtureFileError> {
    if path.as_os_str().is_empty() {
        return Err(invalid_path(path, "path must not be empty"));
    }

    if path.is_absolute() {
        return Err(invalid_path(path, "absolute paths are not allowed"));
    }

    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::ParentDir => {
                return Err(invalid_path(
                    path,
                    "parent directory traversal is not allowed",
                ));
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(invalid_path(path, "absolute paths are not allowed"));
            }
        }
    }

    Ok(())
}

fn normalize_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                normalized.pop();
            }
            Component::RootDir | Component::Prefix(_) => normalized.push(component.as_os_str()),
        }
    }

    normalized
}

fn invalid_path(path: &Path, reason: &'static str) -> FixtureFileError {
    FixtureFileError::InvalidPath {
        path: path.display().to_string(),
        reason,
    }
}
