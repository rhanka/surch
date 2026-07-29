#![forbid(unsafe_code)]
//! Lucene-compatible query model, scoring, collectors, and fuzzy automata.

pub mod collector;
pub mod execution;
pub mod fuzzy;
pub mod maxscore;
pub mod query;
pub mod scoring;
pub mod small_float;
pub mod topk;

/// Short crate purpose used by workspace smoke tests.
pub const CRATE_PURPOSE: &str = "Lucene-compatible search";
