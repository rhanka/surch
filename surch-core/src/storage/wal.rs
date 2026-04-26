use super::error::Error;
use crate::common::Document;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;

const WAL_FILE_NAME: &str = "wal.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    pub seq: u64,
    pub index: String,
    pub doc_id: String,
    pub operation: WalOperation,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        let entries = Self::load_entries(&path)?;
        let current_seq = entries.last().map(|entry| entry.seq).unwrap_or(0);

        Ok(Self {
            path,
            entries: Arc::new(RwLock::new(entries)),
            current_seq: Arc::new(RwLock::new(current_seq)),
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
        let wal_path = self.path.join(WAL_FILE_NAME);
        let mut file = File::create(wal_path)?;

        for entry in self.entries.read().iter() {
            serde_json::to_writer(&mut file, entry)
                .map_err(|err| Error::Serialization(err.to_string()))?;
            file.write_all(b"\n")?;
        }

        file.flush()?;
        Ok(())
    }

    fn load_entries(path: &std::path::Path) -> Result<Vec<WalEntry>, Error> {
        let wal_path = path.join(WAL_FILE_NAME);
        if !wal_path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(wal_path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let entry = serde_json::from_str::<WalEntry>(&line)
                .map_err(|err| Error::Corruption(err.to_string()))?;
            entries.push(entry);
        }

        Ok(entries)
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

#[cfg(test)]
mod tests {
    use super::{WalOperation, WriteAheadLog};
    use crate::common::{Document, FieldValue};
    use tempfile::tempdir;

    fn sample_document(id: &str) -> Document {
        Document::new(id).with_field("title", FieldValue::Text("hello wal".to_string()))
    }

    #[test]
    fn replays_flushed_entries_after_reopen() {
        let temp_dir = tempdir().expect("temp dir");
        let wal = WriteAheadLog::new(temp_dir.path()).expect("create wal");

        wal.append(
            "books",
            "doc-1",
            WalOperation::Index(sample_document("doc-1")),
        )
        .expect("append first entry");
        wal.append(
            "books",
            "doc-1",
            WalOperation::Delete {
                doc_id: "doc-1".to_string(),
            },
        )
        .expect("append delete entry");

        wal.flush().expect("flush wal");

        let reopened = WriteAheadLog::new(temp_dir.path()).expect("reopen wal");
        let entries = reopened.read_all();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].seq, 1);
        assert_eq!(entries[1].seq, 2);
        assert_eq!(entries[0].index, "books");
        assert_eq!(entries[0].doc_id, "doc-1");

        match &entries[0].operation {
            WalOperation::Index(doc) => {
                assert_eq!(doc.id, "doc-1");
                assert_eq!(doc.get_text("title"), Some("hello wal".to_string()));
            }
            other => panic!("expected index operation, got {other:?}"),
        }

        match &entries[1].operation {
            WalOperation::Delete { doc_id } => assert_eq!(doc_id, "doc-1"),
            other => panic!("expected delete operation, got {other:?}"),
        }
    }

    #[test]
    fn append_continues_sequence_after_replay() {
        let temp_dir = tempdir().expect("temp dir");
        let wal = WriteAheadLog::new(temp_dir.path()).expect("create wal");

        wal.append(
            "books",
            "doc-1",
            WalOperation::Index(sample_document("doc-1")),
        )
        .expect("append first entry");
        wal.flush().expect("flush wal");

        let reopened = WriteAheadLog::new(temp_dir.path()).expect("reopen wal");
        let next_seq = reopened
            .append(
                "books",
                "doc-2",
                WalOperation::Index(sample_document("doc-2")),
            )
            .expect("append after replay");

        assert_eq!(next_seq, 2);
    }

    #[test]
    fn reopening_empty_wal_returns_no_entries() {
        let temp_dir = tempdir().expect("temp dir");

        let wal = WriteAheadLog::new(temp_dir.path()).expect("create wal");
        assert!(wal.read_all().is_empty());

        wal.flush().expect("flush wal");

        let reopened = WriteAheadLog::new(temp_dir.path()).expect("reopen wal");
        assert!(reopened.read_all().is_empty());
    }
}
