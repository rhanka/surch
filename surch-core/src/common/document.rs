use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::common::FieldValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub fields: HashMap<String, FieldValue>,
    pub version: Option<u64>,
    pub seq_no: Option<u64>,
    pub primary_term: Option<u64>,
}

impl Document {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            fields: HashMap::new(),
            version: None,
            seq_no: None,
            primary_term: None,
        }
    }

    pub fn with_field(mut self, name: impl Into<String>, value: impl Into<FieldValue>) -> Self {
        self.fields.insert(name.into(), value.into());
        self
    }

    pub fn with_fields(mut self, fields: HashMap<String, FieldValue>) -> Self {
        self.fields = fields;
        self
    }

    pub fn get_field(&self, name: &str) -> Option<&FieldValue> {
        self.fields.get(name)
    }

    pub fn get_text(&self, name: &str) -> Option<String> {
        self.get_field(name).and_then(|v| v.as_text().map(String::from))
    }
}

impl From<HashMap<String, FieldValue>> for Document {
    fn from(fields: HashMap<String, FieldValue>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            fields,
            version: None,
            seq_no: None,
            primary_term: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRequest {
    pub index: String,
    pub id: Option<String>,
    #[serde(default)]
    pub pipeline: Option<String>,
    pub document: HashMap<String, FieldValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexResponse {
    pub _index: String,
    pub _id: String,
    #[serde(rename = "_version")]
    pub version: u64,
    pub _seq_no: u64,
    pub _primary_term: u64,
    pub result: String,
    #[serde(rename = "_shards")]
    pub shards: ShardsInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardsInfo {
    pub total: u32,
    pub successful: u32,
    pub failed: u32,
}

impl Default for ShardsInfo {
    fn default() -> Self {
        Self {
            total: 1,
            successful: 1,
            failed: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkRequest {
    #[serde(default)]
    pub actions: Vec<BulkAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum BulkAction {
    #[serde(rename = "index")]
    Index {
        #[serde(rename = "_index")]
        index: String,
        #[serde(rename = "_id")]
        id: Option<String>,
    },
    #[serde(rename = "delete")]
    Delete {
        #[serde(rename = "_index")]
        index: String,
        #[serde(rename = "_id")]
        id: String,
    },
    Document(HashMap<String, FieldValue>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkResponse {
    pub took: u64,
    pub errors: bool,
    pub items: Vec<BulkItemResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkItemResponse {
    pub index: Option<BulkItemResult>,
    pub delete: Option<BulkItemResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkItemResult {
    #[serde(rename = "_index")]
    pub index: String,
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "_version")]
    pub version: u64,
    pub result: String,
    pub status: u16,
}
