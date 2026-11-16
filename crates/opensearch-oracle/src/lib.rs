#![forbid(unsafe_code)]
// MVP L2b: the workspace enables serde_json `preserve_order` (ES key order
// in responses), which makes `serde_json::Value` 72 bytes instead of 32.
// The comparison errors carry two `Value`s; they are built once per
// mismatch in this offline oracle, so the larger `Err` is harmless.
#![allow(clippy::result_large_err)]
//! Offline OpenSearch oracle replay and comparison helpers.

pub mod ban;
pub mod dataset;
pub mod files;
pub mod normalize;
pub mod replay;
pub mod runner;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "OpenSearch oracle replay helpers";
