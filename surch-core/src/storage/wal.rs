use super::error::Error;
use crate::common::{Document, FieldValue};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct WalEntry {
    pub seq: u64,
    pub index: String,
    pub doc_id: String,
    pub operation: WalOperation,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub enum WalOperation {
    Index(Document),
    Delete { doc_id: String },
}

#[derive(Clone)]
pub struct WriteAheadLog {
    path: PathBuf,
    entries: Arc<RwLock<Vec<WalEntry>>>,
    current_seq: Arc<RwLock<u64>>,
}

impl WriteAheadLog {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        std::fs::create_dir_all(&path)?;

        Ok(Self {
            path,
            entries: Arc::new(RwLock::new(Vec::new())),
            current_seq: Arc::new(RwLock::new(0)),
        })
    }

    pub fn append(
        &self,
        index: impl Into<String>,
        doc_id: impl Into<String>,
        operation: WalOperation,
    ) -> Result<u64, Error> {
        let seq = {
            let mut current = self.current_seq.write();
            *current += 1;
            *current
        };

        let entry = WalEntry {
            seq,
            index: index.into(),
            doc_id: doc_id.into(),
            operation,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        self.entries.write().push(entry);

        Ok(seq)
    }

    pub fn read_all(&self) -> Vec<WalEntry> {
        self.entries.read().clone()
    }

    pub fn clear(&self) {
        self.entries.write().clear();
    }

    pub fn flush(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct WalManager {
    wal: Arc<RwLock<Option<WriteAheadLog>>>,
}

impl WalManager {
    pub fn new() -> Self {
        Self {
            wal: Arc::new(RwLock::new(None)),
        }
    }

    pub fn init(&self, path: impl Into<PathBuf>) -> Result<(), Error> {
        let wal = WriteAheadLog::new(path)?;
        *self.wal.write() = Some(wal);
        Ok(())
    }

    pub fn write_index(&self, index: &str, doc: Document) -> Result<u64, Error> {
        let wal = self.wal.read();
        let wal = wal
            .as_ref()
            .ok_or_else(|| Error::Wal("WAL not initialized".to_string()))?;

        wal.append(index, doc.id.clone(), WalOperation::Index(doc))
    }

    pub fn write_delete(&self, index: &str, doc_id: &str) -> Result<u64, Error> {
        let wal = self.wal.read();
        let wal = wal
            .as_ref()
            .ok_or_else(|| Error::Wal("WAL not initialized".to_string()))?;

        wal.append(
            index,
            doc_id.to_string(),
            WalOperation::Delete {
                doc_id: doc_id.to_string(),
            },
        )
    }
}

impl Default for WalManager {
    fn default() -> Self {
        Self::new()
    }
}
