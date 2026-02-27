use std::path::PathBuf;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::common::FieldValue;
use super::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentMeta {
    pub id: String,
    pub num_docs: u64,
    pub deleted_docs: u64,
    pub size_bytes: u64,
    pub created_at: i64,
    pub version: u64,
}

impl SegmentMeta {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            num_docs: 0,
            deleted_docs: 0,
            size_bytes: 0,
            created_at: chrono::Utc::now().timestamp_millis(),
            version: 1,
        }
    }
}

#[derive(Clone)]
pub struct Segment {
    pub meta: SegmentMeta,
    pub path: PathBuf,
    pub terms: HashMap<String, Vec<TermPosting>>,
    pub store: HashMap<String, HashMap<String, FieldValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermPosting {
    pub doc_id: u64,
    pub term_freq: u32,
    pub positions: Vec<u32>,
}

impl Segment {
    pub fn new(id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            meta: SegmentMeta::new(id),
            path: path.into(),
            terms: HashMap::new(),
            store: HashMap::new(),
        }
    }

    pub fn add_document(&mut self, doc_id: u64, fields: &HashMap<String, FieldValue>) -> Result<(), Error> {
        for (field, value) in fields {
            if let Some(text) = value.as_text() {
                let tokens = tokenize(text);
                for (pos, token) in tokens.iter().enumerate() {
                    self.terms
                        .entry(format!("{}_{}", field, token))
                        .or_insert_with(Vec::new)
                        .push(TermPosting {
                            doc_id,
                            term_freq: 1,
                            positions: vec![pos as u32],
                        });
                }
            }
            
            if let Some(keyword) = value.as_keyword() {
                self.terms
                    .entry(format!("{}_kw", field))
                    .or_insert_with(Vec::new)
                    .push(TermPosting {
                        doc_id,
                        term_freq: 1,
                        positions: vec![],
                    });
                
                self.store
                    .entry(format!("{}_kw", field))
                    .or_insert_with(HashMap::new)
                    .insert(doc_id.to_string(), FieldValue::Keyword(keyword.to_string()));
            }
            
            self.store
                .entry(field.clone())
                .or_insert_with(HashMap::new)
                .insert(doc_id.to_string(), value.clone());
        }
        
        self.meta.num_docs += 1;
        Ok(())
    }

    pub fn get_document(&self, doc_id: &str) -> Option<HashMap<String, FieldValue>> {
        let mut doc = HashMap::new();
        for (_, fields) in &self.store {
            if let Some(value) = fields.get(doc_id) {
                for (k, v) in fields.iter() {
                    if !doc.contains_key(k) {
                        doc.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        if doc.is_empty() { None } else { Some(doc) }
    }

    pub fn search_term(&self, field: &str, term: &str) -> Vec<u64> {
        let key = format!("{}_{}", field, term);
        self.terms.get(&key).map(|postings| {
            postings.iter().map(|p| p.doc_id).collect()
        }).unwrap_or_default()
    }

    pub fn num_docs(&self) -> u64 {
        self.meta.num_docs - self.meta.deleted_docs
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub struct SegmentManager {
    segments: Vec<Segment>,
}

impl SegmentManager {
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    pub fn add_segment(&mut self, segment: Segment) {
        self.segments.push(segment);
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn num_segments(&self) -> usize {
        self.segments.len()
    }

    pub fn total_docs(&self) -> u64 {
        self.segments.iter().map(|s| s.num_docs()).sum()
    }

    pub fn merge(&mut self) -> Result<Segment, Error> {
        if self.segments.len() < 2 {
            return Err(Error::Segment("Not enough segments to merge".to_string()));
        }

        let merged_id = uuid::Uuid::new_v4().to_string();
        let mut merged = Segment::new(merged_id.clone(), PathBuf::new());

        for segment in self.segments.drain(..) {
            for (key, postings) in segment.terms {
                merged.terms.entry(key).or_insert_with(Vec::new).extend(postings);
            }
            for (key, docs) in segment.store {
                merged.store.entry(key).or_insert_with(HashMap::new).extend(docs);
            }
            merged.meta.num_docs += segment.meta.num_docs;
        }

        self.segments.push(merged.clone());
        Ok(merged)
    }
}

impl Default for SegmentManager {
    fn default() -> Self {
        Self::new()
    }
}
