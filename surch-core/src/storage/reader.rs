use super::{error::Error, Segment, SegmentManager};
use crate::common::Document;
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

const SEGMENT_FILE_SUFFIX: &str = ".segment.json";

pub struct IndexReader {
    path: PathBuf,
    segments: Arc<RwLock<SegmentManager>>,
}

impl IndexReader {
    pub fn new(path: impl Into<PathBuf>, _index_name: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            segments: Arc::new(RwLock::new(SegmentManager::new())),
        }
    }

    pub fn load_segments(&mut self) -> Result<(), Error> {
        let mut manager = SegmentManager::new();

        if self.path.exists() {
            let mut segment_paths: Vec<PathBuf> = std::fs::read_dir(&self.path)?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.ends_with(SEGMENT_FILE_SUFFIX))
                        .unwrap_or(false)
                })
                .collect();

            segment_paths.sort();

            for segment_path in segment_paths {
                manager.add_segment(Segment::load(segment_path)?);
            }
        }

        *self.segments.write() = manager;
        Ok(())
    }

    pub fn get_document(&self, doc_id: &str) -> Option<Document> {
        for segment in self.segments.read().segments() {
            if let Some(fields) = segment.get_document(doc_id) {
                return Some(Document {
                    id: doc_id.to_string(),
                    fields,
                    version: None,
                    seq_no: None,
                    primary_term: None,
                });
            }
        }
        None
    }

    pub fn search(&self, field: &str, term: &str) -> Vec<u64> {
        self.segments
            .read()
            .segments()
            .iter()
            .flat_map(|s| s.search_term(field, term))
            .collect()
    }

    pub fn get_all_documents(&self) -> Vec<Document> {
        self.segments
            .read()
            .segments()
            .iter()
            .flat_map(|s| {
                s.all_documents()
                    .into_iter()
                    .map(|(doc_id, fields)| Document {
                        id: doc_id,
                        fields,
                        version: None,
                        seq_no: None,
                        primary_term: None,
                    })
            })
            .collect()
    }

    pub fn num_docs(&self) -> u64 {
        self.segments.read().total_docs()
    }

    pub fn num_segments(&self) -> usize {
        self.segments.read().num_segments()
    }

    pub fn set_segments(&self, segments: Vec<Segment>) {
        let mut manager = self.segments.write();
        for seg in segments {
            manager.add_segment(seg);
        }
    }
}
