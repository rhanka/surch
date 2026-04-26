use super::{error::Error, Segment, SegmentManager, WalManager};
use crate::common::Document;
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

const SEGMENT_FILE_SUFFIX: &str = ".segment.json";

pub struct IndexWriter {
    path: PathBuf,
    segments: Arc<RwLock<SegmentManager>>,
    wal: WalManager,
    index_name: String,
    current_doc_id: u64,
}

impl IndexWriter {
    pub fn new(
        path: impl Into<PathBuf>,
        index_name: impl Into<String>,
        wal: WalManager,
    ) -> Result<Self, Error> {
        let path = path.into();
        std::fs::create_dir_all(&path)?;

        let current_doc_id = std::fs::read_dir(&path)?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.ends_with(SEGMENT_FILE_SUFFIX))
                    .unwrap_or(false)
            })
            .count() as u64;

        Ok(Self {
            path,
            segments: Arc::new(RwLock::new(SegmentManager::new())),
            wal,
            index_name: index_name.into(),
            current_doc_id,
        })
    }

    pub fn index_document(&mut self, doc: Document) -> Result<u64, Error> {
        self.current_doc_id += 1;
        let doc_id = self.current_doc_id;

        self.wal.write_index(&self.index_name, doc.clone())?;

        let mut segment = Segment::new(format!("seg_{}", uuid::Uuid::new_v4()), &self.path);

        segment.add_document(doc_id, &doc.id, &doc.fields)?;
        segment.persist()?;

        self.segments.write().add_segment(segment);

        Ok(doc_id)
    }

    pub fn delete_document(&mut self, doc_id: &str) -> Result<(), Error> {
        self.wal.write_delete(&self.index_name, doc_id)?;
        Ok(())
    }

    pub fn commit(&self) -> Result<(), Error> {
        self.segments.read().segments().iter().for_each(|s| {
            tracing::debug!("Committing segment: {}", s.meta.id);
        });
        Ok(())
    }

    pub fn segments(&self) -> Vec<super::Segment> {
        self.segments.read().segments().to_vec()
    }

    pub fn refresh(&self) -> Result<(), Error> {
        Ok(())
    }
}
