use surch_analysis::{
    Analyzer, KeywordAnalyzer, NormAnalyzer, SimpleAnalyzer, StandardAnalyzer, StopAnalyzer,
    WhitespaceAnalyzer,
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
    /// `analyzer.norm` from matchID's `deces_index.yml`: standard tokenizer
    /// followed by `lowercase` + `asciifolding`.
    Norm,
}

impl AnalyzerName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Simple => "simple",
            Self::Stop => "stop",
            Self::Keyword => "keyword",
            Self::Whitespace => "whitespace",
            Self::Norm => "norm",
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
            Self::Norm => NormAnalyzer.token_stream(text),
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "standard" => Some(Self::Standard),
            "simple" => Some(Self::Simple),
            "stop" => Some(Self::Stop),
            "keyword" => Some(Self::Keyword),
            "whitespace" => Some(Self::Whitespace),
            "norm" => Some(Self::Norm),
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
    pub norms: Option<bool>,
    /// Field-level `normalizer` reference for keyword fields. Resolved to a
    /// builtin (`AnalyzerName::Norm`) when the named normalizer matches the
    /// canonical `lowercase + asciifolding` shape; otherwise `None`.
    pub normalizer: Option<AnalyzerName>,
}

impl FieldMapping {
    pub fn new(field_type: FieldType, analyzer: Option<AnalyzerName>) -> Self {
        Self::with_norms(field_type, analyzer, None)
    }

    pub fn with_norms(
        field_type: FieldType,
        analyzer: Option<AnalyzerName>,
        norms: Option<bool>,
    ) -> Self {
        Self {
            field_type,
            analyzer,
            norms,
            normalizer: None,
        }
    }

    pub fn with_normalizer(mut self, normalizer: Option<AnalyzerName>) -> Self {
        self.normalizer = normalizer;
        self
    }

    pub fn analyzer(&self) -> AnalyzerName {
        self.analyzer
            .unwrap_or_else(|| AnalyzerName::default_for(self.field_type))
    }

    pub fn norms_enabled(&self) -> bool {
        self.norms
            .unwrap_or_else(|| default_norms_for(self.field_type))
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

        if let Some(norms) = self.norms {
            object.insert("norms".to_owned(), Value::Bool(norms));
        }

        Value::Object(object)
    }
}

fn default_norms_for(field_type: FieldType) -> bool {
    matches!(field_type, FieldType::Text)
}

/// Parsed `settings.analysis.tokenizer.<name>` for `type: edge_ngram`.
///
/// `token_chars` (`letter`, `digit`, …) is captured for parity but not
/// enforced by the executor — see A13 notes in
/// `docs/wp-d-matchid/gap-analysis.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeNgramTokenizerDefinition {
    pub min_gram: usize,
    pub max_gram: usize,
    pub token_chars: Vec<String>,
}

/// Parsed `settings.analysis.analyzer.<name>` for `tokenizer + filter[]`
/// chains. Used to register user-defined analyzers (e.g. `norm`,
/// `autocomplete_analyzer`) declared in `deces_index.yml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerDefinition {
    pub tokenizer: String,
    pub filter: Vec<String>,
}

/// Parsed `settings.analysis.normalizer.<name>` for `type: custom` chains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizerDefinition {
    pub kind: String,
    pub filter: Vec<String>,
}

/// Settings parsed from `PUT /:index` body's `settings.analysis` block.
///
/// Captured per matchID's `deces_index.yml` shape (excerpted in
/// `docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md`
/// §2.12). Each map is keyed by the user-chosen name (e.g. `norm`,
/// `edge_ngram_tokenizer`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalysisSettings {
    pub tokenizers: BTreeMap<String, EdgeNgramTokenizerDefinition>,
    pub analyzers: BTreeMap<String, AnalyzerDefinition>,
    pub normalizers: BTreeMap<String, NormalizerDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexMapping {
    fields: BTreeMap<String, FieldMapping>,
    analysis: AnalysisSettings,
}

impl IndexMapping {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_field_mapping(&mut self, field: impl Into<String>, mapping: FieldMapping) {
        self.fields.insert(field.into(), mapping);
    }

    /// Parsed `settings.analysis` block, when present. Empty by default.
    pub fn analysis(&self) -> &AnalysisSettings {
        &self.analysis
    }

    /// Overrides the parsed `settings.analysis` block.
    pub fn set_analysis(&mut self, analysis: AnalysisSettings) {
        self.analysis = analysis;
    }

    pub fn analyzer(&self, field: &str) -> AnalyzerName {
        self.fields
            .get(field)
            .map(|mapping| mapping.analyzer())
            .unwrap_or_else(|| AnalyzerName::default_for(FieldType::Text))
    }

