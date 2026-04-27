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
    #[serde(default = "default_transpositions")]
    pub transpositions: bool,
}

fn default_fuzziness() -> usize {
    2
}
fn default_prefix_length() -> usize {
    0
}
fn default_transpositions() -> bool {
    true
}

impl FuzzyQuery {
    pub fn new(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
            fuzziness: 2,
            prefix_length: 0,
            transpositions: true,
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
        let query_value = self.value.to_lowercase();

        for doc in docs {
            if let Some(field_value) = doc.get_field(&self.field) {
                let field_text = field_value.as_text().unwrap_or("").to_lowercase();

                if let Some(distance) = self.match_distance(&query_value, &field_text) {
                    let score = 1.0 / (distance as f64 + 1.0);
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

impl FuzzyQuery {
    fn match_distance(&self, query: &str, candidate: &str) -> Option<usize> {
        let prefix_len = self.prefix_length.min(query.len()).min(candidate.len());
        if query[..prefix_len] != candidate[..prefix_len] {
            return None;
        }

        let query_suffix = &query[prefix_len..];
        let candidate_suffix = &candidate[prefix_len..];
        let distance = if self.transpositions {
            FuzzyAlgorithm::damerau_levenshtein(query_suffix, candidate_suffix, self.fuzziness)
        } else {
            FuzzyAlgorithm::levenshtein(query_suffix, candidate_suffix)
        };

        (distance <= self.fuzziness).then_some(distance)
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

#[cfg(test)]
mod tests {
    use super::{FuzzyQuery, MultiMatchQuery, PrefixQuery, WildcardQuery};
    use crate::common::{Document, FieldValue};
    use crate::search::Query;

    #[test]
    fn prefix_query_matches_case_insensitive_prefix() {
        let docs =
            vec![Document::new("1")
                .with_field("title", FieldValue::Text("Surch Engine".to_string()))];

        let results = PrefixQuery::new("title", "sur").execute(&docs);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn wildcard_query_matches_star_pattern() {
        let docs =
            vec![Document::new("1")
                .with_field("title", FieldValue::Text("search-engine".to_string()))];

        let results = WildcardQuery::new("title", "search*").execute(&docs);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn multi_match_query_matches_any_listed_field() {
        let docs = vec![
            Document::new("1").with_field("body", FieldValue::Text("hello rust world".to_string())),
            Document::new("2").with_field("title", FieldValue::Text("plain text".to_string())),
        ];

        let results = MultiMatchQuery::new("rust", vec!["title".to_string(), "body".to_string()])
            .execute(&docs);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc.id, "1");
    }

    #[test]
    fn fuzzy_query_honors_prefix_length() {
        let docs =
            vec![Document::new("1").with_field("title", FieldValue::Text("jello".to_string()))];

        let results = FuzzyQuery::new("title", "hello")
            .with_fuzziness(1)
            .with_prefix_length(1)
            .execute(&docs);

        assert!(results.is_empty());
    }

    #[test]
    fn fuzzy_query_matches_transposition_with_distance_one() {
        let docs = vec![Document::new("1").with_field("title", FieldValue::Text("ba".to_string()))];

        let results = FuzzyQuery::new("title", "ab")
            .with_fuzziness(1)
            .execute(&docs);

        assert_eq!(results.len(), 1);
    }
}
