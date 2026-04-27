use crate::common::Document;
use crate::search::{Query, QueryType, ScoredDocument};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BoolQuery {
    #[serde(default)]
    pub must: Vec<QueryType>,
    #[serde(default)]
    pub filter: Vec<QueryType>,
    #[serde(default)]
    pub should: Vec<QueryType>,
    #[serde(default)]
    pub must_not: Vec<QueryType>,
    #[serde(default = "default_minimum_should_match")]
    pub minimum_should_match: usize,
}

fn default_minimum_should_match() -> usize {
    1
}

impl BoolQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn must(mut self, query: QueryType) -> Self {
        self.must.push(query);
        self
    }

    pub fn filter(mut self, query: QueryType) -> Self {
        self.filter.push(query);
        self
    }

    pub fn should(mut self, query: QueryType) -> Self {
        self.should.push(query);
        self
    }

    pub fn must_not(mut self, query: QueryType) -> Self {
        self.must_not.push(query);
        self
    }
}

impl Query for BoolQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        let mut results = Vec::new();
        let minimum_should_match = if self.minimum_should_match == default_minimum_should_match()
            && (!self.must.is_empty() || !self.filter.is_empty())
        {
            0
        } else {
            self.minimum_should_match
        };

        for doc in docs {
            let mut must_score = 0.0;
            let mut must_match = self.must.is_empty();
            let mut filter_pass = self.filter.is_empty();
            let mut should_score = 0.0;
            let mut should_match_count = 0;
            let mut must_not_match = true;

            for q in &self.must {
                let hits = q.execute(&[doc.clone()]);
                if !hits.is_empty() {
                    must_score += hits[0].score;
                    must_match = true;
                }
            }

            for q in &self.filter {
                let hits = q.execute(&[doc.clone()]);
                if !hits.is_empty() {
                    filter_pass = true;
                } else {
                    filter_pass = false;
                    break;
                }
            }

            for q in &self.should {
                let hits = q.execute(&[doc.clone()]);
                if !hits.is_empty() {
                    should_score += hits[0].score;
                    should_match_count += 1;
                }
            }

            for q in &self.must_not {
                let hits = q.execute(&[doc.clone()]);
                if !hits.is_empty() {
                    must_not_match = false;
                    break;
                }
            }

            if must_match && filter_pass && must_not_match {
                let total_score = must_score + should_score;
                if self.should.is_empty() || should_match_count >= minimum_should_match {
                    results.push(ScoredDocument {
                        doc: doc.clone(),
                        score: total_score,
                    });
                }
            }
        }

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        results
    }

    fn estimate_cost(&self) -> usize {
        self.must.len() * 50
            + self.filter.len() * 50
            + self.should.len() * 50
            + self.must_not.len() * 50
    }
}

#[cfg(test)]
mod tests {
    use super::BoolQuery;
    use crate::common::{Document, FieldValue};
    use crate::search::{MatchQuery, Query, QueryType, TermQuery};

    #[test]
    fn bool_query_with_must_does_not_require_should_by_default() {
        let docs = vec![
            Document::new("1")
                .with_field("status", FieldValue::Keyword("published".to_string()))
                .with_field("title", FieldValue::Text("rust search".to_string())),
            Document::new("2").with_field("status", FieldValue::Keyword("published".to_string())),
        ];

        let query = BoolQuery::new()
            .must(QueryType::Term(TermQuery::new("status", "published")))
            .should(QueryType::Match(MatchQuery::new("title", "rust")));

        let results = query.execute(&docs);
        assert_eq!(results.len(), 2);
    }
}
