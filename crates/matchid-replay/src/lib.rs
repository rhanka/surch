//! matchID replay parser (gap B1 phase 3).
//!
//! Pure parser library for the curated matchID replay manifests
//! committed under `tests/matchid_compat/replays/`. Extracted from
//! `crates/surch-api/tests/matchid_compat.rs` so the existing
//! compatibility harness AND the future `b1_oracle` driver can share a
//! single source of truth for the on-disk schema.
//!
//! Each manifest declares a top-level [`Replay`] (name + dataset +
//! requests). Each [`Request`] entry wraps an HTTP [`RequestSpec`]
//! (method, path, optional JSON or NDJSON body) and an optional
//! [`Expected`] critical-shape assertion (`hits.total.value` and
//! `hits.hits[0]._id`). The `skip` field, when present, references an
//! active gap (e.g. `gap-A4`, `gap-A6`) and excludes the entry from
//! execution — see the matchid_compat header for the lifecycle.

use std::fs;

use serde::Deserialize;
use serde_json::Value;

/// Top-level replay manifest as serialised in
/// `tests/matchid_compat/replays/<name>.json`.
#[derive(Debug, Deserialize)]
pub struct Replay {
    pub name: String,
    pub dataset: String,
    pub requests: Vec<Request>,
}

/// Single replay entry: HTTP request spec, expected critical shape,
/// and the optional `skip` gap reference.
#[derive(Debug, Deserialize)]
pub struct Request {
    pub name: String,
    pub request: RequestSpec,
    pub expected: Option<Expected>,
    #[serde(default)]
    pub skip: Option<String>,
}

/// HTTP request as serialised in the manifest. Exactly one of `body`
/// or `body_ndjson` may be set; both being present is rejected at
/// execution time by the harness.
#[derive(Debug, Deserialize)]
pub struct RequestSpec {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body: Option<Value>,
    #[serde(default)]
    pub body_ndjson: Option<Vec<Value>>,
}

/// Expected critical-shape assertions for a non-skipped entry.
#[derive(Debug, Deserialize)]
pub struct Expected {
    #[serde(rename = "hits.total.value")]
    pub total_value: u64,
    #[serde(rename = "hits.hits[0]._id")]
    pub top_id: Option<String>,
}

/// Errors raised by [`load_replay`].
#[derive(Debug)]
pub enum ReplayError {
    Io(std::io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::Io(err) => write!(f, "replay io error: {err}"),
            ReplayError::Parse(err) => write!(f, "replay parse error: {err}"),
        }
    }
}

impl std::error::Error for ReplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReplayError::Io(err) => Some(err),
            ReplayError::Parse(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for ReplayError {
    fn from(err: std::io::Error) -> Self {
        ReplayError::Io(err)
    }
}

impl From<serde_json::Error> for ReplayError {
    fn from(err: serde_json::Error) -> Self {
        ReplayError::Parse(err)
    }
}

/// Read and parse the JSON manifest at `path` into a [`Replay`].
pub fn load_replay(path: &str) -> Result<Replay, ReplayError> {
    let raw = fs::read_to_string(path)?;
    let replay = serde_json::from_str(&raw)?;
    Ok(replay)
}
