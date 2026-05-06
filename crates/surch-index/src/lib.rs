#![forbid(unsafe_code)]
//! Lucene-compatible mappings, indexing chain, postings, and term dictionary.

pub mod field_infos;
pub mod field_infos_codec;
pub mod segment_infos;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "Lucene-compatible indexing";
