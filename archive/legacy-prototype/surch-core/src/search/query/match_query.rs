use crate::common::Document;
use crate::search::{Query, ScoredDocument};
use serde::{Deserialize, Serialize};

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
                    query_terms
                        .iter()
                        .filter(|qt| {
                            doc_terms
                                .iter()
                                .any(|dt| FuzzyAlgorithm::is_fuzzy_match(qt, dt, fuzziness))
                        })
                        .count()
                } else {
                    match self.operator {
                        MatchOperator::Or => query_terms
                            .iter()
                            .filter(|qt| doc_terms.iter().any(|dt| (*dt).contains(qt.as_str())))
                            .count(),
                        MatchOperator::And => {
                            if query_terms
                                .iter()
                                .all(|qt| doc_terms.iter().any(|dt| (*dt).contains(qt.as_str())))
                            {
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

fn default_slop() -> usize {
    0
}

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

        if query_terms.len() > doc_terms.len() {
            return false;
        }

        for start in 0..doc_terms.len() {
            if doc_terms[start] != query_terms[0] {
                continue;
            }

            let mut qi = 1;
            let mut last_match = start;
            let mut used_slop = 0;

            for (index, term) in doc_terms.iter().enumerate().skip(start + 1) {
                if qi == query_terms.len() {
                    break;
                }

                if *term == query_terms[qi] {
                    used_slop += index.saturating_sub(last_match + 1);
                    if used_slop > slop {
                        break;
                    }
                    last_match = index;
                    qi += 1;
                }
            }

            if qi == query_terms.len() {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::{MatchOperator, MatchPhraseQuery, MatchQuery};
    use crate::common::{Document, FieldValue};
    use crate::search::Query;

    #[test]
    fn match_query_with_and_requires_all_terms() {
        let docs = vec![
            Document::new("1").with_field("title", FieldValue::Text("rust search".to_string())),
            Document::new("2").with_field("title", FieldValue::Text("rust only".to_string())),
        ];

        let results = MatchQuery::new("title", "rust search")
            .with_operator(MatchOperator::And)
            .execute(&docs);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc.id, "1");
    }

    #[test]
    fn match_phrase_query_requires_adjacency_when_slop_is_zero() {
        let docs = vec![
            Document::new("1")
                .with_field("title", FieldValue::Text("search fast engine".to_string())),
            Document::new("2").with_field("title", FieldValue::Text("search engine".to_string())),
        ];

        let results = MatchPhraseQuery::new("title", "search engine").execute(&docs);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc.id, "2");
    }

    #[test]
    fn match_phrase_query_uses_slop_for_single_gap() {
        let docs = vec![Document::new("1")
            .with_field("title", FieldValue::Text("search fast engine".to_string()))];

        let results = MatchPhraseQuery {
            field: "title".to_string(),
            query: "search engine".to_string(),
            slop: 1,
        }
        .execute(&docs);

        assert_eq!(results.len(), 1);
    }
}
