//! Term query execution over an in-memory term dictionary.

use std::collections::BTreeMap;

use surch_index::postings::{PostingsBlockSkipIter, PostingsList, TermDictionary};
use thiserror::Error;

use crate::collector::{CollectorError, TopDocs, TopDocsCollector};
use crate::query::{BooleanQuery, Query, TermQuery};
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

/// Executes boolean must queries by intersecting exact term query
/// results. Lot 2: when every must-clause is a [`TermQuery`] and every
/// clause has a non-empty posting list, the executor uses a leapfrog
/// AND driven by the codec [`PostingsBlockSkipIter`] cursors instead
/// of materialising each clause's full BTreeMap of doc-id → score.
/// The scoring semantics are unchanged (per-clause BM25 contributions
/// are summed in the same order), so the result set is byte-for-byte
/// identical to the pre-Lot-2 path. A perf counter
/// (`last_blocks_skipped`) exposes the number of 128-blocks the
/// cursors skipped over during the last `execute` call so tests can
/// assert that the skip list is actually doing work.
#[derive(Debug, Clone)]
pub struct BooleanQueryExecutor<'a> {
    term_executor: TermQueryExecutor<'a>,
    /// Number of 128-block chunks the AND-leapfrog cursors skipped
    /// over during the last `execute` call (summed across clauses).
    /// Zero when no leapfrog was used (single clause, fallback path,
    /// or every clause's postings fit in one block).
    last_blocks_skipped: std::cell::Cell<usize>,
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
    /// Programmer-bug-level invariant violation in the codec
    /// `BlockSkipList` builder. Cannot fire in practice (postings are
    /// sorted strictly increasing by `PostingsBuilder::build()`), but
    /// surfaced as a typed error so the AND-leapfrog path never panics.
    #[error("block skip list invariant violated: {0}")]
    BlockSkip(String),
    #[error(transparent)]
    Bm25(#[from] Bm25Error),
}

/// Errors returned while executing boolean queries.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum BooleanQueryExecutionError {
    #[error("boolean must clause {clause_index} must be a term query")]
    UnsupportedMustClause { clause_index: usize },
    #[error(transparent)]
    Term(#[from] TermQueryExecutionError),
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
        let mut collector = TopDocsCollector::new(size)?;
        let Some(postings) = self
            .dictionary
            .postings_with_block_metas(&query.field, &query.value)
        else {
            return Ok(collector.finish());
        };
        let doc_freq = postings.doc_freq_from_block_metas() as u64;
        debug_assert_eq!(
            doc_freq,
            postings.doc_ids().len() as u64,
            "FoR block metadata doc_freq must stay aligned with postings"
        );

        if doc_freq == 0 {
            return Ok(collector.finish());
        }

        for (&doc_id, &freq) in postings.doc_ids().iter().zip(postings.freqs()) {
            let doc_len = self
                .stats
                .doc_len_by_doc_id
                .get(&doc_id)
                .copied()
                .ok_or(TermQueryExecutionError::MissingDocLen { doc_id })?;
            let score = bm25_score(
                self.bm25_config,
                self.stats.doc_count,
                doc_freq,
                u64::from(freq),
                doc_len,
                self.stats.avg_doc_len,
            )?;

            collector.collect(doc_id, score)?;
        }

        Ok(collector.finish())
    }
}

impl<'a> BooleanQueryExecutor<'a> {
    /// Creates a boolean query executor using the term query executor for every clause.
    pub fn new(dictionary: &'a TermDictionary, stats: TermQueryStats) -> Self {
        Self {
            term_executor: TermQueryExecutor::new(dictionary, stats),
            last_blocks_skipped: std::cell::Cell::new(0),
        }
    }

    /// Number of 128-block chunks the AND-leapfrog cursors skipped
    /// over during the last `execute` call (summed across clauses).
    /// Returns `0` if `execute` has not yet been called, or if the
    /// last call took the BTreeMap fallback path (single clause, or
    /// any clause without postings).
    pub fn last_blocks_skipped(&self) -> usize {
        self.last_blocks_skipped.get()
    }

