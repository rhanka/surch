use crate::search::ScoredDocument;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub hits: Hits,
    pub took: u64,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub shards: Shards,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hits {
    pub total: TotalHits,
    pub hits: Vec<Hit>,
    #[serde(default)]
    pub max_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotalHits {
    pub value: u64,
    pub relation: String,
}

impl TotalHits {
    pub fn new(value: u64) -> Self {
        Self {
            value,
            relation: if value >= 10000 { "gte" } else { "eq" }.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_score")]
    pub score: f64,
    #[serde(default)]
    pub source: Option<serde_json::Value>,
    #[serde(default)]
    pub highlights: Option<std::collections::HashMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Shards {
    pub total: u32,
    pub successful: u32,
    pub failed: u32,
}

pub struct ResultCollector {
    from: usize,
    size: usize,
    sort: Vec<SortField>,
    results: Vec<ScoredDocument>,
}

#[derive(Debug, Clone)]
pub struct SortField {
    pub field: String,
    pub order: SortOrder,
    pub missing: Option<MissingValue>,
}

#[derive(Debug, Clone, Copy)]
pub enum SortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub enum MissingValue {
    First,
    Last,
}

impl ResultCollector {
    pub fn new() -> Self {
        Self {
            from: 0,
            size: 10,
            sort: Vec::new(),
            results: Vec::new(),
        }
    }

    pub fn with_pagination(mut self, from: usize, size: usize) -> Self {
        self.from = from;
        self.size = size;
        self
    }

    pub fn with_sort(mut self, field: &str, order: SortOrder) -> Self {
        self.sort.push(SortField {
            field: field.to_string(),
            order,
            missing: None,
        });
        self
    }

    pub fn collect(&mut self, docs: Vec<ScoredDocument>) {
        self.results = docs;
    }

    pub fn finalize(self, index_name: &str) -> SearchResult {
        let mut hits = self.results;

        if !self.sort.is_empty() {
            for sf in self.sort.iter().rev() {
                match sf.order {
                    SortOrder::Asc => {
                        hits.sort_by(|a, b| {
                            let a_val = a
                                .doc
                                .get_field(&sf.field)
                                .map(|v| v.as_text().unwrap_or(""));
                            let b_val = b
                                .doc
                                .get_field(&sf.field)
                                .map(|v| v.as_text().unwrap_or(""));
                            a_val.cmp(&b_val)
                        });
                    }
                    SortOrder::Desc => {
                        hits.sort_by(|a, b| {
                            let a_val = a
                                .doc
                                .get_field(&sf.field)
                                .map(|v| v.as_text().unwrap_or(""));
                            let b_val = b
                                .doc
                                .get_field(&sf.field)
                                .map(|v| v.as_text().unwrap_or(""));
                            b_val.cmp(&a_val)
                        });
                    }
                }
            }
        }

        let total = hits.len() as u64;
        let max_score = hits.iter().map(|h| h.score).fold(0.0f64, |a, b| a.max(b));

        let paginated: Vec<Hit> = hits
            .into_iter()
            .skip(self.from)
            .take(self.size)
            .map(|sd| Hit {
                index: index_name.to_string(),
                id: sd.doc.id.clone(),
                score: sd.score,
                source: Some(
                    serde_json::to_value(&sd.doc.fields).unwrap_or(serde_json::Value::Null),
                ),
                highlights: None,
            })
            .collect();

        SearchResult {
            took: 0,
            timed_out: false,
            shards: Shards {
                total: 1,
                successful: 1,
                failed: 0,
            },
            hits: Hits {
                total: TotalHits::new(total),
                hits: paginated,
                max_score,
            },
        }
    }
}

impl Default for ResultCollector {
    fn default() -> Self {
        Self::new()
    }
}
