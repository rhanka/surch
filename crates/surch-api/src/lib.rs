#![deny(unsafe_code)]
//! OpenSearch-compatible REST API boundary.

use axum::{
    extract::DefaultBodyLimit,
    http::HeaderValue,
    response::Response,
    routing::{get, post, put},
    Router,
};
use std::sync::OnceLock;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

pub mod aliases;
pub mod analyze;
pub mod bulk;
pub mod cat;
pub mod cluster;
pub mod component_template;
pub mod count;
pub mod document;
pub mod error;
pub mod field_caps;
pub mod index;
pub mod index_template;
pub mod metrics;
pub mod mget;
pub mod msearch;
pub mod root;
pub mod scroll;
pub mod search;
pub mod slm;
pub mod snapshot;
pub mod snapshot_es;
pub mod state;
pub mod stats;
pub mod telemetry;
mod topn;

pub use bulk::{parse_bulk_ndjson, BulkItemParseResult, BulkOperation, BulkParseError};
pub use error::OpenSearchError;

const BULK_BODY_LIMIT_BYTES: usize = 16 * 1024 * 1024;
/// Hard cap on `POST /_surch/snapshot/import` body. Matches the
/// internal cap in `snapshot::IMPORT_BODY_CAP_BYTES`; `axum`'s
/// `DefaultBodyLimit` enforces it before the handler sees the bytes,
/// so a hostile uploader cannot exhaust memory.
const SNAPSHOT_IMPORT_BODY_LIMIT_BYTES: usize = 1024 * 1024 * 1024;

/// Bundle of long-lived shared state surfaced by the router builder so
/// the scheduler / tests can drive the same registries the HTTP layer
/// observes. `app_router_with_state` is the explicit entry point used
/// by `main.rs` to also spawn the SLM background task; `app_router()`
/// keeps the simple no-side-effect shape that tests rely on.
#[derive(Clone, Default)]
pub struct AppRouterState {
    pub app: state::AppState,
    pub snapshot_repositories: snapshot_es::SnapshotRepositoryRegistry,
    pub slm_policies: slm::SlmPolicyRegistry,
}

/// Opt-in Elasticsearch product-compat: when `SURCH_ELASTIC_PRODUCT_COMPAT`
/// is set (`1`/`true`), Surch stamps every response with
/// `x-elastic-product: Elasticsearch`. The official `@elastic/elasticsearch`
/// v8 client enforces a product check on that header and rejects every
/// request otherwise — so this flag is required for ES-v8-client apps
/// (e.g. matchID's `deces-backend`) to talk to Surch. Default off keeps the
/// OpenSearch identity untouched. Read once at startup.
fn elastic_product_compat_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("SURCH_ELASTIC_PRODUCT_COMPAT")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

async fn stamp_elastic_product_header(mut response: Response) -> Response {
    if elastic_product_compat_enabled() {
        response.headers_mut().insert(
            "x-elastic-product",
            HeaderValue::from_static("Elasticsearch"),
        );
    }
    response
}

/// Build the P0 OpenSearch-compatible API router.
pub fn app_router() -> Router {
    app_router_with_state(AppRouterState::default())
}

