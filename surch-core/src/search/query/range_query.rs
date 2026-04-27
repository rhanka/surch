use crate::common::Document;
use crate::search::{Query, ScoredDocument};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeQuery {
    pub field: String,
    pub gte: Option<Bound>,
    pub gt: Option<Bound>,
    pub lte: Option<Bound>,
    pub lt: Option<Bound>,
    #[serde(default)]
    pub boost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Bound {
    Integer(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
}

impl RangeQuery {
    pub fn new(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            gte: None,
            gt: None,
            lte: None,
            lt: None,
            boost: 1.0,
        }
    }

    pub fn gte(mut self, value: Bound) -> Self {
        self.gte = Some(value);
        self
    }

    pub fn gt(mut self, value: Bound) -> Self {
        self.gt = Some(value);
        self
    }

    pub fn lte(mut self, value: Bound) -> Self {
        self.lte = Some(value);
        self
    }

    pub fn lt(mut self, value: Bound) -> Self {
        self.lt = Some(value);
        self
    }
}

fn compare_lower_bound(bound: &Bound, value: &crate::common::FieldValue) -> Option<bool> {
    match (bound, value) {
        (Bound::Integer(i), _) => value.as_i64().map(|v| v >= *i as i64),
        (Bound::Long(l), _) => value.as_i64().map(|v| v >= *l),
        (Bound::Float(f), _) => value.as_f64().map(|v| v >= *f as f64),
        (Bound::Double(d), _) => value.as_f64().map(|v| v >= *d),
        (Bound::String(s), _) => value.as_text().map(|v| v >= s.as_str()),
    }
}

fn compare_lower_bound_strict(bound: &Bound, value: &crate::common::FieldValue) -> Option<bool> {
    match (bound, value) {
        (Bound::Integer(i), _) => value.as_i64().map(|v| v > *i as i64),
        (Bound::Long(l), _) => value.as_i64().map(|v| v > *l),
        (Bound::Float(f), _) => value.as_f64().map(|v| v > *f as f64),
        (Bound::Double(d), _) => value.as_f64().map(|v| v > *d),
        (Bound::String(s), _) => value.as_text().map(|v| v > s.as_str()),
    }
}

fn compare_upper_bound(bound: &Bound, value: &crate::common::FieldValue) -> Option<bool> {
    match (bound, value) {
        (Bound::Integer(i), _) => value.as_i64().map(|v| v <= *i as i64),
        (Bound::Long(l), _) => value.as_i64().map(|v| v <= *l),
        (Bound::Float(f), _) => value.as_f64().map(|v| v <= *f as f64),
        (Bound::Double(d), _) => value.as_f64().map(|v| v <= *d),
        (Bound::String(s), _) => value.as_text().map(|v| v <= s.as_str()),
    }
}

fn compare_upper_bound_strict(bound: &Bound, value: &crate::common::FieldValue) -> Option<bool> {
    match (bound, value) {
        (Bound::Integer(i), _) => value.as_i64().map(|v| v < *i as i64),
        (Bound::Long(l), _) => value.as_i64().map(|v| v < *l),
        (Bound::Float(f), _) => value.as_f64().map(|v| v < *f as f64),
        (Bound::Double(d), _) => value.as_f64().map(|v| v < *d),
        (Bound::String(s), _) => value.as_text().map(|v| v < s.as_str()),
    }
}

impl Query for RangeQuery {
    fn execute(&self, docs: &[Document]) -> Vec<ScoredDocument> {
        docs.iter()
            .filter(|doc| {
                if let Some(field_value) = doc.get_field(&self.field) {
                    let mut matches = true;

                    if let Some(gte) = &self.gte {
                        matches = matches && compare_lower_bound(gte, field_value).unwrap_or(false);
                    }
                    if let Some(gt) = &self.gt {
                        matches =
                            matches && compare_lower_bound_strict(gt, field_value).unwrap_or(false);
                    }
                    if let Some(lte) = &self.lte {
                        matches = matches && compare_upper_bound(lte, field_value).unwrap_or(false);
                    }
                    if let Some(lt) = &self.lt {
                        matches =
                            matches && compare_upper_bound_strict(lt, field_value).unwrap_or(false);
                    }

                    matches
                } else {
                    false
                }
            })
            .map(|doc| ScoredDocument {
                doc: doc.clone(),
                score: self.boost,
            })
            .collect()
    }

    fn estimate_cost(&self) -> usize {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::{Bound, RangeQuery};
    use crate::common::{Document, FieldValue};
    use crate::search::Query;

    #[test]
    fn range_query_honors_upper_bounds() {
        let docs = vec![
            Document::new("1").with_field("year", FieldValue::Integer(2023)),
            Document::new("2").with_field("year", FieldValue::Integer(2024)),
            Document::new("3").with_field("year", FieldValue::Integer(2025)),
        ];

        let results = RangeQuery::new("year")
            .gte(Bound::Integer(2023))
            .lte(Bound::Integer(2024))
            .execute(&docs);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].doc.id, "1");
        assert_eq!(results[1].doc.id, "2");
    }
}
