use serde::{Deserialize, Serialize};
use crate::common::Document;
use crate::search::{Query, ScoredDocument};

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
    fn from(s: String) -> Self { TermValue::Text(s) }
}

impl From<&str> for TermValue {
    fn from(s: &str) -> Self { TermValue::Text(s.to_string()) }
}

impl From<i32> for TermValue {
    fn from(i: i32) -> Self { TermValue::Integer(i) }
}

impl From<i64> for TermValue {
    fn from(i: i64) -> Self { TermValue::Long(i) }
}

impl From<f32> for TermValue {
    fn from(f: f32) -> Self { TermValue::Float(f) }
}

impl From<f64> for TermValue {
    fn from(f: f64) -> Self { TermValue::Double(f) }
}

impl From<bool> for TermValue {
    fn from(b: bool) -> Self { TermValue::Bool(b) }
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
                        (TermValue::Float(f), _) => fv.as_f64().map(|v| (v - *f as f64).abs() < 0.0001),
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
