use serde::{Deserialize, Serialize};
use crate::common::FieldType;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    Null,
    Bool(bool),
    Integer(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Text(String),
    Keyword(String),
    Date(String),
    Array(Vec<FieldValue>),
}

impl FieldValue {
    pub fn field_type(&self) -> FieldType {
        match self {
            FieldValue::Null => FieldType::Unknown,
            FieldValue::Bool(_) => FieldType::Boolean,
            FieldValue::Integer(_) => FieldType::Integer,
            FieldValue::Long(_) => FieldType::Long,
            FieldValue::Float(_) => FieldType::Float,
            FieldValue::Double(_) => FieldType::Double,
            FieldValue::Text(_) => FieldType::Text,
            FieldValue::Keyword(_) => FieldType::Keyword,
            FieldValue::Date(_) => FieldType::Date,
            FieldValue::Array(arr) => {
                arr.first().map(|v| v.field_type()).unwrap_or(FieldType::Unknown)
            }
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            FieldValue::Text(s) => Some(s),
            FieldValue::Keyword(s) => Some(s),
            FieldValue::Date(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_keyword(&self) -> Option<&str> {
        match self {
            FieldValue::Keyword(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            FieldValue::Integer(i) => Some(*i as i64),
            FieldValue::Long(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            FieldValue::Float(f) => Some(*f as f64),
            FieldValue::Double(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            FieldValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl From<bool> for FieldValue {
    fn from(b: bool) -> Self {
        FieldValue::Bool(b)
    }
}

impl From<i32> for FieldValue {
    fn from(i: i32) -> Self {
        FieldValue::Integer(i)
    }
}

impl From<i64> for FieldValue {
    fn from(i: i64) -> Self {
        FieldValue::Long(i)
    }
}

impl From<f32> for FieldValue {
    fn from(f: f32) -> Self {
        FieldValue::Float(f)
    }
}

impl From<f64> for FieldValue {
    fn from(f: f64) -> Self {
        FieldValue::Double(f)
    }
}

impl From<String> for FieldValue {
    fn from(s: String) -> Self {
        FieldValue::Text(s)
    }
}

impl From<&str> for FieldValue {
    fn from(s: &str) -> Self {
        FieldValue::Text(s.to_string())
    }
}

impl<T: Into<FieldValue>> From<Vec<T>> for FieldValue {
    fn from(arr: Vec<T>) -> Self {
        FieldValue::Array(arr.into_iter().map(|v| v.into()).collect())
    }
}
