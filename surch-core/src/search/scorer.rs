use serde::{Deserialize, Serialize};
use crate::common::Document;
use crate::search::ScoredDocument;

pub trait Scorer: Send + Sync {
    fn score(&self, doc: &Document, term_freq: u32, doc_len: usize, avg_doc_len: f64, num_docs: u64) -> f64;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Similarity {
    Tf,
    BM25,
}

impl Default for Similarity {
    fn default() -> Self {
        Similarity::BM25
    }
}

pub struct BM25Scorer {
    pub k1: f64,
    pub b: f64,
}

impl BM25Scorer {
    pub fn new() -> Self {
        Self {
            k1: 1.5,
            b: 0.75,
        }
    }
}

impl Default for BM25Scorer {
    fn default() -> Self {
        Self::new()
    }
}

impl Scorer for BM25Scorer {
    fn score(&self, _doc: &Document, term_freq: u32, doc_len: usize, avg_doc_len: f64, _num_docs: u64) -> f64 {
        let tf = term_freq as f64;
        let doc_len = doc_len as f64;
        
        let numerator = tf * (self.k1 + 1.0);
        let denominator = tf + self.k1 * (1.0 - self.b + self.b * (doc_len / avg_doc_len));
        
        numerator / denominator
    }
}

pub struct TfIdfScorer;

impl TfIdfScorer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TfIdfScorer {
    fn default() -> Self {
        Self::new()
    }
}

impl Scorer for TfIdfScorer {
    fn score(&self, _doc: &Document, term_freq: u32, _doc_len: usize, _avg_doc_len: f64, _num_docs: u64) -> f64 {
        (1.0 + (term_freq as f64).ln())
    }
}

pub struct SearchScorer {
    similarity: Box<dyn Scorer>,
}

impl SearchScorer {
    pub fn new(similarity: impl Scorer + 'static) -> Self {
        Self {
            similarity: Box::new(similarity),
        }
    }

    pub fn score(&self, doc: &Document, term_freq: u32, doc_len: usize, avg_doc_len: f64, num_docs: u64) -> f64 {
        self.similarity.score(doc, term_freq, doc_len, avg_doc_len, num_docs)
    }
}

impl Default for SearchScorer {
    fn default() -> Self {
        Self::new(BM25Scorer::new())
    }
}
