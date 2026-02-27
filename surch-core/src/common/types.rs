use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Keyword,
    Integer,
    Long,
    Float,
    Double,
    Boolean,
    Date,
    #[serde(other)]
    Unknown,
}

impl Default for FieldType {
    fn default() -> Self {
        FieldType::Text
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDefinition {
    pub name: String,
    pub field_type: FieldType,
    pub analyzer: Option<String>,
    pub index: Option<bool>,
    pub doc_values: Option<bool>,
    pub store: Option<bool>,
}

impl FieldDefinition {
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            name: name.into(),
            field_type,
            analyzer: None,
            index: None,
            doc_values: None,
            store: None,
        }
    }

    pub fn with_analyzer(mut self, analyzer: impl Into<String>) -> Self {
        self.analyzer = Some(analyzer.into());
        self
    }

    pub fn with_index(mut self, index: bool) -> Self {
        self.index = Some(index);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Mapping {
    pub properties: HashMap<String, FieldDefinition>,
}

impl Mapping {
    pub fn new() -> Self {
        Self {
            properties: HashMap::new(),
        }
    }

    pub fn add_field(&mut self, field: FieldDefinition) {
        self.properties.insert(field.name.clone(), field);
    }

    pub fn get_field(&self, name: &str) -> Option<&FieldDefinition> {
        self.properties.get(name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexSettings {
    pub number_of_shards: Option<usize>,
    pub number_of_replicas: Option<usize>,
    pub refresh_interval: Option<String>,
}

impl IndexSettings {
    pub fn default_settings() -> Self {
        Self {
            number_of_shards: Some(1),
            number_of_replicas: Some(0),
            refresh_interval: Some("1s".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub name: String,
    pub uuid: String,
    pub mapping: Mapping,
    pub settings: IndexSettings,
    pub created_at: String,
    pub version: u64,
}

impl IndexMetadata {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            uuid: uuid::Uuid::new_v4().to_string(),
            mapping: Mapping::new(),
            settings: IndexSettings::default_settings(),
            created_at: chrono::Utc::now().to_rfc3339(),
            version: 1,
        }
    }
}
