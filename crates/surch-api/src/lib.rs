#![forbid(unsafe_code)]
//! OpenSearch-compatible REST API boundary.

use axum::{
    routing::{get, post, put},
    Router,
};

pub mod bulk;
pub mod count;
pub mod document;
pub mod error;
pub mod root;
pub mod search;

pub use bulk::{parse_bulk_ndjson, BulkOperation, BulkParseError};
pub use error::OpenSearchError;

/// Build the P0 OpenSearch-compatible API router.
pub fn app_router() -> Router {
    Router::new()
        .route("/", get(root::root_handler))
        .route("/_bulk", post(bulk::bulk_handler))
        .route(
            "/:index/_doc/:id",
            put(document::document_handler).post(document::document_handler),
        )
        .route("/:index/_count", post(count::count_handler))
        .route(
            "/:index/_search",
            post(search::search_handler).get(search::search_handler),
        )
}

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch-compatible API";
