use std::collections::BTreeMap;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, FieldInfosError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FieldInfosError {
    #[error("duplicate field name: {name}")]
    DuplicateFieldName { name: String },
    #[error("duplicate field number: {number}")]
    DuplicateFieldNumber { number: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexOptions {
    None,
    Docs,
    DocsAndFreqs,
    DocsAndFreqsAndPositions,
    DocsAndFreqsAndPositionsAndOffsets,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocValuesType {
    None,
    Numeric,
    Binary,
    Sorted,
    SortedNumeric,
    SortedSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FieldInfo {
    name: String,
    number: u32,
    index_options: IndexOptions,
    doc_values_type: DocValuesType,
    omit_norms: bool,
    store_payloads: bool,
}

impl FieldInfo {
    pub fn new(
        name: impl Into<String>,
        number: u32,
        index_options: IndexOptions,
        doc_values_type: DocValuesType,
        omit_norms: bool,
        store_payloads: bool,
    ) -> Self {
        Self {
            name: name.into(),
            number,
            index_options,
            doc_values_type,
            omit_norms,
            store_payloads,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn number(&self) -> u32 {
        self.number
    }

    pub fn index_options(&self) -> IndexOptions {
        self.index_options
    }

    pub fn doc_values_type(&self) -> DocValuesType {
        self.doc_values_type
    }

    pub fn omit_norms(&self) -> bool {
        self.omit_norms
    }

    pub fn store_payloads(&self) -> bool {
        self.store_payloads
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfos {
    fields: Vec<FieldInfo>,
    by_name: BTreeMap<String, usize>,
    by_number: BTreeMap<u32, usize>,
}

impl FieldInfos {
    pub fn new(fields: Vec<FieldInfo>) -> Result<Self> {
        let mut by_name = BTreeMap::new();
        let mut by_number = BTreeMap::new();

        for (position, field) in fields.iter().enumerate() {
            if by_name.insert(field.name.clone(), position).is_some() {
                return Err(FieldInfosError::DuplicateFieldName {
                    name: field.name.clone(),
                });
            }
            if by_number.insert(field.number, position).is_some() {
                return Err(FieldInfosError::DuplicateFieldNumber {
                    number: field.number,
                });
            }
        }

        Ok(Self {
            fields,
            by_name,
            by_number,
        })
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FieldInfo> {
        self.fields.iter()
    }

    pub fn field_info(&self, name: &str) -> Option<&FieldInfo> {
        self.by_name
            .get(name)
            .and_then(|position| self.fields.get(*position))
    }

    pub fn field_info_by_number(&self, number: u32) -> Option<&FieldInfo> {
        self.by_number
            .get(&number)
            .and_then(|position| self.fields.get(*position))
    }
}
