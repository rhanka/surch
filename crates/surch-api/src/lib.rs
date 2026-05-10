#![forbid(unsafe_code)]
//! OpenSearch-compatible REST API boundary.

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post, put},
    Router,
};

pub mod bulk;
pub mod count;
pub mod document;
pub mod error;
pub mod index;
pub mod mget;
pub mod root;
pub mod search;
pub mod state;

pub use bulk::{parse_bulk_ndjson, BulkOperation, BulkParseError};
pub use error::OpenSearchError;

const BULK_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// Build the P0 OpenSearch-compatible API router.
pub fn app_router() -> Router {
    Router::new()
        .route("/", get(root::root_handler))
        .route(
            "/_bulk",
            post(bulk::bulk_state_handler)
                .put(bulk::bulk_state_handler)
                .layer(DefaultBodyLimit::max(BULK_BODY_LIMIT_BYTES)),
        )
        .route("/_mapping", get(index::mappings_handler))
        .route("/:index/_mapping", get(index::mapping_handler))
        .route(
            "/:index",
            put(index::create_index_handler).delete(index::delete_index_handler),
        )
        .route(
            "/:index/_bulk",
            post(bulk::index_bulk_state_handler)
                .put(bulk::index_bulk_state_handler)
                .layer(DefaultBodyLimit::max(BULK_BODY_LIMIT_BYTES)),
        )
        .route(
            "/:index/_doc/:id",
            put(document::document_handler).post(document::document_handler),
        )
        .route("/:index/_refresh", post(index::refresh_index_handler))
        .route(
            "/:index/_count",
            post(count::count_handler).get(count::count_handler),
        )
        .route(
            "/:index/_search",
            post(search::search_handler).get(search::search_handler),
        )
        .route(
            "/_mget",
            post(mget::mget_state_handler).get(mget::mget_state_handler),
        )
        .route(
            "/:index/_mget",
            post(mget::index_mget_state_handler).get(mget::index_mget_state_handler),
        )
        .with_state(state::AppState::default())
}

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch-compatible API";
