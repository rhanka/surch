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
    aliases: BTreeMap<String, BTreeMap<String, Value>>,
    component_templates: BTreeMap<String, StoredComponentTemplate>,
    index_templates: BTreeMap<String, StoredIndexTemplate>,
}

#[derive(Debug, Default, Clone, PartialEq)]
struct InMemoryIndex {
    documents: BTreeMap<String, Value>,
    document_ids: BTreeMap<String, u32>,
    reverse_document_ids: BTreeMap<u32, String>,
    next_doc_id: u32,
    mapping: IndexMapping,
    settings: Value,
    index: DocumentIndex,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredDocument {
    pub index: String,
    pub id: String,
    pub source: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexMetadata {
    pub aliases: BTreeMap<String, Value>,
    pub mapping: Value,
    pub settings: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredComponentTemplate {
    pub component_template: Value,
    pub mapping: IndexMapping,
    pub settings: Value,
    pub aliases: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredIndexTemplate {
    pub index_template: Value,
    pub index_patterns: Vec<String>,
    pub composed_of: Vec<String>,
    pub mapping: IndexMapping,
    pub settings: Value,
    pub aliases: BTreeMap<String, Value>,
    pub priority: i64,
}

impl InMemoryIndex {
    fn new(mapping: IndexMapping, settings: Value) -> Self {
        Self {
            mapping,
            settings,
            next_doc_id: 0,
            ..Self::default()
        }
    }

    fn upsert_document(&mut self, id: &str, source: Value) {
        self.upsert_document_deferred(id, source);
        self.rebuild_index();
    }

    fn upsert_document_deferred(&mut self, id: &str, source: Value) {
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
    }

    fn delete_document(&mut self, id: &str) {
        if self.delete_document_deferred(id) {
            self.rebuild_index();
        }
    }

    fn delete_document_deferred(&mut self, id: &str) -> bool {
        if let Some(doc_id) = self.document_ids.remove(id) {
            self.documents.remove(id);
            self.reverse_document_ids.remove(&doc_id);
            return true;
        }
        false
    }

    fn mapping_value(&self) -> Value {
        self.mapping.as_value()
    }

    fn settings_value(&self) -> Value {
        self.settings.clone()
    }

    fn has_document(&self, id: &str) -> bool {
        self.document_ids.contains_key(id)
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        let documents = self
            .documents
            .iter()
            .filter_map(|(id, source)| {
                self.document_ids
                    .get(id)
                    .map(|doc_id| (*doc_id, indexed_fields_for_document(source, &self.mapping)))
            })
            .collect::<Vec<_>>();
        let _ = self
            .index
            .add_documents_with_mapping(documents, &self.mapping);
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

#[derive(Clone, Debug, PartialEq)]
pub enum DocumentWriteOperation {
    Index {
        index: String,
        id: String,
        source: Value,
        status: u16,
    },
    Create {
        index: String,
        id: String,
        source: Value,
    },
    Delete {
        index: String,
        id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentWriteResult {
    Applied {
        index: String,
        id: String,
        status: u16,
    },
    VersionConflict {
        index: String,
        id: String,
    },
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
    pub fn create_index(
        &self,
        index: &str,
        mapping: Option<IndexMapping>,
        settings: Value,
        aliases: BTreeMap<String, Value>,
    ) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");

        create_index_if_missing(
            &mut store,
            index,
            mapping.unwrap_or_default(),
            settings,
            aliases,
        );
    }

    pub fn put_index_template(
        &self,
        name: &str,
        mut template: StoredIndexTemplate,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        snapshot_component_templates(&mut template, &store.component_templates)?;
        store.index_templates.insert(name.to_owned(), template);
        Ok(())
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

    pub fn put_component_template(&self, name: &str, template: StoredComponentTemplate) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.component_templates.insert(name.to_owned(), template);
    }

    pub fn component_template(&self, name: &str) -> Option<StoredComponentTemplate> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.component_templates.get(name).cloned()
    }

    pub fn all_component_templates(&self) -> BTreeMap<String, StoredComponentTemplate> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.component_templates.clone()
    }

    pub fn delete_component_template(&self, name: &str) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.component_templates.remove(name).is_some()
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

        create_index_if_missing(
            &mut store,
            index,
            IndexMapping::default(),
            Value::Object(Default::default()),
            BTreeMap::new(),
        );
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

        create_index_if_missing(
            &mut store,
            index,
            IndexMapping::default(),
            Value::Object(Default::default()),
            BTreeMap::new(),
        );
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
            .or_insert_with(|| {
                InMemoryIndex::new(IndexMapping::default(), Value::Object(Default::default()))
            })
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

    pub fn apply_document_writes(
        &self,
        operations: Vec<DocumentWriteOperation>,
    ) -> Vec<DocumentWriteResult> {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        let mut touched = BTreeSet::new();
        let mut results = Vec::with_capacity(operations.len());

        for operation in operations {
            match operation {
                DocumentWriteOperation::Index {
                    index,
                    id,
                    source,
                    status,
                } => {
                    create_index_if_missing(
                        &mut store,
                        &index,
                        IndexMapping::default(),
                        Value::Object(Default::default()),
                        BTreeMap::new(),
                    );
                    let data = store
                        .indices
                        .get_mut(&index)
                        .expect("index must exist after implicit creation");
                    data.upsert_document_deferred(&id, source);
                    touched.insert(index.clone());
                    results.push(DocumentWriteResult::Applied { index, id, status });
                }
                DocumentWriteOperation::Create { index, id, source } => {
                    create_index_if_missing(
                        &mut store,
                        &index,
                        IndexMapping::default(),
                        Value::Object(Default::default()),
                        BTreeMap::new(),
                    );
                    let data = store
                        .indices
                        .get_mut(&index)
                        .expect("index must exist after implicit creation");
                    if data.has_document(&id) {
                        results.push(DocumentWriteResult::VersionConflict { index, id });
                    } else {
                        data.upsert_document_deferred(&id, source);
                        touched.insert(index.clone());
                        results.push(DocumentWriteResult::Applied {
                            index,
                            id,
                            status: 201,
                        });
                    }
                }
                DocumentWriteOperation::Delete { index, id } => {
                    if let Some(data) = store.indices.get_mut(&index) {
                        if data.delete_document_deferred(&id) {
                            touched.insert(index.clone());
                        }
                    }
                    results.push(DocumentWriteResult::Applied {
                        index,
                        id,
                        status: 200,
                    });
                }
            }
        }

        for index in touched {
            if let Some(data) = store.indices.get_mut(&index) {
                data.rebuild_index();
            }
        }

        results
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

    pub fn index_metadata(&self, index: &str) -> Option<IndexMetadata> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let data = store.indices.get(index)?;
        let aliases = store
            .aliases
            .iter()
            .filter_map(|(alias, indices)| {
                indices
                    .get(index)
                    .map(|definition| (alias.clone(), definition.clone()))
            })
            .collect();
        Some(IndexMetadata {
            aliases,
            mapping: data.mapping_value(),
            settings: data.settings_value(),
        })
    }

    pub fn index_names(&self) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.keys().cloned().collect()
    }

    pub fn add_alias(&self, index: &str, alias: &str) -> bool {
        self.add_alias_with_definition(index, alias, Value::Object(Default::default()))
    }

    pub fn add_alias_with_definition(&self, index: &str, alias: &str, definition: Value) -> bool {
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
            .insert(index.to_owned(), definition);
        true
    }

    pub fn remove_alias(&self, index: &str, alias: &str) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        let mut removed = false;
        if let Some(entry) = store.aliases.get_mut(alias) {
            removed = entry.remove(index).is_some();
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
            .filter(|(_, indices)| indices.contains_key(index))
            .map(|(alias, _)| alias.clone())
            .collect()
    }

    pub fn alias_definitions_for_index(&self, index: &str) -> BTreeMap<String, Value> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .aliases
            .iter()
            .filter_map(|(alias, indices)| {
                indices
                    .get(index)
                    .map(|definition| (alias.clone(), definition.clone()))
            })
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
            .map(|indices| indices.keys().cloned().collect())
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
        if let Some(indices) = store.aliases.get(target) {
            return match indices.len() {
                1 => Ok(indices.keys().next().expect("non-empty alias map").clone()),
                _ => {
                    let write_indices = indices
                        .iter()
                        .filter(|(_, definition)| {
                            definition
                                .get("is_write_index")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                        })
                        .map(|(index, _)| index.clone())
                        .collect::<Vec<_>>();
                    if write_indices.len() == 1 {
                        return Ok(write_indices[0].clone());
                    }
                    Err(format!(
                    "no write index is defined for alias [{target}], target alias must point to a single index"
                    ))
                }
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
            .map(|indices| indices.keys().cloned().collect())
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
            .map(|(alias, indices)| (alias.clone(), indices.keys().cloned().collect()))
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

fn create_index_if_missing(
    store: &mut MemoryStore,
    index: &str,
    explicit_mapping: IndexMapping,
    explicit_settings: Value,
    explicit_aliases: BTreeMap<String, Value>,
) {
    if store.indices.contains_key(index) {
        return;
    }

    let templates = matching_index_templates(index, &store.index_templates);
    let defaults = template_defaults_for_new_index(&templates);
    let mut mapping = defaults.mapping;
    merge_mapping_fields(&mut mapping, &explicit_mapping);
    let mut settings = defaults.settings;
    merge_settings(&mut settings, &explicit_settings);
    let mut aliases = defaults.aliases;
    aliases.extend(explicit_aliases);
    store
        .indices
        .insert(index.to_owned(), InMemoryIndex::new(mapping, settings));

    for (alias, definition) in aliases {
        store
            .aliases
            .entry(alias)
            .or_default()
            .insert(index.to_owned(), definition);
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

#[derive(Default)]
struct TemplateDefaults {
    mapping: IndexMapping,
    settings: Value,
    aliases: BTreeMap<String, Value>,
}

fn template_defaults_for_new_index(
    matching_templates: &[(&String, &StoredIndexTemplate)],
) -> TemplateDefaults {
    let mut defaults = TemplateDefaults {
        mapping: IndexMapping::default(),
        settings: Value::Object(Default::default()),
        aliases: BTreeMap::new(),
    };

    for (_, template) in matching_templates {
        merge_mapping_fields(&mut defaults.mapping, &template.mapping);
        merge_settings(&mut defaults.settings, &template.settings);
        defaults.aliases.extend(template.aliases.clone());
    }
    defaults
}

fn snapshot_component_templates(
    template: &mut StoredIndexTemplate,
    component_templates: &BTreeMap<String, StoredComponentTemplate>,
) -> Result<(), String> {
    let inline_mapping = template.mapping.clone();
    let inline_settings = template.settings.clone();
    let inline_aliases = template.aliases.clone();

    template.mapping = IndexMapping::default();
    template.settings = Value::Object(Default::default());
    template.aliases.clear();

    for component_name in &template.composed_of {
        let component = component_templates
            .get(component_name)
            .ok_or_else(|| component_name.clone())?;
        merge_mapping_fields(&mut template.mapping, &component.mapping);
        merge_settings(&mut template.settings, &component.settings);
        template.aliases.extend(component.aliases.clone());
    }

    merge_mapping_fields(&mut template.mapping, &inline_mapping);
    merge_settings(&mut template.settings, &inline_settings);
    template.aliases.extend(inline_aliases);
    Ok(())
}

fn merge_mapping_fields(target: &mut IndexMapping, source: &IndexMapping) {
    for (field, mapping) in source.fields() {
        target.set_field_mapping(field.to_owned(), *mapping);
    }
}

fn merge_settings(target: &mut Value, source: &Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };

    for (key, value) in source {
        match (target.get_mut(key), value) {
            (Some(target_value), Value::Object(_)) if target_value.is_object() => {
                merge_settings(target_value, value);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
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
