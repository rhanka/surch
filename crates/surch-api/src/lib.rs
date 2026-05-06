#![forbid(unsafe_code)]
//! OpenSearch-compatible REST API boundary.

use axum::{routing::post, Router};

pub mod bulk;

pub use bulk::{parse_bulk_ndjson, BulkOperation, BulkParseError};

/// Build the P0 OpenSearch-compatible API router.
pub fn app_router() -> Router {
    Router::new().route("/_bulk", post(bulk::bulk_handler))
}

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch-compatible API";
