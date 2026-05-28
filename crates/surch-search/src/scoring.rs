//! Lucene-compatible scoring primitives.

use thiserror::Error;

/// BM25 configuration compatible with Lucene's BM25Similarity defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bm25Config {
    pub k1: f64,
    pub b: f64,
}

/// Errors returned by BM25 scoring helpers.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum Bm25Error {
    /// The corpus must contain at least one document.
    #[error("doc_count must be greater than 0")]
    DocCountZero,
    /// The term must appear in at least one document.
    #[error("doc_freq must be greater than 0")]
    DocFreqZero,
    /// The term document frequency cannot exceed the corpus document count.
    #[error("doc_freq {doc_freq} exceeds doc_count {doc_count}")]
    DocFreqExceedsDocCount { doc_freq: u64, doc_count: u64 },
    /// The term must occur at least once in the scored document.
    #[error("term_freq must be greater than 0")]
    TermFreqZero,
    /// The scored document length must be positive.
    #[error("doc_len must be greater than 0")]
    DocLenZero,
    /// Average document length must be a positive finite value.
    #[error("avg_doc_len must be greater than 0")]
    AvgDocLenNotPositive,
    /// Lucene BM25 accepts non-negative k1 values.
    #[error("k1 {k1} must be greater than or equal to 0")]
    NegativeK1 { k1: f64 },
    /// k1 must be finite.
    #[error("k1 must be finite")]
    NonFiniteK1,
    /// Lucene BM25 requires b to be in the inclusive range [0, 1].
    #[error("b {b} must be in the inclusive range [0, 1]")]
    BOutOfRange { b: f64 },
    /// b must be finite.
    #[error("b must be finite")]
    NonFiniteB,
}

impl Bm25Config {
    /// Creates a BM25 configuration after validating Lucene parameter bounds.
    pub fn new(k1: f64, b: f64) -> Result<Self, Bm25Error> {
        if !k1.is_finite() {
            return Err(Bm25Error::NonFiniteK1);
        }

        if k1 < 0.0 {
            return Err(Bm25Error::NegativeK1 { k1 });
        }

        if !b.is_finite() {
            return Err(Bm25Error::NonFiniteB);
        }

        if !(0.0..=1.0).contains(&b) {
            return Err(Bm25Error::BOutOfRange { b });
        }

        Ok(Self { k1, b })
    }
}

impl Default for Bm25Config {
    fn default() -> Self {
        Self { k1: 1.2, b: 0.75 }
    }
}

/// Computes Lucene BM25Similarity inverse document frequency.
pub fn bm25_idf(doc_count: u64, doc_freq: u64) -> Result<f64, Bm25Error> {
    validate_corpus_stats(doc_count, doc_freq)?;

    Ok((1.0 + (doc_count as f64 - doc_freq as f64 + 0.5) / (doc_freq as f64 + 0.5)).ln())
}

/// A BM25 scorer with the per-term constants (idf, k1, b, avg_doc_len)
/// precomputed ONCE, so the per-document hot loop is branch-free and `ln()`-free.
///
/// Track A (beat-ES optimisation #6): the previous hot path called
/// [`bm25_score`] per scored doc / per block, which re-validated the config
/// (4 branches), re-validated corpus stats, and recomputed the `ln()` idf —
/// all constant for a given term. Hoisting them out matches Lucene's
/// per-term-cached `BM25Scorer.idf` + length-norm table. The per-doc
/// [`Bm25TermScorer::score`] kernel is the SAME float expression (same
/// left-association) as [`bm25_score`], so scores are **bit-identical**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bm25TermScorer {
    idf: f64,
    k1: f64,
    b: f64,
    avg_doc_len: f64,
}

impl Bm25TermScorer {
    /// Validate the per-term constants once and precompute `idf`. Returns the
    /// same errors [`bm25_score`] would for invalid config / corpus stats /
    /// `avg_doc_len`, so a caller that drops the term on `Err` keeps identical
    /// behaviour to dropping it on a per-call `bm25_score` error.
    pub fn new(
        config: Bm25Config,
        doc_count: u64,
        doc_freq: u64,
        avg_doc_len: f64,
    ) -> Result<Self, Bm25Error> {
        let config = Bm25Config::new(config.k1, config.b)?;
        validate_corpus_stats(doc_count, doc_freq)?;
        if !avg_doc_len.is_finite() || avg_doc_len <= 0.0 {
            return Err(Bm25Error::AvgDocLenNotPositive);
        }
        let idf = bm25_idf(doc_count, doc_freq)?;
        Ok(Self {
            idf,
            k1: config.k1,
            b: config.b,
            avg_doc_len,
        })
    }

    /// Per-document BM25 contribution. Infallible: callers guarantee
    /// `term_freq >= 1` and `doc_len >= 1` at the hot sites (a matched posting
    /// has tf >= 1; doc_len is guarded `> 0`), which is exactly when
    /// [`bm25_score`] would not error. Bit-identical to `bm25_score`'s kernel.
    #[inline]
    pub fn score(&self, term_freq: u64, doc_len: u64) -> f64 {
        let freq = term_freq as f64;
        let length_norm = doc_len as f64 / self.avg_doc_len;
        let denominator = freq + self.k1 * (1.0 - self.b + self.b * length_norm);
        let tf_norm = freq * (self.k1 + 1.0) / denominator;
        self.idf * tf_norm
    }
}

/// Computes a BM25 term score using explicit corpus and document statistics.
///
/// Thin wrapper over [`Bm25TermScorer`] (one shared kernel) plus the
/// `term_freq`/`doc_len` input validation its non-hot-loop callers rely on.
pub fn bm25_score(
    config: Bm25Config,
    doc_count: u64,
    doc_freq: u64,
    term_freq: u64,
    doc_len: u64,
    avg_doc_len: f64,
) -> Result<f64, Bm25Error> {
    validate_score_inputs(doc_count, doc_freq, term_freq, doc_len, avg_doc_len)?;
    let scorer = Bm25TermScorer::new(config, doc_count, doc_freq, avg_doc_len)?;
    Ok(scorer.score(term_freq, doc_len))
}

fn validate_corpus_stats(doc_count: u64, doc_freq: u64) -> Result<(), Bm25Error> {
    if doc_count == 0 {
        return Err(Bm25Error::DocCountZero);
    }

    if doc_freq == 0 {
        return Err(Bm25Error::DocFreqZero);
    }

    if doc_freq > doc_count {
        return Err(Bm25Error::DocFreqExceedsDocCount {
            doc_freq,
            doc_count,
        });
    }

    Ok(())
}

fn validate_score_inputs(
    doc_count: u64,
    doc_freq: u64,
    term_freq: u64,
    doc_len: u64,
    avg_doc_len: f64,
) -> Result<(), Bm25Error> {
    validate_corpus_stats(doc_count, doc_freq)?;

    if term_freq == 0 {
        return Err(Bm25Error::TermFreqZero);
    }

    if doc_len == 0 {
        return Err(Bm25Error::DocLenZero);
    }

    if !avg_doc_len.is_finite() || avg_doc_len <= 0.0 {
        return Err(Bm25Error::AvgDocLenNotPositive);
    }

    Ok(())
}
