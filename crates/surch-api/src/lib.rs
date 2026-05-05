#![forbid(unsafe_code)]
//! OpenSearch-compatible REST API boundary.

pub mod bulk;

pub use bulk::{parse_bulk_ndjson, BulkOperation, BulkParseError};

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch-compatible API";
