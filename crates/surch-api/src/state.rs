use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use serde_json::Value;

/// Shared in-memory API state used by compatibility oracle replays.
#[derive(Clone, Default)]
pub struct AppState {
    store: Arc<RwLock<MemoryStore>>,
}

#[derive(Default)]
struct MemoryStore {
    indices: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredDocument {
    pub index: String,
    pub id: String,
    pub source: Value,
}

impl AppState {
    pub fn create_index(&self, index: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.entry(index.to_owned()).or_default();
    }

    pub fn delete_index(&self, index: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.remove(index);
    }

    pub fn refresh_index(&self, _index: &str) {}

    pub fn index_document(&self, index: &str, id: &str, source: Value) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .entry(index.to_owned())
            .or_default()
            .insert(id.to_owned(), source);
    }

    pub fn delete_document(&self, index: &str, id: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(documents) = store.indices.get_mut(index) {
            documents.remove(id);
        }
    }

    pub fn count(&self, index: &str) -> u64 {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map(|documents| documents.len() as u64)
            .unwrap_or(0)
    }

    pub fn documents(&self, index: &str) -> Vec<StoredDocument> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");

        store
            .indices
            .get(index)
            .into_iter()
            .flat_map(|documents| {
                documents.iter().map(|(id, source)| StoredDocument {
                    index: index.to_owned(),
                    id: id.clone(),
                    source: source.clone(),
                })
            })
            .collect()
    }
}
