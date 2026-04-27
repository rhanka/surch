mod bool_query;
mod match_query;
mod range_query;
mod term_query;

pub use bool_query::*;
pub use match_query::*;
pub use range_query::*;
pub use term_query::*;

use crate::common::{Document, FieldValue};
use crate::search::error::Error;
use crate::search::fuzzy::FuzzyAlgorithm;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub trait Query: Send + Sync {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument>;
    fn estimate_cost(&self) -> usize;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredDocument {
    pub doc: Document,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum QueryType {
    Match(MatchQuery),
    Term(TermQuery),
    Terms(TermsQuery),
    Range(RangeQuery),
    Bool(BoolQuery),
    Fuzzy(FuzzyQuery),
    Exists(ExistsQuery),
    Prefix(PrefixQuery),
    Wildcard(WildcardQuery),
    MultiMatch(MultiMatchQuery),
}

impl Query for QueryType {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        match self {
            QueryType::Match(q) => q.execute(docs),
            QueryType::Term(q) => q.execute(docs),
            QueryType::Terms(q) => q.execute(docs),
            QueryType::Range(q) => q.execute(docs),
            QueryType::Bool(q) => q.execute(docs),
            QueryType::Fuzzy(q) => q.execute(docs),
            QueryType::Exists(q) => q.execute(docs),
            QueryType::Prefix(q) => q.execute(docs),
            QueryType::Wildcard(q) => q.execute(docs),
            QueryType::MultiMatch(q) => q.execute(docs),
        }
    }

    fn estimate_cost(&self) -> usize {
        match self {
            QueryType::Match(q) => q.estimate_cost(),
            QueryType::Term(q) => q.estimate_cost(),
            QueryType::Terms(q) => q.estimate_cost(),
            QueryType::Range(q) => q.estimate_cost(),
            QueryType::Bool(q) => q.estimate_cost(),
            QueryType::Fuzzy(q) => q.estimate_cost(),
            QueryType::Exists(q) => q.estimate_cost(),
            QueryType::Prefix(q) => q.estimate_cost(),
            QueryType::Wildcard(q) => q.estimate_cost(),
            QueryType::MultiMatch(q) => q.estimate_cost(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzyQuery {
    pub field: String,
    pub value: String,
    #[serde(default = "default_fuzziness")]
    pub fuzziness: usize,
    #[serde(default = "default_prefix_length")]
    pub prefix_length: usize,
}

fn default_fuzziness() -> usize {
    2
}
fn default_prefix_length() -> usize {
    0
}

impl FuzzyQuery {
    pub fn new(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
            fuzziness: 2,
            prefix_length: 0,
        }
    }

    pub fn with_fuzziness(mut self, fuzziness: usize) -> Self {
        self.fuzziness = fuzziness;
        self
    }

    pub fn with_prefix_length(mut self, prefix_length: usize) -> Self {
        self.prefix_length = prefix_length;
        self
    }
}

impl Query for FuzzyQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        let mut results = Vec::new();

        for doc in docs {
            if let Some(field_value) = doc.get_field(&self.field) {
                let field_text = field_value.as_text().unwrap_or("");

                if FuzzyAlgorithm::is_fuzzy_match(&self.value, field_text, self.fuzziness) {
                    let score = 1.0
                        / (FuzzyAlgorithm::damerau_levenshtein(
                            &self.value,
                            field_text,
                            self.fuzziness,
                        ) as f64
                            + 1.0);
                    results.push(ScoredDocument {
                        doc: doc.clone(),
                        score,
                    });
                }
            }
        }

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results
    }

    fn estimate_cost(&self) -> usize {
        100
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistsQuery {
    pub field: String,
}

impl ExistsQuery {
    pub fn new(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
        }
    }
}

impl Query for ExistsQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        docs.iter()
            .filter(|doc| doc.get_field(&self.field).is_some())
            .map(|doc| ScoredDocument {
                doc: doc.clone(),
                score: 1.0,
            })
            .collect()
    }

    fn estimate_cost(&self) -> usize {
        10
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefixQuery {
    pub field: String,
    pub value: String,
}

impl PrefixQuery {
    pub fn new(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            value: value.into().to_lowercase(),
        }
    }
}

impl Query for PrefixQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        let mut results = Vec::new();

        for doc in docs {
            if let Some(field_value) = doc.get_field(&self.field) {
                if let Some(text) = field_value.as_text() {
                    if text.to_lowercase().starts_with(&self.value) {
                        results.push(ScoredDocument {
                            doc: doc.clone(),
                            score: 1.0,
                        });
                    }
                }
            }
        }

        results
    }

    fn estimate_cost(&self) -> usize {
        50
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WildcardQuery {
    pub field: String,
    pub value: String,
}

impl WildcardQuery {
    pub fn new(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            value: value.into().to_lowercase(),
        }
    }

    fn matches_pattern(&self, text: &str) -> bool {
        let pattern = self.value.replace('*', ".*").replace('?', ".");
        regex::Regex::new(&format!("^{}$", pattern))
            .map(|r| r.is_match(&text.to_lowercase()))
            .unwrap_or(false)
    }
}

impl Query for WildcardQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        let mut results = Vec::new();

        for doc in docs {
            if let Some(field_value) = doc.get_field(&self.field) {
                if let Some(text) = field_value.as_text() {
                    if self.matches_pattern(text) {
                        results.push(ScoredDocument {
                            doc: doc.clone(),
                            score: 1.0,
                        });
                    }
                }
            }
        }

        results
    }

    fn estimate_cost(&self) -> usize {
        100
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiMatchQuery {
    pub query: String,
    pub fields: Vec<String>,
    #[serde(default)]
    pub fuzziness: Option<usize>,
}

impl MultiMatchQuery {
    pub fn new(query: impl Into<String>, fields: Vec<String>) -> Self {
        Self {
            query: query.into(),
            fields,
            fuzziness: None,
        }
    }
}

impl Query for MultiMatchQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        let mut results = Vec::new();

        for doc in docs {
            let mut match_count = 0;

            for field in &self.fields {
                if let Some(field_value) = doc.get_field(field) {
                    if let Some(text) = field_value.as_text() {
                        if let Some(fuzziness) = self.fuzziness {
                            if FuzzyAlgorithm::is_fuzzy_match(&self.query, text, fuzziness) {
                                match_count += 1;
                            }
                        } else if text.to_lowercase().contains(&self.query.to_lowercase()) {
                            match_count += 1;
                        }
                    }
                }
            }

            if match_count > 0 {
                results.push(ScoredDocument {
                    doc: doc.clone(),
                    score: match_count as f64,
                });
            }
        }

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results
    }

    fn estimate_cost(&self) -> usize {
        self.fields.len() * 50
    }
}
