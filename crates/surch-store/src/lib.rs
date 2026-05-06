#![forbid(unsafe_code)]
//! Lucene-compatible directory, translog, manifest, and segment storage.

pub mod data_io;
pub mod directory;
pub mod index_io;
pub mod lock;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "Lucene-compatible storage";
