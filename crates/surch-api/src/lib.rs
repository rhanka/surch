#![forbid(unsafe_code)]
//! OpenSearch-compatible REST API boundary.

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post, put},
    Router,
};

pub mod aliases;
pub mod analyze;
pub mod bulk;
pub mod cat;
pub mod cluster;
pub mod count;
pub mod document;
pub mod error;
pub mod field_caps;
pub mod index;
pub mod index_template;
pub mod mget;
pub mod msearch;
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
        .route(
            "/_index_template",
            get(index_template::list_index_templates_handler),
        )
        .route(
            "/_index_template/:name",
            get(index_template::get_index_template_handler)
                .put(index_template::put_index_template_handler)
                .delete(index_template::delete_index_template_handler),
        )
        .route(
            "/:index/_mapping",
            get(index::mapping_handler)
                .put(index::put_mapping_handler)
                .post(index::put_mapping_handler),
        )
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
        .route(
            "/_msearch",
            post(msearch::msearch_state_handler)
                .get(msearch::msearch_state_handler)
                .layer(DefaultBodyLimit::max(BULK_BODY_LIMIT_BYTES)),
        )
        .route(
            "/:index/_msearch",
            post(msearch::index_msearch_state_handler)
                .get(msearch::index_msearch_state_handler)
                .layer(DefaultBodyLimit::max(BULK_BODY_LIMIT_BYTES)),
        )
        .route(
            "/_field_caps",
            post(field_caps::field_caps_state_handler).get(field_caps::field_caps_state_handler),
        )
        .route(
            "/:index/_field_caps",
            post(field_caps::index_field_caps_state_handler)
                .get(field_caps::index_field_caps_state_handler),
        )
        .route(
            "/_analyze",
            post(analyze::analyze_state_handler).get(analyze::analyze_state_handler),
        )
        .route(
            "/:index/_analyze",
            post(analyze::index_analyze_state_handler).get(analyze::index_analyze_state_handler),
        )
        .route("/_cluster/health", get(cluster::cluster_health_handler))
        .route(
            "/_cluster/health/:index",
            get(cluster::cluster_health_index_handler),
        )
        .route("/_cat/indices", get(cat::cat_indices_handler))
        .route("/_cat/health", get(cat::cat_health_handler))
        .route("/_cat/aliases", get(cat::cat_aliases_handler))
        .route("/_cat/aliases/:name", get(cat::cat_aliases_by_name_handler))
        .route("/_cat/count", get(cat::cat_count_handler))
        .route("/_cat/count/:index", get(cat::cat_count_index_handler))
        .route(
            "/_aliases",
            post(aliases::aliases_state_handler).put(aliases::aliases_state_handler),
        )
        .route("/_alias", get(aliases::list_all_aliases_handler))
        .route("/_alias/:name", get(aliases::list_alias_by_name_handler))
        .route("/:index/_alias", get(aliases::list_index_aliases_handler))
        .route(
            "/:index/_alias/:name",
            get(aliases::get_index_alias_handler)
                .put(aliases::put_index_alias_handler)
                .delete(aliases::delete_index_alias_handler),
        )
        .with_state(state::AppState::default())
}

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch-compatible API";
