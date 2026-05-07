#![forbid(unsafe_code)]
//! Offline OpenSearch oracle replay and comparison helpers.

pub mod ban;
pub mod dataset;
pub mod files;
pub mod normalize;
pub mod replay;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch oracle replay helpers";
