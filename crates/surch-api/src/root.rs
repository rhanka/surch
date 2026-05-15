use axum::Json;
use serde::Serialize;

/// OpenSearch wire-compatibility version Surch emulates. Bump when the
/// emulated wire contract changes; never derived from `CARGO_PKG_VERSION`.
pub const OPENSEARCH_COMPAT_VERSION: &str = "2.17.1";

/// Deterministic P0 OpenSearch-compatible root response.
///
/// Convention (mirrors Quickwit's `quickwit_version` + `elasticsearch_version`
/// split): `version.number` carries the OpenSearch wire-compat version that
/// OpenSearch clients check against, while `surch_version` exposes the
/// Surch binary version (`CARGO_PKG_VERSION`) for operators.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RootResponse {
    pub name: &'static str,
    pub cluster_name: &'static str,
    pub cluster_uuid: &'static str,
    pub version: RootVersion,
    pub surch_version: &'static str,
    pub opensearch_compat_version: &'static str,
    pub tagline: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RootVersion {
    pub number: &'static str,
    pub distribution: &'static str,
}

/// Axum handler for `GET /`.
pub async fn root_handler() -> Json<RootResponse> {
    Json(root_response())
}

/// Build a deterministic bootstrap response compatible with OpenSearch clients.
pub fn root_response() -> RootResponse {
    RootResponse {
        name: "surch-node-0",
        cluster_name: "surch-cluster",
        cluster_uuid: "00000000-0000-0000-0000-000000000000",
        version: RootVersion {
            number: OPENSEARCH_COMPAT_VERSION,
            distribution: "opensearch",
        },
        surch_version: env!("CARGO_PKG_VERSION"),
        opensearch_compat_version: OPENSEARCH_COMPAT_VERSION,
        tagline: "The OpenSearch Project: https://opensearch.org/",
    }
}
