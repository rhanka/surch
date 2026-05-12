use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

use serde_json::Value;
use surch_index::{
    document_index::DocumentIndex,
    mapping::{FieldMapping, IndexMapping},
};

/// Shared in-memory API state used by API handlers.
#[derive(Clone, Default)]
pub struct AppState {
    store: Arc<RwLock<MemoryStore>>,
}

#[derive(Default)]
struct MemoryStore {
    indices: BTreeMap<String, InMemoryIndex>,
    aliases: BTreeMap<String, BTreeSet<String>>,
    index_templates: BTreeMap<String, StoredIndexTemplate>,
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

#[derive(Clone, Debug, PartialEq)]
pub struct StoredIndexTemplate {
    pub index_template: Value,
    pub index_patterns: Vec<String>,
    pub mapping: IndexMapping,
    pub aliases: Vec<String>,
    pub priority: i64,
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

        create_index_if_missing(&mut store, index, mapping.unwrap_or_default());
    }

    pub fn put_index_template(&self, name: &str, template: StoredIndexTemplate) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.index_templates.insert(name.to_owned(), template);
    }

    pub fn index_template(&self, name: &str) -> Option<StoredIndexTemplate> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.index_templates.get(name).cloned()
    }

    pub fn all_index_templates(&self) -> BTreeMap<String, StoredIndexTemplate> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.index_templates.clone()
    }

    pub fn delete_index_template(&self, name: &str) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.index_templates.remove(name).is_some()
    }

    pub fn delete_index(&self, index: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.remove(index);
        let stale_aliases: Vec<String> = store
            .aliases
            .iter_mut()
            .filter_map(|(alias, indices)| {
                indices.remove(index);
                indices.is_empty().then(|| alias.clone())
            })
            .collect();
        for alias in stale_aliases {
            store.aliases.remove(&alias);
        }
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

        create_index_if_missing(&mut store, index, IndexMapping::default());
        let data = store
            .indices
            .get_mut(index)
            .expect("index must exist after implicit creation");
        data.upsert_document(id, source);
    }

    pub fn create_document(&self, index: &str, id: &str, source: Value) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");

        create_index_if_missing(&mut store, index, IndexMapping::default());
        let data = store
            .indices
            .get_mut(index)
            .expect("index must exist after implicit creation");
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

    /// Merge the supplied field mappings into the existing index mapping.
    ///
    /// Returns the field name on the first type conflict; new fields are appended.
    pub fn merge_field_mappings(
        &self,
        index: &str,
        new_fields: &[(String, FieldMapping)],
    ) -> Result<(), String> {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        let Some(data) = store.indices.get_mut(index) else {
            return Err(format!("index [{index}] missing"));
        };

        let mut merged = data.mapping.clone();
        for (field, mapping) in new_fields {
            if let Some(existing) = merged.field(field) {
                if existing.field_type != mapping.field_type {
                    return Err(format!(
                        "mapper [{field}] of different type, current_type [{}], merged_type [{}]",
                        existing.field_type.as_str(),
                        mapping.field_type.as_str(),
                    ));
                }
            }
            merged.set_field_mapping(field.clone(), *mapping);
        }

        data.set_mapping(merged);
        Ok(())
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

    pub fn index_names(&self) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.keys().cloned().collect()
    }

    pub fn add_alias(&self, index: &str, alias: &str) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if !store.indices.contains_key(index) {
            return false;
        }
        store
            .aliases
            .entry(alias.to_owned())
            .or_default()
            .insert(index.to_owned());
        true
    }

    pub fn remove_alias(&self, index: &str, alias: &str) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        let mut removed = false;
        if let Some(entry) = store.aliases.get_mut(alias) {
            removed = entry.remove(index);
            if entry.is_empty() {
                store.aliases.remove(alias);
            }
        }
        removed
    }

    pub fn alias_exists(&self, alias: &str) -> bool {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.aliases.contains_key(alias)
    }

    pub fn aliases_for_index(&self, index: &str) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .aliases
            .iter()
            .filter(|(_, indices)| indices.contains(index))
            .map(|(alias, _)| alias.clone())
            .collect()
    }

    pub fn indices_for_alias(&self, alias: &str) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .aliases
            .get(alias)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Resolve a write-side path target to a single physical index name.
    ///
    /// - Existing index → returns that index.
    /// - Unknown name (will be implicitly created) → returns the name as-is.
    /// - Alias pointing to exactly one index → returns that index.
    /// - Alias pointing to several indices → `Err` with the OpenSearch reason.
    pub fn resolve_write_target(&self, target: &str) -> Result<String, String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        if store.indices.contains_key(target) {
            return Ok(target.to_owned());
        }
        if let Some(set) = store.aliases.get(target) {
            return match set.len() {
                1 => Ok(set.iter().next().expect("non-empty set").clone()),
                _ => Err(format!(
                    "no write index is defined for alias [{target}], target alias must point to a single index"
                )),
            };
        }
        Ok(target.to_owned())
    }

    /// Resolve a path-level target into the set of physical indices it points to.
    ///
    /// - Existing index name → `[name]`.
    /// - Known alias → the list of indices the alias points to.
    /// - Unknown name → empty.
    pub fn resolve_index(&self, target: &str) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        if store.indices.contains_key(target) {
            return vec![target.to_owned()];
        }
        store
            .aliases
            .get(target)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn all_aliases(&self) -> BTreeMap<String, Vec<String>> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .aliases
            .iter()
            .map(|(alias, indices)| (alias.clone(), indices.iter().cloned().collect()))
            .collect()
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

