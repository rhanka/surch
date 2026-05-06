use axum::Json;
use serde::Serialize;

/// Deterministic P0 OpenSearch-compatible root response.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RootResponse {
    pub name: &'static str,
    pub cluster_name: &'static str,
    pub cluster_uuid: &'static str,
    pub version: RootVersion,
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
            number: env!("CARGO_PKG_VERSION"),
            distribution: "opensearch",
        },
        tagline: "The OpenSearch Project: https://opensearch.org/",
    }
}
