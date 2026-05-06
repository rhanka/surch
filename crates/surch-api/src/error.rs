use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Reusable OpenSearch-compatible error response envelope.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpenSearchError {
    pub error: OpenSearchErrorBody,
    pub status: u16,
}

impl OpenSearchError {
    /// Build an OpenSearch-compatible error envelope from public type/reason/status fields.
    pub fn new(
        status_code: StatusCode,
        error_type: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            error: OpenSearchErrorBody {
                error_type: error_type.into(),
                reason: reason.into(),
            },
            status: status_code.as_u16(),
        }
    }
}

impl IntoResponse for OpenSearchError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self)).into_response()
    }
}

/// Inner OpenSearch-compatible error object.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpenSearchErrorBody {
    #[serde(rename = "type")]
    pub error_type: String,
    pub reason: String,
}
