use surch_analysis::{
    Analyzer, KeywordAnalyzer, SimpleAnalyzer, StandardAnalyzer, StopAnalyzer, WhitespaceAnalyzer,
};

use std::collections::BTreeMap;
use std::iter::FromIterator;

use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    Keyword,
    Integer,
    Long,
    Float,
    Double,
    Boolean,
    Date,
    Object,
    Array,
    Unknown,
}

impl FieldType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Keyword => "keyword",
            Self::Integer => "integer",
            Self::Long => "long",
            Self::Float => "float",
            Self::Double => "double",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::Object => "object",
            Self::Array => "array",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "keyword" => Some(Self::Keyword),
            "integer" | "int" => Some(Self::Integer),
            "long" => Some(Self::Long),
            "float" => Some(Self::Float),
            "double" => Some(Self::Double),
            "boolean" | "bool" => Some(Self::Boolean),
            "date" => Some(Self::Date),
            "object" => Some(Self::Object),
            "array" => Some(Self::Array),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyzerName {
    Standard,
    Simple,
    Stop,
    Keyword,
    Whitespace,
}

impl AnalyzerName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Simple => "simple",
            Self::Stop => "stop",
            Self::Keyword => "keyword",
            Self::Whitespace => "whitespace",
        }
    }

    pub fn first_term(&self, text: &str) -> String {
        self.token_stream(text)
            .first()
            .map(|token| token.term.clone())
            .unwrap_or_default()
    }

    pub fn terms(&self, text: &str) -> Vec<String> {
        self.token_stream(text)
            .into_iter()
            .map(|token| token.term)
            .collect()
    }

    fn token_stream(&self, text: &str) -> Vec<surch_analysis::Token> {
        match self {
            Self::Standard => StandardAnalyzer.token_stream(text),
            Self::Simple => SimpleAnalyzer.token_stream(text),
            Self::Stop => StopAnalyzer.token_stream(text),
            Self::Keyword => KeywordAnalyzer.token_stream(text),
            Self::Whitespace => WhitespaceAnalyzer.token_stream(text),
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "standard" => Some(Self::Standard),
            "simple" => Some(Self::Simple),
            "stop" => Some(Self::Stop),
            "keyword" => Some(Self::Keyword),
            "whitespace" => Some(Self::Whitespace),
            _ => None,
        }
    }

    pub fn default_for(field_type: FieldType) -> Self {
        match field_type {
            FieldType::Text => Self::Simple,
            _ => Self::Keyword,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldMapping {
    pub field_type: FieldType,
    pub analyzer: Option<AnalyzerName>,
}

impl FieldMapping {
    pub fn new(field_type: FieldType, analyzer: Option<AnalyzerName>) -> Self {
        Self {
            field_type,
            analyzer,
        }
    }

    pub fn analyzer(&self) -> AnalyzerName {
        self.analyzer
            .unwrap_or_else(|| AnalyzerName::default_for(self.field_type))
    }

    fn as_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "type".to_owned(),
            Value::String(self.field_type.as_str().to_owned()),
        );

        if let Some(analyzer) = self.analyzer {
            object.insert(
                "analyzer".to_owned(),
                Value::String(analyzer.as_str().to_owned()),
            );
        }

        Value::Object(object)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexMapping {
    fields: BTreeMap<String, FieldMapping>,
}

impl IndexMapping {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_field_mapping(&mut self, field: impl Into<String>, mapping: FieldMapping) {
        self.fields.insert(field.into(), mapping);
    }

    pub fn analyzer(&self, field: &str) -> AnalyzerName {
        self.fields
            .get(field)
            .map(|mapping| mapping.analyzer())
            .unwrap_or_else(|| AnalyzerName::default_for(FieldType::Text))
    }

    pub fn has_field(&self, field: &str) -> bool {
        self.fields.contains_key(field)
    }

    pub fn field(&self, field: &str) -> Option<&FieldMapping> {
        self.fields.get(field)
    }

    pub fn as_value(&self) -> Value {
        let properties = self
            .fields
            .iter()
            .map(|(name, mapping)| (name.clone(), mapping.as_value()))
            .collect::<Map<_, _>>();

        Value::Object(Map::from_iter([(
            "properties".to_owned(),
            Value::Object(properties),
        )]))
    }

    pub fn fields(&self) -> impl Iterator<Item = (&str, &FieldMapping)> {
        self.fields
            .iter()
            .map(|(name, mapping)| (name.as_str(), mapping))
    }

    pub fn infer_from_document(document: &Value) -> Self {
        let object = match document.as_object() {
            Some(object) => object,
            None => return Self::new(),
        };

        let mut mapping = Self::new();
        for (name, value) in object {
            let field_type = infer_field_type(value);
            mapping.set_field_mapping(name.as_str(), FieldMapping::new(field_type, None));
        }

        mapping
    }

    pub fn ensure_fields(&mut self, document: &Value) {
        let Some(object) = document.as_object() else {
            return;
        };

        for (field, value) in object {
            if self.has_field(field) {
                continue;
            }
            let field_type = infer_field_type(value);
            self.set_field_mapping(field.as_str(), FieldMapping::new(field_type, None));
        }
    }

    pub fn from_properties_value(value: &Value) -> Result<Self, MappingError> {
        let properties = value.as_object().ok_or(MappingError::PropertiesNotObject)?;

        let mut mapping = Self::new();
        for (field, field_definition) in properties {
            if field.trim().is_empty() {
                return Err(MappingError::EmptyFieldName);
            }

            let field_definition = field_definition.as_object().ok_or_else(|| {
                MappingError::FieldDefinitionNotObject {
                    field: field.clone(),
                }
            })?;

            let field_type_name = field_definition
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("text");
            let field_type = FieldType::from_name(field_type_name).ok_or_else(|| {
                MappingError::UnsupportedFieldType {
                    field: field.clone(),
                    field_type: field_type_name.to_owned(),
                }
            })?;

            let analyzer = match field_definition.get("analyzer") {
                Some(value) => {
                    let value = value
                        .as_str()
                        .ok_or_else(|| MappingError::AnalyzerNotString {
                            field: field.clone(),
                        })?;
                    Some(AnalyzerName::from_name(value).ok_or_else(|| {
                        MappingError::UnsupportedAnalyzer {
                            field: field.clone(),
                            analyzer: value.to_owned(),
                        }
                    })?)
                }
                None => None,
            };

            if field_type != FieldType::Text && analyzer.is_some() {
                return Err(MappingError::AnalyzerNotSupported {
                    field: field.clone(),
                    field_type: field_type.as_str().to_owned(),
                });
            }

            mapping.set_field_mapping(field.to_owned(), FieldMapping::new(field_type, analyzer));
        }

        Ok(mapping)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MappingError {
    #[error("mapping properties must be an object")]
    PropertiesNotObject,
    #[error("mapping field name cannot be empty")]
    EmptyFieldName,
    #[error("field `{field}` definition must be an object")]
    FieldDefinitionNotObject { field: String },
    #[error("field `{field}` has unsupported type `{field_type}`")]
    UnsupportedFieldType { field: String, field_type: String },
    #[error("field `{field}` analyzer must be a string")]
    AnalyzerNotString { field: String },
    #[error("field `{field}` has unsupported analyzer `{analyzer}`")]
    UnsupportedAnalyzer { field: String, analyzer: String },
    #[error("field `{field}` analyzer is only supported on text fields")]
    AnalyzerNotSupported { field: String, field_type: String },
}

pub fn infer_field_type(value: &Value) -> FieldType {
    match value {
        Value::String(text) if is_numeric_string(text) => FieldType::Keyword,
        Value::String(_) => FieldType::Text,
        Value::Number(number) => {
            if number.is_f64() {
                FieldType::Double
            } else if number.is_u64() || number.is_i64() {
                FieldType::Integer
            } else {
                FieldType::Unknown
            }
        }
        Value::Bool(_) => FieldType::Boolean,
        Value::Array(values) => values
            .iter()
            .find_map(|value| {
                let inferred = infer_field_type(value);
                (inferred != FieldType::Unknown).then_some(inferred)
            })
            .unwrap_or(FieldType::Array),
        Value::Object(_) => FieldType::Object,
        Value::Null => FieldType::Unknown,
    }
}

fn is_numeric_string(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|character| character.is_ascii_digit())
}