/// Build the router on top of an explicit shared-state bundle. Useful
/// for `main.rs` (which also spawns the SLM scheduler against the same
/// `slm_policies` registry) and for integration tests that drive the
/// scheduler tick loop directly.
pub fn app_router_with_state(shared: AppRouterState) -> Router {
    // Install the global Prometheus recorder before the router starts
    // emitting metrics. Idempotent: safe to call from every test and
    // from `main` at startup.
    let _ = metrics::install_global();

    let app_state = shared.app;
    let snapshot_registry = shared.snapshot_repositories;
    let slm_policies = shared.slm_policies;
    let snapshot_state = snapshot_es::routes::SnapshotAppState {
        app: app_state.clone(),
        registry: snapshot_registry.clone(),
    };
    let slm_state = slm::routes::SlmAppState {
        app: app_state.clone(),
        repositories: snapshot_registry.clone(),
        policies: slm_policies.clone(),
    };

    let slm_router = Router::new()
        .route("/_slm/policy", get(slm::routes::list_policies_handler))
        .route(
            "/_slm/policy/:name",
            get(slm::routes::get_policy_handler)
                .put(slm::routes::put_policy_handler)
                .delete(slm::routes::delete_policy_handler),
        )
        .route(
            "/_slm/policy/:name/_execute",
            post(slm::routes::execute_policy_handler),
        )
        .route(
            "/_slm/policy/:name/_executions",
            get(slm::routes::executions_handler),
        )
        .route("/_slm/status", get(slm::routes::status_handler))
        .with_state(slm_state);

    let snapshot_es_router = Router::new()
        .route(
            "/_snapshot",
            get(snapshot_es::routes::list_repositories_handler),
        )
        .route(
            "/_snapshot/_status",
            get(snapshot_es::routes::snapshot_status_all_handler),
        )
        .route(
            "/_snapshot/:repository",
            get(snapshot_es::routes::get_repository_handler)
                .put(snapshot_es::routes::register_repository_handler)
                .delete(snapshot_es::routes::delete_repository_handler),
        )
        .route(
            "/_snapshot/:repository/_verify",
            post(snapshot_es::routes::verify_repository_handler)
                .get(snapshot_es::routes::verify_repository_handler),
        )
        .route(
            "/_snapshot/:repository/_status",
            get(snapshot_es::routes::snapshot_status_repo_handler),
        )
        .route(
            "/_snapshot/:repository/:snapshot",
            get(snapshot_es::routes::get_snapshot_handler)
                .put(snapshot_es::routes::create_snapshot_handler)
                .delete(snapshot_es::routes::delete_snapshot_handler),
        )
        .route(
            "/_snapshot/:repository/:snapshot/_restore",
            post(snapshot_es::routes::restore_snapshot_handler),
        )
        .route(
            "/_snapshot/:repository/:snapshot/_status",
            get(snapshot_es::routes::snapshot_status_one_handler),
        )
        .with_state(snapshot_state);

    Router::new()
        .route("/", get(root::root_handler))
        .route("/_prometheus_metrics", get(metrics::prometheus_handler))
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
            "/_component_template",
            get(component_template::list_component_templates_handler),
        )
        .route(
            "/_component_template/:name",
            get(component_template::get_component_template_handler)
                .put(component_template::put_component_template_handler)
                .delete(component_template::delete_component_template_handler),
        )
        .route(
            "/:index/_mapping",
            get(index::mapping_handler)
                .put(index::put_mapping_handler)
                .post(index::put_mapping_handler),
        )
        .route(
            "/:index",
            get(index::index_metadata_handler)
                .put(index::create_index_handler)
                .delete(index::delete_index_handler),
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
            "/_search/scroll",
            post(search::scroll_handler).get(search::scroll_handler),
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
        .route("/_surch/stats", get(stats::stats_handler))
        .route(
            "/_surch/snapshot/export",
            post(snapshot::export_handler).get(snapshot::export_handler),
        )
        .route(
            "/_surch/snapshot/import",
            post(snapshot::import_handler)
                .layer(DefaultBodyLimit::max(SNAPSHOT_IMPORT_BODY_LIMIT_BYTES)),
        )
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
        .with_state(app_state)
        .merge(snapshot_es_router)
        .merge(slm_router)
        // HTTP middleware: one span per request with method/route/status
        // attributes. Sits at the bottom of the router so every route
        // inherits it. When the OTLP exporter is wired (see
        // `telemetry::init_telemetry`), these spans are forwarded to
        // the collector; otherwise they only land in the `fmt` log.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        // Opt-in ES product-compat header (see `elastic_product_compat_enabled`).
        .layer(axum::middleware::map_response(stamp_elastic_product_header))
}

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch-compatible API";
