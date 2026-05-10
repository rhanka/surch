use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use serde_json::Value;
use surch_index::{document_index::DocumentIndex, mapping::IndexMapping};

/// Shared in-memory API state used by API handlers.
#[derive(Clone, Default)]
pub struct AppState {
    store: Arc<RwLock<MemoryStore>>,
}

#[derive(Default)]
struct MemoryStore {
    indices: BTreeMap<String, InMemoryIndex>,
}

#[derive(Debug, Default, Clone, PartialEq)]
struct InMemoryIndex {
    documents: BTreeMap<String, Value>,
    document_ids: BTreeMap<String, u32>,
    reverse_document_ids: BTreeMap<u32, String>,
    next_doc_id: u32,
    mapping: IndexMapping,
    index: DocumentIndex,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredDocument {
    pub index: String,
    pub id: String,
    pub source: Value,
}

impl InMemoryIndex {
    fn new(mapping: IndexMapping) -> Self {
        Self {
            mapping,
            next_doc_id: 0,
            ..Self::default()
        }
    }

    fn upsert_document(&mut self, id: &str, source: Value) {
        self.document_ids.entry(id.to_owned()).or_insert_with(|| {
            let doc_id = self.next_doc_id;
            self.next_doc_id += 1;
            self.reverse_document_ids.insert(doc_id, id.to_owned());
            doc_id
        });

        self.documents.insert(id.to_owned(), source);
        let inserted_source = self
            .documents
            .get(id)
            .expect("document must exist after insertion");
        self.mapping.ensure_fields(inserted_source);
        self.rebuild_index();
    }

    fn delete_document(&mut self, id: &str) {
        if let Some(doc_id) = self.document_ids.remove(id) {
            self.documents.remove(id);
            self.reverse_document_ids.remove(&doc_id);
            self.rebuild_index();
        }
    }

    fn mapping_value(&self) -> Value {
        self.mapping.as_value()
    }

    fn has_document(&self, id: &str) -> bool {
        self.document_ids.contains_key(id)
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        for (id, source) in &self.documents {
            if let Some(doc_id) = self.document_ids.get(id) {
                let fields = indexed_fields_for_document(source, &self.mapping);
                let _ = self
                    .index
                    .add_document_with_mapping(*doc_id, fields, &self.mapping);
            }
        }
    }

    fn set_mapping(&mut self, mapping: IndexMapping) {
        self.mapping = mapping;
        self.rebuild_index();
    }

    fn term_hits(&self, field: &str, value: &str) -> Vec<String> {
        if field.trim().is_empty() || value.is_empty() {
            return Vec::new();
        }

        let token = normalized_term_for_field(value, field, &self.mapping);
        if token.is_empty() {
            return Vec::new();
        }

        self.index
            .postings(field, &token)
            .into_iter()
            .flat_map(|postings| postings.map(|posting| posting.doc_id))
            .filter_map(|doc_id| self.reverse_document_ids.get(&doc_id).cloned())
            .collect()
    }

    fn count_term_hits(&self, field: &str, value: &str) -> usize {
        self.term_hits(field, value).len()
    }
}

fn normalized_term_for_field(value: &str, field: &str, mapping: &IndexMapping) -> String {
    mapping.analyzer(field).first_term(value)
}

fn indexed_fields_for_document(document: &Value, mapping: &IndexMapping) -> Vec<(String, String)> {
    let Some(object) = document.as_object() else {
        return Vec::new();
    };

    object
        .iter()
        .flat_map(|(name, value)| {
            let values = scalar_values(value, mapping, name);
            values.into_iter().map(move |value| (name.clone(), value))
        })
        .collect()
}

fn scalar_values(document: &Value, mapping: &IndexMapping, field: &str) -> Vec<String> {
    match document {
        Value::String(value) => vec![value.clone()],
        Value::Number(value) => vec![value.to_string()],
        Value::Bool(value) => vec![value.to_string()],
        Value::Array(values) => values
            .iter()
            .flat_map(|value| scalar_values(value, mapping, field))
            .collect(),
        Value::Object(value) if mapping.field(field).is_some() => {
            serde_json::to_string(value).map_or_else(|_| Vec::new(), |encoded| vec![encoded])
        }
        Value::Object(_) => Vec::new(),
        Value::Null => Vec::new(),
    }
}

impl AppState {
    pub fn create_index(&self, index: &str, mapping: Option<IndexMapping>) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");

        store
            .indices
            .entry(index.to_owned())
            .or_insert_with(|| InMemoryIndex::new(mapping.unwrap_or_default()));
    }

    pub fn delete_index(&self, index: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.remove(index);
    }

    pub fn refresh_index(&self, _index: &str) {}

    pub fn index_exists(&self, index: &str) -> bool {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.contains_key(index)
    }

    pub fn index_document(&self, index: &str, id: &str, source: Value) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");

        let data = store
            .indices
            .entry(index.to_owned())
            .or_insert_with(|| InMemoryIndex::new(IndexMapping::default()));
        data.upsert_document(id, source);
    }

    pub fn create_document(&self, index: &str, id: &str, source: Value) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");

        let data = store
            .indices
            .entry(index.to_owned())
            .or_insert_with(|| InMemoryIndex::new(IndexMapping::default()));
        if data.has_document(id) {
            return false;
        }

        data.upsert_document(id, source);
        true
    }

    pub fn set_mapping(&self, index: &str, mapping: IndexMapping) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .entry(index.to_owned())
            .or_insert_with(|| InMemoryIndex::new(IndexMapping::default()))
            .set_mapping(mapping);
    }

    pub fn delete_document(&self, index: &str, id: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            data.delete_document(id);
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
            .map_or(0, |index| index.documents.len() as u64)
    }

    pub fn mapping(&self, index: &str) -> Option<Value> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.get(index).map(|data| data.mapping_value())
    }

    pub fn all_mappings(&self) -> BTreeMap<String, Value> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .iter()
            .map(|(index, data)| (index.clone(), data.mapping_value()))
            .collect()
    }

    pub fn get_document(&self, index: &str, id: &str) -> Option<Value> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .and_then(|data| data.documents.get(id).cloned())
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
            .flat_map(|data| {
                data.documents.iter().map(|(id, source)| StoredDocument {
                    index: index.to_owned(),
                    id: id.clone(),
                    source: source.clone(),
                })
            })
            .collect()
    }

    pub fn documents_for_term(&self, index: &str, field: &str, value: &str) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or_else(Vec::new, |data| data.term_hits(field, value))
    }

    pub fn term_matches_count(&self, index: &str, field: &str, value: &str) -> usize {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or(0, |data| data.count_term_hits(field, value))
    }
}