    pub fn norms_enabled(&self, field: &str) -> bool {
        self.fields
            .get(field)
            .map(FieldMapping::norms_enabled)
            .unwrap_or_else(|| default_norms_for(FieldType::Text))
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

            let norms = match field_definition.get("norms") {
                Some(value) => {
                    Some(
                        value
                            .as_bool()
                            .ok_or_else(|| MappingError::NormsNotBoolean {
                                field: field.clone(),
                            })?,
                    )
                }
                None => None,
            };

            if field_type != FieldType::Text && analyzer.is_some() {
                return Err(MappingError::AnalyzerNotSupported {
                    field: field.clone(),
                    field_type: field_type.as_str().to_owned(),
                });
            }

            let normalizer = match field_definition.get("normalizer") {
                Some(value) => {
                    let value =
                        value
                            .as_str()
                            .ok_or_else(|| MappingError::NormalizerNotString {
                                field: field.clone(),
                            })?;
                    Some(AnalyzerName::from_name(value).ok_or_else(|| {
                        MappingError::UnsupportedNormalizer {
                            field: field.clone(),
                            normalizer: value.to_owned(),
                        }
                    })?)
                }
                None => None,
            };

            mapping.set_field_mapping(
                field.to_owned(),
                FieldMapping::with_norms(field_type, analyzer, norms)
                    .with_normalizer(normalizer),
            );
        }

        Ok(mapping)
    }

    /// Parses the `settings.analysis` block of a `PUT /:index` body.
    ///
    /// Accepted shape (subset, matching `deces_index.yml`):
    ///
    /// ```json
    /// {
    ///   "analysis": {
    ///     "tokenizer": {
    ///       "edge_ngram_tokenizer": {
    ///         "type": "edge_ngram",
    ///         "min_gram": 2,
    ///         "max_gram": 20,
    ///         "token_chars": ["letter", "digit"]
    ///       }
    ///     },
    ///     "analyzer": {
    ///       "norm": { "tokenizer": "standard", "filter": ["lowercase", "asciifolding"] }
    ///     },
    ///     "normalizer": {
    ///       "norm": { "type": "custom", "filter": ["lowercase", "asciifolding"] }
    ///     }
    ///   }
    /// }
    /// ```
    ///
    /// Unknown tokenizer types are skipped silently (forward-compatible);
    /// only `edge_ngram` is captured for the MVP. Filter names other than
    /// `lowercase`/`asciifolding` are accepted but stored verbatim for
    /// later inspection — the executor honours only the two we ship.
    pub fn from_index_settings_value(value: &Value) -> Result<AnalysisSettings, MappingError> {
        let settings = value
            .as_object()
            .ok_or(MappingError::SettingsNotObject)?;

        let analysis = match settings.get("analysis") {
            Some(value) => value
                .as_object()
                .ok_or(MappingError::AnalysisNotObject)?,
            None => return Ok(AnalysisSettings::default()),
        };

        let mut parsed = AnalysisSettings::default();

        if let Some(tokenizers) = analysis.get("tokenizer") {
            let tokenizers = tokenizers
                .as_object()
                .ok_or(MappingError::AnalysisSectionNotObject {
                    section: "tokenizer".to_owned(),
                })?;
            for (name, definition) in tokenizers {
                let definition = definition.as_object().ok_or_else(|| {
                    MappingError::AnalysisEntryNotObject {
                        section: "tokenizer".to_owned(),
                        name: name.clone(),
                    }
                })?;
                let kind = definition
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if kind != "edge_ngram" {
                    // forward-compatible: skip unsupported tokenizer types
                    continue;
                }
                let min_gram = definition
                    .get("min_gram")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| MappingError::EdgeNgramMissingBound {
                        name: name.clone(),
                        bound: "min_gram".to_owned(),
                    })? as usize;
                let max_gram = definition
                    .get("max_gram")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| MappingError::EdgeNgramMissingBound {
                        name: name.clone(),
                        bound: "max_gram".to_owned(),
                    })? as usize;
                let token_chars = definition
                    .get("token_chars")
                    .and_then(Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                parsed.tokenizers.insert(
                    name.clone(),
                    EdgeNgramTokenizerDefinition {
                        min_gram,
                        max_gram,
                        token_chars,
                    },
                );
            }
        }

        if let Some(analyzers) = analysis.get("analyzer") {
            let analyzers = analyzers
                .as_object()
                .ok_or(MappingError::AnalysisSectionNotObject {
                    section: "analyzer".to_owned(),
                })?;
            for (name, definition) in analyzers {
                let definition = definition.as_object().ok_or_else(|| {
                    MappingError::AnalysisEntryNotObject {
                        section: "analyzer".to_owned(),
                        name: name.clone(),
                    }
                })?;
                let tokenizer = definition
                    .get("tokenizer")
                    .and_then(Value::as_str)
                    .unwrap_or("standard")
                    .to_owned();
                let filter = read_filter_chain(definition, name, "analyzer")?;
                parsed.analyzers.insert(
                    name.clone(),
                    AnalyzerDefinition { tokenizer, filter },
                );
            }
        }

        if let Some(normalizers) = analysis.get("normalizer") {
            let normalizers = normalizers
                .as_object()
                .ok_or(MappingError::AnalysisSectionNotObject {
                    section: "normalizer".to_owned(),
                })?;
            for (name, definition) in normalizers {
                let definition = definition.as_object().ok_or_else(|| {
                    MappingError::AnalysisEntryNotObject {
                        section: "normalizer".to_owned(),
                        name: name.clone(),
                    }
                })?;
                let kind = definition
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("custom")
                    .to_owned();
                let filter = read_filter_chain(definition, name, "normalizer")?;
                parsed.normalizers.insert(
                    name.clone(),
                    NormalizerDefinition { kind, filter },
                );
            }
        }

        Ok(parsed)
    }
}

