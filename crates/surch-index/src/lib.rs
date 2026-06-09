#![forbid(unsafe_code)]
//! Lucene-compatible mappings, indexing chain, postings, and term dictionary.

pub mod document_index;
pub mod field_infos;

// Convenience re-exports for the BM25 hot paths in `surch-api` /
// `surch-search` that consume the quantized doc-length slice.
pub use document_index::decode_doc_len_byte;
pub mod field_infos_codec;
pub mod live_docs;
pub mod mapping;
pub mod memory;
pub mod postings;
pub mod roaring;
pub mod segment_field_infos;
pub mod segment_infos;
pub mod segment_manifest;
pub mod stored_fields;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "Lucene-compatible indexing";
