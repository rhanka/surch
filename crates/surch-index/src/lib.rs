#![forbid(unsafe_code)]
//! Lucene-compatible mappings, indexing chain, postings, and term dictionary.

pub mod document_index;
pub mod field_infos;
pub mod field_infos_codec;
pub mod live_docs;
pub mod mapping;
pub mod postings;
pub mod segment_field_infos;
pub mod segment_infos;
pub mod segment_manifest;
pub mod stored_fields;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "Lucene-compatible indexing";
