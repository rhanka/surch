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

/// Computes a BM25 term score using explicit corpus and document statistics.
pub fn bm25_score(
    config: Bm25Config,
    doc_count: u64,
    doc_freq: u64,
    term_freq: u64,
    doc_len: u64,
    avg_doc_len: f64,
) -> Result<f64, Bm25Error> {
    let config = Bm25Config::new(config.k1, config.b)?;
    validate_score_inputs(doc_count, doc_freq, term_freq, doc_len, avg_doc_len)?;

    let idf = bm25_idf(doc_count, doc_freq)?;
    let freq = term_freq as f64;
    let length_norm = doc_len as f64 / avg_doc_len;
    let denominator = freq + config.k1 * (1.0 - config.b + config.b * length_norm);
    let tf_norm = freq * (config.k1 + 1.0) / denominator;

    Ok(idf * tf_norm)
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
