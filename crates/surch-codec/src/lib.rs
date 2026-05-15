#![forbid(unsafe_code)]
//! Lucene-compatible codec utilities and binary format foundations.

pub mod codec_util;
pub mod postings_block;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "Lucene-compatible codecs";
