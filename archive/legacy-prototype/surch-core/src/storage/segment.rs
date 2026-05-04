use super::error::Error;
use crate::common::FieldValue;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const SEGMENT_FILE_SUFFIX: &str = ".segment.json";

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

#[derive(Serialize, Deserialize)]
struct PersistedSegment {
    meta: SegmentMeta,
    terms: HashMap<String, Vec<TermPosting>>,
    store: HashMap<String, HashMap<String, FieldValue>>,
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

    pub fn add_document(
        &mut self,
        internal_doc_id: u64,
        external_doc_id: &str,
        fields: &HashMap<String, FieldValue>,
    ) -> Result<(), Error> {
        for (field, value) in fields {
            if let Some(text) = value.as_text() {
                let tokens = tokenize(text);
                for (pos, token) in tokens.iter().enumerate() {
                    self.terms
                        .entry(format!("{}_{}", field, token))
                        .or_default()
                        .push(TermPosting {
                            doc_id: internal_doc_id,
                            term_freq: 1,
                            positions: vec![pos as u32],
                        });
                }
            }

            if let Some(_keyword) = value.as_keyword() {
                self.terms
                    .entry(format!("{}_kw", field))
                    .or_default()
                    .push(TermPosting {
                        doc_id: internal_doc_id,
                        term_freq: 1,
                        positions: vec![],
                    });
            }

            self.store
                .entry(field.clone())
                .or_default()
                .insert(external_doc_id.to_string(), value.clone());
        }

        self.meta.num_docs += 1;
        Ok(())
    }

    pub fn persist(&mut self) -> Result<(), Error> {
        std::fs::create_dir_all(&self.path)?;

        let persisted = PersistedSegment {
            meta: self.meta.clone(),
            terms: self.terms.clone(),
            store: self.store.clone(),
        };

        let bytes =
            serde_json::to_vec(&persisted).map_err(|err| Error::Serialization(err.to_string()))?;
        let mut file = File::create(self.file_path())?;
        file.write_all(&bytes)?;
        file.flush()?;

        self.meta.size_bytes = bytes.len() as u64;
        Ok(())
    }

    pub fn load(file_path: impl AsRef<Path>) -> Result<Self, Error> {
        let file_path = file_path.as_ref();
        let mut file = File::open(file_path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        let persisted: PersistedSegment =
            serde_json::from_slice(&bytes).map_err(|err| Error::Corruption(err.to_string()))?;

        Ok(Self {
            meta: persisted.meta,
            path: file_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            terms: persisted.terms,
            store: persisted.store,
        })
    }

    pub fn file_path(&self) -> PathBuf {
        self.path
            .join(format!("{}{}", self.meta.id, SEGMENT_FILE_SUFFIX))
    }

    pub fn get_document(&self, doc_id: &str) -> Option<HashMap<String, FieldValue>> {
        let mut doc = HashMap::new();

        for (field, fields) in &self.store {
            if let Some(value) = fields.get(doc_id) {
                doc.insert(field.clone(), value.clone());
            }
        }

        if doc.is_empty() {
            None
        } else {
            Some(doc)
        }
    }

    pub fn all_documents(&self) -> Vec<(String, HashMap<String, FieldValue>)> {
        let doc_ids: BTreeSet<String> = self
            .store
            .values()
            .flat_map(|fields| fields.keys().cloned())
            .collect();

        doc_ids
            .into_iter()
            .filter_map(|doc_id| self.get_document(&doc_id).map(|fields| (doc_id, fields)))
            .collect()
    }

    pub fn remove_document(&mut self, doc_id: &str) -> bool {
        let mut removed = false;

        for fields in self.store.values_mut() {
            if fields.remove(doc_id).is_some() {
                removed = true;
            }
        }

        if removed {
            self.meta.deleted_docs += 1;
        }

        removed
    }

    pub fn search_term(&self, field: &str, term: &str) -> Vec<u64> {
        let key = format!("{}_{}", field, term);
        self.terms
            .get(&key)
            .map(|postings| postings.iter().map(|p| p.doc_id).collect())
            .unwrap_or_default()
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
        let mut merged = Segment::new(merged_id, PathBuf::new());

        for segment in self.segments.drain(..) {
            for (key, postings) in segment.terms {
                merged.terms.entry(key).or_default().extend(postings);
            }
            for (key, docs) in segment.store {
                merged.store.entry(key).or_default().extend(docs);
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

#[cfg(test)]
mod tests {
    use super::Segment;
    use crate::common::FieldValue;
    use tempfile::tempdir;

    #[test]
    fn persists_and_loads_segment_documents() {
        let temp_dir = tempdir().expect("temp dir");
        let mut segment = Segment::new("seg-test", temp_dir.path());

        segment
            .add_document(
                1,
                "doc-1",
                &std::collections::HashMap::from([
                    (
                        "title".to_string(),
                        FieldValue::Text("Hello World".to_string()),
                    ),
                    ("year".to_string(), FieldValue::Integer(2024)),
                ]),
            )
            .expect("add document");

        segment.persist().expect("persist segment");

        let loaded = Segment::load(temp_dir.path().join("seg-test.segment.json"))
            .expect("load persisted segment");

        let fields = loaded.get_document("doc-1").expect("document should exist");
        assert_eq!(
            fields.get("title").and_then(FieldValue::as_text),
            Some("Hello World")
        );
        assert_eq!(fields.get("year").and_then(FieldValue::as_i64), Some(2024));
    }
}