    /// Executes required term clauses and returns documents present in every clause.
    pub fn execute(
        &self,
        query: &BooleanQuery,
        size: usize,
    ) -> Result<TopDocs, BooleanQueryExecutionError> {
        self.last_blocks_skipped.set(0);
        let mut collector = TopDocsCollector::new(size).map_err(TermQueryExecutionError::from)?;

        // Validate every clause is a TermQuery first, so the leapfrog
        // path below can stay unconditional.
        let mut term_queries: Vec<&TermQuery> = Vec::with_capacity(query.must.len());
        for (clause_index, clause) in query.must.iter().enumerate() {
            match clause {
                Query::Term(term_query) => term_queries.push(term_query),
                _ => {
                    return Err(BooleanQueryExecutionError::UnsupportedMustClause { clause_index });
                }
            }
        }

        // Pull each clause's PostingsList + per-clause doc_freq once,
        // up front, so the inner leapfrog loop only walks postings.
        let mut clauses: Vec<LeapfrogClause<'a>> = Vec::with_capacity(term_queries.len());
        for term_query in &term_queries {
            let Some(postings) = self
                .term_executor
                .dictionary
                .postings_with_block_metas(&term_query.field, &term_query.value)
            else {
                // Missing term => AND result is empty. Short-circuit.
                return Ok(collector.finish());
            };
            let doc_freq = postings.doc_freq_from_block_metas() as u64;
            if doc_freq == 0 {
                return Ok(collector.finish());
            }
            let skip_iter = match postings.skip_iter() {
                Ok(Some(iter)) => iter,
                // `block_skip_list()` returned None: empty postings
                // (caught above by doc_freq == 0). Defensive
                // short-circuit.
                Ok(None) => return Ok(collector.finish()),
                Err(err) => {
                    return Err(BooleanQueryExecutionError::Term(
                        TermQueryExecutionError::BlockSkip(err.to_string()),
                    ));
                }
            };
            clauses.push(LeapfrogClause {
                postings,
                doc_freq,
                iter: skip_iter,
            });
        }

        // Order clauses by ascending doc_freq so the rarest term
        // drives the leapfrog. This minimises the number of
        // `advance_to` calls on the longer posting lists.
        clauses.sort_by_key(|clause| clause.doc_freq);

        let summed_scores = self
            .run_leapfrog(&mut clauses)
            .map_err(BooleanQueryExecutionError::Term)?;

        // Record blocks_skipped sum after running the leapfrog. Cell
        // makes this observable to tests without taking &mut self.
        let blocks_skipped: usize = clauses.iter().map(|c| c.iter.blocks_skipped()).sum();
        self.last_blocks_skipped.set(blocks_skipped);

        for (doc_id, score) in summed_scores {
            collector
                .collect(doc_id, score)
                .map_err(TermQueryExecutionError::from)?;
        }

        Ok(collector.finish())
    }