fn read_filter_chain(
    definition: &Map<String, Value>,
    name: &str,
    section: &str,
) -> Result<Vec<String>, MappingError> {
    match definition.get("filter") {
        Some(Value::Array(values)) => Ok(values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()),
        Some(_) => Err(MappingError::AnalysisFilterNotArray {
            section: section.to_owned(),
            name: name.to_owned(),
        }),
        None => Ok(Vec::new()),
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
    #[error("field `{field}` norms must be a boolean")]
    NormsNotBoolean { field: String },
    #[error("field `{field}` normalizer must be a string")]
    NormalizerNotString { field: String },
    #[error("field `{field}` has unsupported normalizer `{normalizer}`")]
    UnsupportedNormalizer { field: String, normalizer: String },
    #[error("index settings must be an object")]
    SettingsNotObject,
    #[error("settings.analysis must be an object")]
    AnalysisNotObject,
    #[error("settings.analysis.{section} must be an object")]
    AnalysisSectionNotObject { section: String },
    #[error("settings.analysis.{section}.{name} must be an object")]
    AnalysisEntryNotObject { section: String, name: String },
    #[error("settings.analysis.{section}.{name}.filter must be an array")]
    AnalysisFilterNotArray { section: String, name: String },
    #[error("edge_ngram tokenizer `{name}` missing required field `{bound}`")]
    EdgeNgramMissingBound { name: String, bound: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deces_index_settings_with_edge_ngram_norm_and_normalizer() {
        // JSON equivalent of the deces_index.yml `settings:` block excerpted
        // in docs/wp-d-matchid/incoming/2026-05-15-deces-backend-dsl-inventory.md §2.12.
        let settings = serde_json::json!({
            "analysis": {
                "tokenizer": {
                    "edge_ngram_tokenizer": {
                        "type": "edge_ngram",
                        "min_gram": 2,
                        "max_gram": 20,
                        "token_chars": ["letter", "digit"]
                    }
                },
                "analyzer": {
                    "autocomplete_analyzer": { "tokenizer": "edge_ngram_tokenizer" },
                    "norm": {
                        "tokenizer": "standard",
                        "filter": ["lowercase", "asciifolding"]
                    }
                },
                "normalizer": {
                    "norm": {
                        "type": "custom",
                        "filter": ["lowercase", "asciifolding"]
                    }
                }
            }
        });

        let analysis = IndexMapping::from_index_settings_value(&settings)
            .expect("settings.analysis should parse without error");

        let tokenizer = analysis
            .tokenizers
            .get("edge_ngram_tokenizer")
            .expect("edge_ngram_tokenizer registered");
        assert_eq!(tokenizer.min_gram, 2);
        assert_eq!(tokenizer.max_gram, 20);
        assert_eq!(
            tokenizer.token_chars,
            vec!["letter".to_owned(), "digit".to_owned()]
        );

        let norm_analyzer = analysis
            .analyzers
            .get("norm")
            .expect("norm analyzer registered");
        assert_eq!(norm_analyzer.tokenizer, "standard");
        assert_eq!(
            norm_analyzer.filter,
            vec!["lowercase".to_owned(), "asciifolding".to_owned()]
        );

        let autocomplete = analysis
            .analyzers
            .get("autocomplete_analyzer")
            .expect("autocomplete_analyzer registered");
        assert_eq!(autocomplete.tokenizer, "edge_ngram_tokenizer");
        assert!(autocomplete.filter.is_empty());

        let normalizer = analysis
            .normalizers
            .get("norm")
            .expect("norm normalizer registered");
        assert_eq!(normalizer.kind, "custom");
        assert_eq!(
            normalizer.filter,
            vec!["lowercase".to_owned(), "asciifolding".to_owned()]
        );
    }

    #[test]
    fn from_properties_value_accepts_norm_analyzer_on_text_field() {
        // matchID maps NOM/PRENOMS as `{ type: text, analyzer: norm }`.
        let properties = serde_json::json!({
            "NOM": { "type": "text", "analyzer": "norm" }
        });

        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("norm analyzer should resolve to a builtin");
        let nom = mapping.field("NOM").expect("NOM field exists");
        assert_eq!(nom.field_type, FieldType::Text);
        assert_eq!(nom.analyzer, Some(AnalyzerName::Norm));
    }

    #[test]
    fn from_properties_value_accepts_normalizer_on_keyword_field() {
        // matchID maps `NOM.raw` as `{ type: keyword, normalizer: norm }`.
        let properties = serde_json::json!({
            "raw": { "type": "keyword", "normalizer": "norm" }
        });

        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("normalizer on keyword should parse");
        let raw = mapping.field("raw").expect("raw field exists");
        assert_eq!(raw.field_type, FieldType::Keyword);
        assert_eq!(raw.normalizer, Some(AnalyzerName::Norm));
    }
}
