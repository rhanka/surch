#![forbid(unsafe_code)]
//! Lucene-compatible directory, translog, manifest, and segment storage.

pub mod data_io;
pub mod directory;
pub mod index_io;
pub mod index_store;
pub mod lock;
pub mod segment_store;
pub mod wal;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "Lucene-compatible storage";