fn create_index_if_missing(store: &mut MemoryStore, index: &str, explicit_mapping: IndexMapping) {
    if store.indices.contains_key(index) {
        return;
    }

    let templates = matching_index_templates(index, &store.index_templates);
    let mapping = mapping_for_new_index(&templates, explicit_mapping);
    store
        .indices
        .insert(index.to_owned(), InMemoryIndex::new(mapping));

    for (_, template) in templates {
        for alias in &template.aliases {
            store
                .aliases
                .entry(alias.clone())
                .or_default()
                .insert(index.to_owned());
        }
    }
}

fn matching_index_templates<'a>(
    index: &str,
    index_templates: &'a BTreeMap<String, StoredIndexTemplate>,
) -> Vec<(&'a String, &'a StoredIndexTemplate)> {
    let mut matching_templates = index_templates
        .iter()
        .filter(|(_, template)| {
            template
                .index_patterns
                .iter()
                .any(|pattern| index_pattern_matches(pattern, index))
        })
        .collect::<Vec<_>>();

    matching_templates.sort_by(|(left_name, left), (right_name, right)| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left_name.cmp(right_name))
    });
    matching_templates
}

fn mapping_for_new_index(
    matching_templates: &[(&String, &StoredIndexTemplate)],
    explicit_mapping: IndexMapping,
) -> IndexMapping {
    let mut mapping = IndexMapping::default();
    for (_, template) in matching_templates {
        merge_mapping_fields(&mut mapping, &template.mapping);
    }
    merge_mapping_fields(&mut mapping, &explicit_mapping);
    mapping
}

fn merge_mapping_fields(target: &mut IndexMapping, source: &IndexMapping) {
    for (field, mapping) in source.fields() {
        target.set_field_mapping(field.to_owned(), *mapping);
    }
}

fn index_pattern_matches(pattern: &str, index: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let index = index.chars().collect::<Vec<_>>();
    let mut matches = vec![vec![false; index.len() + 1]; pattern.len() + 1];
    matches[0][0] = true;

    for pattern_index in 1..=pattern.len() {
        if pattern[pattern_index - 1] == '*' {
            matches[pattern_index][0] = matches[pattern_index - 1][0];
        }
    }

    for pattern_index in 1..=pattern.len() {
        for index_index in 1..=index.len() {
            matches[pattern_index][index_index] = match pattern[pattern_index - 1] {
                '*' => {
                    matches[pattern_index - 1][index_index]
                        || matches[pattern_index][index_index - 1]
                }
                '?' => matches[pattern_index - 1][index_index - 1],
                character => {
                    character == index[index_index - 1]
                        && matches[pattern_index - 1][index_index - 1]
                }
            };
        }
    }

    matches[pattern.len()][index.len()]
}
