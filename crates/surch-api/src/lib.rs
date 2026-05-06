#![forbid(unsafe_code)]
//! OpenSearch-compatible REST API boundary.

use axum::{
    routing::{get, post},
    Router,
};

pub mod bulk;
pub mod count;
pub mod error;
pub mod root;

pub use bulk::{parse_bulk_ndjson, BulkOperation, BulkParseError};
pub use error::OpenSearchError;

/// Build the P0 OpenSearch-compatible API router.
pub fn app_router() -> Router {
    Router::new()
        .route("/", get(root::root_handler))
        .route("/_bulk", post(bulk::bulk_handler))
        .route("/:index/_count", post(count::count_handler))
}

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch-compatible API";
