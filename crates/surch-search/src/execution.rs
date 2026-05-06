//! Term query execution over an in-memory term dictionary.

use std::collections::BTreeMap;

use surch_index::postings::TermDictionary;
use thiserror::Error;

use crate::collector::{CollectorError, TopDocs, TopDocsCollector};
use crate::query::TermQuery;
use crate::scoring::{bm25_score, Bm25Config, Bm25Error};

/// Document length statistics required to score term matches with BM25.
#[derive(Debug, Clone, PartialEq)]
pub struct TermQueryStats {
    pub doc_count: u64,
    pub avg_doc_len: f64,
    pub doc_len_by_doc_id: BTreeMap<u32, u64>,
}

/// Executes exact term queries against a term dictionary.
#[derive(Debug, Clone)]
pub struct TermQueryExecutor<'a> {
    dictionary: &'a TermDictionary,
    stats: TermQueryStats,
    bm25_config: Bm25Config,
}

/// Errors returned while validating stats or executing term queries.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum TermQueryExecutionError {
    #[error("top docs size must be greater than zero")]
    SizeMustBePositive,
    #[error("doc_count must be greater than 0")]
    DocCountZero,
    #[error("avg_doc_len must be greater than 0")]
    AvgDocLenNotPositive,
    #[error("doc length for doc {doc_id} must be greater than 0")]
    DocLenZero { doc_id: u32 },
    #[error("missing doc length for doc {doc_id}")]
    MissingDocLen { doc_id: u32 },
    #[error("score for doc {doc_id} must be finite")]
    NonFiniteScore { doc_id: u32 },
    #[error(transparent)]
    Bm25(#[from] Bm25Error),
}

impl TermQueryStats {
    /// Creates BM25 stats after validating corpus and document length inputs.
    pub fn new(
        doc_count: u64,
        avg_doc_len: f64,
        doc_len_by_doc_id: BTreeMap<u32, u64>,
    ) -> Result<Self, TermQueryExecutionError> {
        if doc_count == 0 {
            return Err(TermQueryExecutionError::DocCountZero);
        }

        if !avg_doc_len.is_finite() || avg_doc_len <= 0.0 {
            return Err(TermQueryExecutionError::AvgDocLenNotPositive);
        }

        for (doc_id, doc_len) in &doc_len_by_doc_id {
            if *doc_len == 0 {
                return Err(TermQueryExecutionError::DocLenZero { doc_id: *doc_id });
            }
        }

        Ok(Self {
            doc_count,
            avg_doc_len,
            doc_len_by_doc_id,
        })
    }
}

impl<'a> TermQueryExecutor<'a> {
    /// Creates a term query executor using Lucene-compatible BM25 defaults.
    pub fn new(dictionary: &'a TermDictionary, stats: TermQueryStats) -> Self {
        Self {
            dictionary,
            stats,
            bm25_config: Bm25Config::default(),
        }
    }

    /// Executes an exact term query and returns top documents sorted by score desc, doc id asc.
    pub fn execute(
        &self,
        query: &TermQuery,
        size: usize,
    ) -> Result<TopDocs, TermQueryExecutionError> {
        let postings = self
            .dictionary
            .postings(&query.field, &query.value)
            .map(|postings| postings.collect::<Vec<_>>())
            .unwrap_or_default();
        let doc_freq = postings.len() as u64;
        let mut collector = TopDocsCollector::new(size)?;

        if doc_freq == 0 {
            return Ok(collector.finish());
        }

        for posting in postings {
            let doc_len = self
                .stats
                .doc_len_by_doc_id
                .get(&posting.doc_id)
                .copied()
                .ok_or(TermQueryExecutionError::MissingDocLen {
                    doc_id: posting.doc_id,
                })?;
            let score = bm25_score(
                self.bm25_config,
                self.stats.doc_count,
                doc_freq,
                u64::from(posting.freq),
                doc_len,
                self.stats.avg_doc_len,
            )?;

            collector.collect(posting.doc_id, score)?;
        }

        Ok(collector.finish())
    }
}

impl From<CollectorError> for TermQueryExecutionError {
    fn from(error: CollectorError) -> Self {
        match error {
            CollectorError::SizeMustBePositive => Self::SizeMustBePositive,
            CollectorError::NonFiniteScore { doc_id } => Self::NonFiniteScore { doc_id },
        }
    }
}
