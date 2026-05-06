use std::collections::BTreeMap;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, StoredFieldsError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StoredFieldsError {
    #[error("duplicate doc id: {doc_id}")]
    DuplicateDocId { doc_id: u32 },
    #[error("stored field name must not be empty")]
    EmptyFieldName,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StoredValue {
    String(String),
    Number(i64),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoredDocument {
    fields: BTreeMap<String, StoredValue>,
}

impl StoredDocument {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_fields<I, K>(fields: I) -> Self
    where
        I: IntoIterator<Item = (K, StoredValue)>,
        K: Into<String>,
    {
        let mut document = Self::new();
        for (name, value) in fields {
            document
                .insert(name, value)
                .expect("stored field name must not be empty");
        }
        document
    }

    pub fn insert(&mut self, name: impl Into<String>, value: StoredValue) -> Result<()> {
        let name = name.into();
        if name.is_empty() {
            return Err(StoredFieldsError::EmptyFieldName);
        }

        self.fields.insert(name, value);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&StoredValue> {
        self.fields.get(name)
    }

    pub fn field_names(&self) -> Vec<&str> {
        self.fields.keys().map(String::as_str).collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &StoredValue)> {
        self.fields
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoredFieldsWriter {
    documents: BTreeMap<u32, StoredDocument>,
}

impl StoredFieldsWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_document(&mut self, doc_id: u32, document: StoredDocument) -> Result<()> {
        if self.documents.contains_key(&doc_id) {
            return Err(StoredFieldsError::DuplicateDocId { doc_id });
        }

        self.documents.insert(doc_id, document);
        Ok(())
    }

    pub fn finish(self) -> StoredFieldsReader {
        StoredFieldsReader {
            documents: self.documents,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoredFieldsReader {
    documents: BTreeMap<u32, StoredDocument>,
}

impl StoredFieldsReader {
    pub fn get(&self, doc_id: u32) -> Option<&StoredDocument> {
        self.documents.get(&doc_id)
    }

    pub fn doc_ids(&self) -> Vec<u32> {
        self.documents.keys().copied().collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &StoredDocument)> {
        self.documents
            .iter()
            .map(|(doc_id, document)| (*doc_id, document))
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
}
