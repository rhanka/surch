use super::{error::Error, IndexReader, IndexWriter, WalManager};
use crate::common::{Document, IndexMetadata};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const SEGMENT_FILE_SUFFIX: &str = ".segment.json";

pub struct IndexStore {
    base_path: PathBuf,
    indexes: Arc<RwLock<HashMap<String, IndexInstance>>>,
    wal: WalManager,
}

struct IndexInstance {
    metadata: IndexMetadata,
    writer: IndexWriter,
    readers: Vec<IndexReader>,
}

impl IndexStore {
    pub fn new(base_path: impl Into<PathBuf>) -> Result<Self, Error> {
        let base_path = base_path.into();
        std::fs::create_dir_all(&base_path)?;

        let wal = WalManager::new();
        wal.init(base_path.join("wal"))?;

        Ok(Self {
            base_path,
            indexes: Arc::new(RwLock::new(HashMap::new())),
            wal,
        })
    }

    pub fn create_index(&self, metadata: IndexMetadata) -> Result<(), Error> {
        let index_path = self.base_path.join(&metadata.name);
        std::fs::create_dir_all(&index_path)?;

        let writer = IndexWriter::new(index_path.join("data"), &metadata.name, self.wal.clone())?;

        let instance = IndexInstance {
            metadata,
            writer,
            readers: Vec::new(),
        };

        self.indexes
            .write()
            .insert(instance.metadata.name.clone(), instance);

        Ok(())
    }

    pub fn delete_index(&self, name: &str) -> Result<(), Error> {
        let mut indexes = self.indexes.write();
        if indexes.remove(name).is_some() {
            let index_path = self.base_path.join(name);
            if index_path.exists() {
                std::fs::remove_dir_all(index_path)?;
            }
            Ok(())
        } else {
            Err(Error::IndexNotFound(name.to_string()))
        }
    }

    pub fn get_index(&self, name: &str) -> Option<IndexMetadata> {
        self.indexes.read().get(name).map(|i| i.metadata.clone())
    }

    pub fn index_document(&self, index_name: &str, doc: Document) -> Result<u64, Error> {
        let mut indexes = self.indexes.write();
        let instance = indexes
            .get_mut(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        instance.writer.index_document(doc)
    }

    pub fn get_document(&self, index_name: &str, doc_id: &str) -> Result<Option<Document>, Error> {
        let indexes = self.indexes.read();
        let instance = indexes
            .get(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        let mut reader = IndexReader::new(self.base_path.join(index_name).join("data"), index_name);
        reader.load_segments()?;

        Ok(reader.get_document(doc_id))
    }

    pub fn delete_document(&self, index_name: &str, doc_id: &str) -> Result<bool, Error> {
        let indexes = self.indexes.read();
        let _instance = indexes
            .get(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        let data_path = self.base_path.join(index_name).join("data");
        if !data_path.exists() {
            return Ok(false);
        }

        let mut segment_paths: Vec<PathBuf> = std::fs::read_dir(&data_path)?
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
            let mut segment = super::Segment::load(&segment_path)?;
            if segment.remove_document(doc_id) {
                segment.persist()?;
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn list_indexes(&self) -> Vec<IndexMetadata> {
        self.indexes
            .read()
            .values()
            .map(|i| i.metadata.clone())
            .collect()
    }

    pub fn get_all_documents(&self, index_name: &str) -> Result<Vec<Document>, Error> {
        let indexes = self.indexes.read();
        let instance = indexes
            .get(index_name)
            .ok_or_else(|| Error::IndexNotFound(index_name.to_string()))?;

        let mut reader = IndexReader::new(self.base_path.join(index_name).join("data"), index_name);
        reader.load_segments()?;

        Ok(reader.get_all_documents())
    }

    pub fn refresh(&self, index_name: &str) -> Result<(), Error> {
        Ok(())
    }

    pub fn flush(&self, index_name: &str) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::IndexStore;
    use crate::common::{Document, FieldValue, IndexMetadata};
    use tempfile::tempdir;

    #[test]
    fn index_document_round_trips_from_store() {
        let temp_dir = tempdir().expect("temp dir");
        let store = IndexStore::new(temp_dir.path()).expect("create store");

        store
            .create_index(IndexMetadata::new("books"))
            .expect("create index");

        store
            .index_document(
                "books",
                Document::new("doc-1")
                    .with_field("title", FieldValue::Text("Hello".to_string()))
                    .with_field("year", FieldValue::Integer(2024)),
            )
            .expect("index document");

        let doc = store
            .get_document("books", "doc-1")
            .expect("get document result")
            .expect("document should exist");

        assert_eq!(doc.get_text("title"), Some("Hello".to_string()));
        assert_eq!(
            doc.get_field("year").and_then(FieldValue::as_i64),
            Some(2024)
        );
    }
}
