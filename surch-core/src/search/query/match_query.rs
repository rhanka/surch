use serde::{Deserialize, Serialize};
use crate::common::Document;
use crate::search::{Query, ScoredDocument};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchQuery {
    pub field: String,
    pub query: String,
    #[serde(default)]
    pub operator: MatchOperator,
    #[serde(default)]
    pub fuzziness: Option<usize>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub enum MatchOperator {
    #[default]
    Or,
    And,
}

impl MatchQuery {
    pub fn new(field: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            query: query.into(),
            operator: MatchOperator::Or,
            fuzziness: None,
        }
    }

    pub fn with_operator(mut self, op: MatchOperator) -> Self {
        self.operator = op;
        self
    }

    pub fn with_fuzziness(mut self, fuzziness: usize) -> Self {
        self.fuzziness = Some(fuzziness);
        self
    }
}

impl Query for MatchQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        use crate::search::fuzzy::FuzzyAlgorithm;
        
        let query_lower = self.query.to_lowercase();
        let query_terms: Vec<String> = query_lower.split_whitespace().map(String::from).collect();
        let mut results = Vec::new();
        
        for doc in docs {
            if let Some(field_value) = doc.get_field(&self.field) {
                let field_text = field_value.as_text().unwrap_or("").to_lowercase();
                let doc_terms: Vec<&str> = field_text.split_whitespace().collect();
                
                let matches = if let Some(fuzziness) = self.fuzziness {
                    query_terms.iter().filter(|qt| {
                        doc_terms.iter().any(|dt| 
                            FuzzyAlgorithm::is_fuzzy_match(qt, dt, fuzziness)
                        )
                    }).count()
                } else {
                    match self.operator {
                        MatchOperator::Or => {
                            query_terms.iter().filter(|qt| {
                                doc_terms.iter().any(|dt| (*dt).contains(qt.as_str()))
                            }).count()
                        }
                        MatchOperator::And => {
                            if query_terms.iter().all(|qt| {
                                doc_terms.iter().any(|dt| (*dt).contains(qt.as_str()))
                            }) {
                                query_terms.len()
                            } else {
                                0
                            }
                        }
                    }
                };
                
                if matches > 0 {
                    let score = matches as f64 / query_terms.len() as f64;
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
pub struct MatchPhraseQuery {
    pub field: String,
    pub query: String,
    #[serde(default = "default_slop")]
    pub slop: usize,
}

fn default_slop() -> usize { 0 }

impl MatchPhraseQuery {
    pub fn new(field: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            query: query.into(),
            slop: 0,
        }
    }
}

impl Query for MatchPhraseQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        let query_terms: Vec<&str> = self.query.split_whitespace().collect();
        let mut results = Vec::new();
        
        for doc in docs {
            if let Some(field_value) = doc.get_field(&self.field) {
                let field_text = field_value.as_text().unwrap_or("");
                let doc_terms: Vec<&str> = field_text.split_whitespace().collect();
                
                if Self::phrase_match(&query_terms, &doc_terms, self.slop) {
                    results.push(ScoredDocument {
                        doc: doc.clone(),
                        score: 1.0,
                    });
                }
            }
        }
        
        results
    }

    fn estimate_cost(&self) -> usize {
        50
    }
}

impl MatchPhraseQuery {
    fn phrase_match(query_terms: &[&str], doc_terms: &[&str], slop: usize) -> bool {
        if query_terms.is_empty() {
            return true;
        }
        
        let mut qi = 0;
        for dt in doc_terms {
            if qi < query_terms.len() && *dt == query_terms[qi] {
                qi += 1;
            }
        }
        
        if qi == query_terms.len() {
            return true;
        }
        
        if slop > 0 {
            qi = 0;
            let mut distance = 0;
            for dt in doc_terms {
                if qi < query_terms.len() && *dt == query_terms[qi] {
                    qi += 1;
                } else if qi < query_terms.len() {
                    distance += 1;
                }
                if distance > slop {
                    return false;
                }
            }
            qi == query_terms.len()
        } else {
            false
        }
    }
}
