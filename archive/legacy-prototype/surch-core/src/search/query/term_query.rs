use crate::common::Document;
use crate::search::{Query, ScoredDocument};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermQuery {
    pub field: String,
    pub value: TermValue,
    #[serde(default)]
    pub boost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TermValue {
    Text(String),
    Integer(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermsQuery {
    pub field: String,
    pub values: Vec<TermValue>,
    #[serde(default)]
    pub boost: f64,
}

impl TermQuery {
    pub fn new(field: impl Into<String>, value: impl Into<TermValue>) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
            boost: 1.0,
        }
    }

    pub fn with_boost(mut self, boost: f64) -> Self {
        self.boost = boost;
        self
    }
}

impl From<String> for TermValue {
    fn from(s: String) -> Self {
        TermValue::Text(s)
    }
}

impl From<&str> for TermValue {
    fn from(s: &str) -> Self {
        TermValue::Text(s.to_string())
    }
}

impl From<i32> for TermValue {
    fn from(i: i32) -> Self {
        TermValue::Integer(i)
    }
}

impl From<i64> for TermValue {
    fn from(i: i64) -> Self {
        TermValue::Long(i)
    }
}

impl From<f32> for TermValue {
    fn from(f: f32) -> Self {
        TermValue::Float(f)
    }
}

impl From<f64> for TermValue {
    fn from(f: f64) -> Self {
        TermValue::Double(f)
    }
}

impl From<bool> for TermValue {
    fn from(b: bool) -> Self {
        TermValue::Bool(b)
    }
}

impl Query for TermQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        docs.iter()
            .filter_map(|doc| {
                doc.get_field(&self.field).and_then(|fv| {
                    let matches = match (&self.value, fv) {
                        (TermValue::Text(t), _) => fv.as_text().map(|v| v == t),
                        (TermValue::Integer(i), _) => fv.as_i64().map(|v| v == *i as i64),
                        (TermValue::Long(l), _) => fv.as_i64().map(|v| v == *l),
                        (TermValue::Float(f), _) => {
                            fv.as_f64().map(|v| (v - *f as f64).abs() < 0.0001)
                        }
                        (TermValue::Double(d), _) => fv.as_f64().map(|v| (v - d).abs() < 0.0001),
                        (TermValue::Bool(b), _) => fv.as_bool().map(|v| v == *b),
                    };

                    matches.map(|m| {
                        if m {
                            Some(ScoredDocument {
                                doc: doc.clone(),
                                score: self.boost,
                            })
                        } else {
                            None
                        }
                    })
                })
            })
            .flatten()
            .collect()
    }

    fn estimate_cost(&self) -> usize {
        10
    }
}

impl TermsQuery {
    pub fn new(field: impl Into<String>, values: Vec<String>) -> Self {
        Self {
            field: field.into(),
            values: values.into_iter().map(TermValue::from).collect(),
            boost: 1.0,
        }
    }
}

impl Query for TermsQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        docs.iter()
            .filter_map(|doc| {
                let field_value = doc.get_field(&self.field)?;

                let matches = self.values.iter().any(|value| match (value, field_value) {
                    (TermValue::Text(t), _) => {
                        field_value.as_text().map(|v| v == t).unwrap_or(false)
                    }
                    (TermValue::Integer(i), _) => field_value
                        .as_i64()
                        .map(|v| v == *i as i64)
                        .unwrap_or(false),
                    (TermValue::Long(l), _) => {
                        field_value.as_i64().map(|v| v == *l).unwrap_or(false)
                    }
                    (TermValue::Float(f), _) => field_value
                        .as_f64()
                        .map(|v| (v - *f as f64).abs() < 0.0001)
                        .unwrap_or(false),
                    (TermValue::Double(d), _) => field_value
                        .as_f64()
                        .map(|v| (v - d).abs() < 0.0001)
                        .unwrap_or(false),
                    (TermValue::Bool(b), _) => {
                        field_value.as_bool().map(|v| v == *b).unwrap_or(false)
                    }
                });

                if matches {
                    Some(ScoredDocument {
                        doc: doc.clone(),
                        score: self.boost,
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    fn estimate_cost(&self) -> usize {
        10 * self.values.len().max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::{TermQuery, TermsQuery};
    use crate::common::{Document, FieldValue};
    use crate::search::Query;

    #[test]
    fn term_query_matches_exact_value() {
        let docs = vec![
            Document::new("1").with_field("status", FieldValue::Keyword("published".to_string())),
            Document::new("2").with_field("status", FieldValue::Keyword("draft".to_string())),
        ];

        let results = TermQuery::new("status", "published").execute(&docs);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc.id, "1");
    }

    #[test]
    fn terms_query_matches_any_exact_value() {
        let docs = vec![
            Document::new("1").with_field("status", FieldValue::Keyword("published".to_string())),
            Document::new("2").with_field("status", FieldValue::Keyword("draft".to_string())),
            Document::new("3").with_field("status", FieldValue::Keyword("archived".to_string())),
        ];

        let results = TermsQuery::new(
            "status",
            vec!["published".to_string(), "archived".to_string()],
        )
        .execute(&docs);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].doc.id, "1");
        assert_eq!(results[1].doc.id, "3");
    }
}