    /// Leapfrog AND over `clauses` (sorted by ascending `doc_freq`).
    /// Returns the BTreeMap of (doc_id, sum-of-per-clause-BM25). The
    /// rarest clause drives the candidate stream; the others
    /// `advance_to(candidate)` to confirm presence and contribute
    /// their per-clause BM25 score.
    fn run_leapfrog(
        &self,
        clauses: &mut [LeapfrogClause<'a>],
    ) -> Result<Vec<(u32, f64)>, TermQueryExecutionError> {
        // The driver walks its doc_ids in ascending order and each term has one
        // posting per doc, so the candidates come out strictly ascending with no
        // duplicates — a `Vec` (append O(1)) replaces the `BTreeMap` (red-black
        // insert per hit) with the same ascending iteration order downstream.
        let mut out: Vec<(u32, f64)> = Vec::new();
        if clauses.is_empty() {
            return Ok(out);
        }

        // Walk the rarest posting list one doc at a time; for each
        // candidate, ask every other clause to `advance_to(doc_id)`
        // and check if it landed exactly on `doc_id`.
        // Drive the rarest term's compact doc_id channel (half the bytes of
        // an AoS `Posting` — the leapfrog only needs doc_ids, never freqs).
        let driver_doc_ids = clauses[0].postings.doc_ids().to_vec();
        for (driver_idx, &doc_id) in driver_doc_ids.iter().enumerate() {
            let mut all_match = true;
            // The driver itself trivially contributes its own freq.
            // Iterate the followers and verify presence.
            for follower in &mut clauses[1..] {
                let landed = follower.iter.advance_to(doc_id);
                let Some(found) = landed else {
                    all_match = false;
                    break;
                };
                if found != doc_id {
                    // The leapfrog landed past doc_id (gap in the
                    // follower's posting list). This candidate is
                    // not in the intersection.
                    all_match = false;
                    // Do NOT break: every follower still needs to
                    // observe a non-decreasing advance_to sequence,
                    // but since we don't rewind, the next outer
                    // iteration's larger doc_id will keep things
                    // monotonic. Break here is therefore safe AND
                    // cheaper.
                    break;
                }
            }
            if !all_match {
                continue;
            }

            // Confirmed: every clause has `doc_id`. Score the
            // contribution of every clause and sum.
            let mut summed = 0.0_f64;
            // Driver contribution: the driver's `freqs` channel is index-aligned
            // with its doc_id channel, so freqs()[driver_idx] is the freq we need.
            let driver_freq = clauses[0].postings.freqs()[driver_idx];
            summed += self.score_posting(&clauses[0], doc_id, driver_freq)?;
            // Follower contributions. The matching loop above just landed each
            // follower's cursor exactly on `doc_id`, so the matched posting is at
            // `iter.position() - 1` (the `freqs` channel is index-aligned with the
            // `doc_ids` channel) — read its `freq` in O(1) instead of a
            // `binary_search` per hit (the cache-missing per-candidate cost the
            // deces tail decompose flagged; see campaign #18/#19).
            for follower in &clauses[1..] {
                let idx = follower.iter.position() - 1;
                debug_assert_eq!(
                    follower.postings.doc_ids()[idx],
                    doc_id,
                    "leapfrog cursor position must point at the just-matched doc_id"
                );
                let follower_freq = follower.postings.freqs()[idx];
                summed += self.score_posting(follower, doc_id, follower_freq)?;
            }

            out.push((doc_id, summed));
        }

        Ok(out)
    }

    fn score_posting(
        &self,
        clause: &LeapfrogClause<'a>,
        doc_id: u32,
        freq: u32,
    ) -> Result<f64, TermQueryExecutionError> {
        let stats = &self.term_executor.stats;
        let doc_len = stats
            .doc_len_by_doc_id
            .get(&doc_id)
            .copied()
            .ok_or(TermQueryExecutionError::MissingDocLen { doc_id })?;
        bm25_score(
            self.term_executor.bm25_config,
            stats.doc_count,
            clause.doc_freq,
            u64::from(freq),
            doc_len,
            stats.avg_doc_len,
        )
        .map_err(TermQueryExecutionError::from)
    }
}

/// Internal view of one boolean must-clause used by the leapfrog AND.
struct LeapfrogClause<'a> {
    postings: PostingsList<'a>,
    doc_freq: u64,
    iter: PostingsBlockSkipIter<'a>,
}

impl From<CollectorError> for TermQueryExecutionError {
    fn from(error: CollectorError) -> Self {
        match error {
            CollectorError::SizeMustBePositive => Self::SizeMustBePositive,
            CollectorError::NonFiniteScore { doc_id } => Self::NonFiniteScore { doc_id },
        }
    }
}
