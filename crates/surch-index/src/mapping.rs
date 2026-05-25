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
    /// matchID `GEOPOINT_NAISSANCE` / `GEOPOINT_DECES` (intake §2.12).
    /// Stored shape is two `f64` floats (`lat`, `lon`) parsed at runtime
    /// from one of three accepted source representations:
    /// `{ "lat": …, "lon": … }`, the string `"lat,lon"`, or the GeoJSON
    /// array `[lon, lat]`. See `parse_geo_point_source` in
    /// `crates/surch-api/src/search.rs`.
    GeoPoint,
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
            Self::GeoPoint => "geo_point",
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
            "geo_point" => Some(Self::GeoPoint),
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

/// Parsed `index_prefixes` field option (matchID §2.12).
///
/// matchID enables `index_prefixes: { min_chars: 2, max_chars: 10 }` on
/// `PRENOMS` to back fast autocomplete prefix queries. ES defaults are
/// `min_chars=2`, `max_chars=5`; both are clamped to `2..=20` and the
/// strict ordering `min < max` is enforced (see ES `IndexPrefixes`
/// validation in `MapperBuilder`).
///
/// Phase-1: this is parse-only metadata — the runtime still answers
/// `prefix` queries via the existing scan/token-iterator in
/// `surch-api/src/search.rs::prefix_field_matches`. A6's runtime
/// acceleration (write-time edge-ngram fan-out) is tracked separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldPrefixes {
    pub min_chars: usize,
    pub max_chars: usize,
}

impl FieldPrefixes {
    pub const DEFAULT_MIN_CHARS: usize = 2;
    pub const DEFAULT_MAX_CHARS: usize = 5;
    pub const MAX_BOUND: usize = 20;

    pub const fn defaults() -> Self {
        Self {
            min_chars: Self::DEFAULT_MIN_CHARS,
            max_chars: Self::DEFAULT_MAX_CHARS,
        }
    }
}

/// Default `format` applied to `type: date` fields when the mapping omits it.
///
/// Matches matchID's `deces_index.yml` (intake §2.12) where `DATE_NAISSANCE`
/// is declared as `{ type: date, format: yyyyMMdd }` over the `YYYYMMDD`
/// keyword strings emitted by the INSEE extracts. The keyword-backed storage
/// is already lex-sortable, so the parsed `format` is captured for wire-compat
/// round-tripping only — no runtime `chrono::NaiveDate` parsing is wired yet.
pub const DEFAULT_DATE_FORMAT: &str = "yyyyMMdd";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldMapping {
    pub field_type: FieldType,
    pub analyzer: Option<AnalyzerName>,
    pub norms: Option<bool>,
    /// Field-level `normalizer` reference for keyword fields. Resolved to a
    /// builtin (`AnalyzerName::Norm`) when the named normalizer matches the
    /// canonical `lowercase + asciifolding` shape; otherwise `None`.
    pub normalizer: Option<AnalyzerName>,
    /// Field-level `index_prefixes` option (text fields only).
    pub index_prefixes: Option<FieldPrefixes>,
    /// `format` token captured when `field_type == Date`. Defaults to
    /// `DEFAULT_DATE_FORMAT` (`yyyyMMdd`) when the mapping omits it. The
    /// MVP stores the value verbatim — any string is accepted but only the
    /// lex-sortable `yyyyMMdd` shape is exercised at search time.
    pub date_format: Option<String>,
    /// A10 phase 2: parsed multi-field sub-fields declared under the
    /// ES `fields: { <sub_name>: { type: …, normalizer: …, … } }` block.
    ///
    /// matchID's `deces_index.yml` (intake §2.12) maps
    /// `NOM: { type: text, analyzer: norm, fields: { raw: { type: keyword,
    /// normalizer: norm } } }`. Wire-shape consumers (`GET
    /// /:index/_mapping`) need every sub-field round-tripped verbatim.
    ///
    /// Write-time fan-out (indexing each parent value under the
    /// `parent.subname` path with the sub-field's analyzer/normalizer) is
    /// **not** wired yet — see gap-analysis A10 "phase 3" note. At
    /// query time `lookup_sort_value` still aliases `parent.subname` to
    /// the parent's stored value; when the sub-field carries a
    /// `normalizer` the alias is normalised on read so `sort: NOM.raw`
    /// returns the lowercased / asciifolded value matchID expects.
    pub fields: BTreeMap<String, FieldMapping>,
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
            index_prefixes: None,
            date_format: None,
            fields: BTreeMap::new(),
        }
    }

    pub fn with_normalizer(mut self, normalizer: Option<AnalyzerName>) -> Self {
        self.normalizer = normalizer;
        self
    }

    pub fn with_index_prefixes(mut self, prefixes: Option<FieldPrefixes>) -> Self {
        self.index_prefixes = prefixes;
        self
    }

    pub fn with_date_format(mut self, date_format: Option<String>) -> Self {
        self.date_format = date_format;
        self
    }

    /// A10 phase 2: overrides the parsed multi-field sub-fields block
    /// (`fields: { <name>: { … } }`).
    pub fn with_subfields(mut self, fields: BTreeMap<String, FieldMapping>) -> Self {
        self.fields = fields;
        self
    }

    /// Effective `format` for a `date` field. Returns `DEFAULT_DATE_FORMAT`
    /// when the mapping omits the attribute. Non-date fields return `None`.
    pub fn date_format(&self) -> Option<&str> {
        if self.field_type != FieldType::Date {
            return None;
        }
        Some(self.date_format.as_deref().unwrap_or(DEFAULT_DATE_FORMAT))
    }

    pub fn analyzer(&self) -> AnalyzerName {
        self.analyzer
            .unwrap_or_else(|| AnalyzerName::default_for(self.field_type))
    }

    /// A10: analyzer applied at write time when fanning a parent value
    /// into this sub-field.
    ///
    /// For a `keyword` sub-field declared with a `normalizer` (matchID's
    /// `NOM.raw: { type: keyword, normalizer: norm }`), the stored token
    /// must be the whole value lowercased / asciifolded — i.e. produced
    /// by the normalizer, not the default keyword analyzer. For a
    /// `text` sub-field the declared (or default) analyzer applies. When
    /// no `normalizer`/`analyzer` is set this falls back to
    /// [`Self::analyzer`], so a plain `keyword` sub-field stores the
    /// untouched value (single keyword token).
    pub fn effective_analyzer(&self) -> AnalyzerName {
        if let Some(normalizer) = self.normalizer {
            return normalizer;
        }
        self.analyzer()
    }

    pub fn norms_enabled(&self) -> bool {
        self.norms
            .unwrap_or_else(|| default_norms_for(self.field_type))
    }

    /// A10: declared multi-field sub-fields, in lexicographic order.
    /// Empty for single fields. Each item is `(sub_name, &sub_mapping)`;
    /// callers build the qualified `parent.sub_name` write path
    /// themselves.
    pub fn subfields(&self) -> impl Iterator<Item = (&str, &FieldMapping)> {
        self.fields
            .iter()
            .map(|(name, mapping)| (name.as_str(), mapping))
    }

    /// A10: whether this field declares at least one sub-field, i.e.
    /// whether write-time fan-out applies to it.
    pub fn has_subfields(&self) -> bool {
        !self.fields.is_empty()
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

        if let Some(normalizer) = self.normalizer {
            object.insert(
                "normalizer".to_owned(),
                Value::String(normalizer.as_str().to_owned()),
            );
        }

        if let Some(prefixes) = self.index_prefixes {
            let mut inner = Map::new();
            inner.insert(
                "min_chars".to_owned(),
                Value::Number(prefixes.min_chars.into()),
            );
            inner.insert(
                "max_chars".to_owned(),
                Value::Number(prefixes.max_chars.into()),
            );
            object.insert("index_prefixes".to_owned(), Value::Object(inner));
        }

        if self.field_type == FieldType::Date {
            // Always emit the effective format so wire-shape consumers
            // (matchID, kibana-compat clients) see a fully-resolved value.
            let format = self
                .date_format
                .clone()
                .unwrap_or_else(|| DEFAULT_DATE_FORMAT.to_owned());
            object.insert("format".to_owned(), Value::String(format));
        }

        if !self.fields.is_empty() {
            let inner = self
                .fields
                .iter()
                .map(|(name, mapping)| (name.clone(), mapping.as_value()))
                .collect::<Map<_, _>>();
            object.insert("fields".to_owned(), Value::Object(inner));
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

    /// A10 phase 2: resolves a (possibly dotted) field path against the
    /// declared multi-field mapping.
    ///
    /// For a top-level path (`"NOM"`) this returns the same value as
    /// [`Self::field`]. For a multi-field path (`"NOM.raw"`) it walks the
    /// parent's `fields` block and returns the sub-field mapping. Returns
    /// `None` when neither the parent nor the sub-field is declared.
    pub fn resolve_field(&self, path: &str) -> Option<&FieldMapping> {
        if let Some(field) = self.fields.get(path) {
            return Some(field);
        }
        let (parent, sub) = path.rsplit_once('.')?;
        self.fields.get(parent)?.fields.get(sub)
    }

    /// A10 phase 2: when `path` is a multi-field sub-field (`parent.sub`)
    /// declared with a `normalizer`, returns that normalizer so callers
    /// (e.g. sort) can apply it to the parent's stored value at read
    /// time, until write-time fan-out lands.
    pub fn subfield_normalizer(&self, path: &str) -> Option<AnalyzerName> {
        let (parent, sub) = path.rsplit_once('.')?;
        self.fields.get(parent)?.fields.get(sub)?.normalizer
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
            let parsed = parse_field_definition(field, field_definition, true)?;
            mapping.set_field_mapping(field.to_owned(), parsed);
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
        let settings = value.as_object().ok_or(MappingError::SettingsNotObject)?;

        let analysis = match settings.get("analysis") {
            Some(value) => value.as_object().ok_or(MappingError::AnalysisNotObject)?,
            None => return Ok(AnalysisSettings::default()),
        };

        let mut parsed = AnalysisSettings::default();

        if let Some(tokenizers) = analysis.get("tokenizer") {
            let tokenizers =
                tokenizers
                    .as_object()
                    .ok_or(MappingError::AnalysisSectionNotObject {
                        section: "tokenizer".to_owned(),
                    })?;
            for (name, definition) in tokenizers {
                let definition =
                    definition
                        .as_object()
                        .ok_or_else(|| MappingError::AnalysisEntryNotObject {
                            section: "tokenizer".to_owned(),
                            name: name.clone(),
                        })?;
                let kind = definition.get("type").and_then(Value::as_str).unwrap_or("");
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
            let analyzers =
                analyzers
                    .as_object()
                    .ok_or(MappingError::AnalysisSectionNotObject {
                        section: "analyzer".to_owned(),
                    })?;
            for (name, definition) in analyzers {
                let definition =
                    definition
                        .as_object()
                        .ok_or_else(|| MappingError::AnalysisEntryNotObject {
                            section: "analyzer".to_owned(),
                            name: name.clone(),
                        })?;
                let tokenizer = definition
                    .get("tokenizer")
                    .and_then(Value::as_str)
                    .unwrap_or("standard")
                    .to_owned();
                let filter = read_filter_chain(definition, name, "analyzer")?;
                parsed
                    .analyzers
                    .insert(name.clone(), AnalyzerDefinition { tokenizer, filter });
            }
        }

        if let Some(normalizers) = analysis.get("normalizer") {
            let normalizers =
                normalizers
                    .as_object()
                    .ok_or(MappingError::AnalysisSectionNotObject {
                        section: "normalizer".to_owned(),
                    })?;
            for (name, definition) in normalizers {
                let definition =
                    definition
                        .as_object()
                        .ok_or_else(|| MappingError::AnalysisEntryNotObject {
                            section: "normalizer".to_owned(),
                            name: name.clone(),
                        })?;
                let kind = definition
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("custom")
                    .to_owned();
                let filter = read_filter_chain(definition, name, "normalizer")?;
                parsed
                    .normalizers
                    .insert(name.clone(), NormalizerDefinition { kind, filter });
            }
        }

        Ok(parsed)
    }
}

/// Parses a single field definition. Used both by the top-level
/// `properties: { <name>: { … } }` loop and recursively by the
/// `fields: { <sub_name>: { … } }` multi-field sub-block (A10 phase 2).
///
/// `allow_subfields` is `true` at the top level and `false` for entries
/// nested under `fields:` — ES rejects multi-field nesting more than one
/// level deep.
fn parse_field_definition(
    field: &str,
    definition: &Value,
    allow_subfields: bool,
) -> Result<FieldMapping, MappingError> {
    let object = definition
        .as_object()
        .ok_or_else(|| MappingError::FieldDefinitionNotObject {
            field: field.to_owned(),
        })?;

    let field_type_name = object.get("type").and_then(Value::as_str).unwrap_or("text");
    let field_type = FieldType::from_name(field_type_name).ok_or_else(|| {
        MappingError::UnsupportedFieldType {
            field: field.to_owned(),
            field_type: field_type_name.to_owned(),
        }
    })?;

    let analyzer = match object.get("analyzer") {
        Some(value) => {
            let value = value
                .as_str()
                .ok_or_else(|| MappingError::AnalyzerNotString {
                    field: field.to_owned(),
                })?;
            Some(AnalyzerName::from_name(value).ok_or_else(|| {
                MappingError::UnsupportedAnalyzer {
                    field: field.to_owned(),
                    analyzer: value.to_owned(),
                }
            })?)
        }
        None => None,
    };

    let norms = match object.get("norms") {
        Some(value) => Some(
            value
                .as_bool()
                .ok_or_else(|| MappingError::NormsNotBoolean {
                    field: field.to_owned(),
                })?,
        ),
        None => None,
    };

    if field_type != FieldType::Text && analyzer.is_some() {
        return Err(MappingError::AnalyzerNotSupported {
            field: field.to_owned(),
            field_type: field_type.as_str().to_owned(),
        });
    }

    let normalizer = match object.get("normalizer") {
        Some(value) => {
            let value = value
                .as_str()
                .ok_or_else(|| MappingError::NormalizerNotString {
                    field: field.to_owned(),
                })?;
            Some(AnalyzerName::from_name(value).ok_or_else(|| {
                MappingError::UnsupportedNormalizer {
                    field: field.to_owned(),
                    normalizer: value.to_owned(),
                }
            })?)
        }
        None => None,
    };

    let index_prefixes = match object.get("index_prefixes") {
        Some(value) => Some(parse_index_prefixes(field, value)?),
        None => None,
    };

    if index_prefixes.is_some() && field_type != FieldType::Text {
        return Err(MappingError::IndexPrefixesNotSupported {
            field: field.to_owned(),
            field_type: field_type.as_str().to_owned(),
        });
    }

    // `format` is only meaningful on `type: date`. Captured verbatim
    // — MVP keyword-backs `YYYYMMDD` strings, so any shape rounds-trips
    // without runtime validation. See gap-analysis A7.
    let date_format = match object.get("format") {
        Some(value) => {
            let value = value
                .as_str()
                .ok_or_else(|| MappingError::DateFormatNotString {
                    field: field.to_owned(),
                })?;
            if field_type != FieldType::Date {
                return Err(MappingError::DateFormatNotSupported {
                    field: field.to_owned(),
                    field_type: field_type.as_str().to_owned(),
                });
            }
            Some(value.to_owned())
        }
        None => None,
    };

    // A10 phase 2: parse `fields: { <sub_name>: { … } }`. ES forbids
    // nesting `fields` more than one level deep — we mirror that.
    let mut subfields: BTreeMap<String, FieldMapping> = BTreeMap::new();
    if let Some(fields_value) = object.get("fields") {
        if !allow_subfields {
            return Err(MappingError::NestedSubfieldsNotSupported {
                field: field.to_owned(),
            });
        }
        let entries = fields_value
            .as_object()
            .ok_or_else(|| MappingError::SubfieldsNotObject {
                field: field.to_owned(),
            })?;
        for (sub_name, sub_definition) in entries {
            if sub_name.trim().is_empty() {
                return Err(MappingError::EmptySubfieldName {
                    field: field.to_owned(),
                });
            }
            let qualified = format!("{field}.{sub_name}");
            let parsed = parse_field_definition(&qualified, sub_definition, false)?;
            subfields.insert(sub_name.clone(), parsed);
        }
    }

    Ok(FieldMapping::with_norms(field_type, analyzer, norms)
        .with_normalizer(normalizer)
        .with_index_prefixes(index_prefixes)
        .with_date_format(date_format)
        .with_subfields(subfields))
}

fn parse_index_prefixes(field: &str, value: &Value) -> Result<FieldPrefixes, MappingError> {
    let object = value
        .as_object()
        .ok_or_else(|| MappingError::IndexPrefixesNotObject {
            field: field.to_owned(),
        })?;

    let unknown_key = object
        .keys()
        .find(|key| key.as_str() != "min_chars" && key.as_str() != "max_chars");
    if let Some(key) = unknown_key {
        return Err(MappingError::IndexPrefixesUnknownKey {
            field: field.to_owned(),
            key: key.to_owned(),
        });
    }

    let min_chars = match object.get("min_chars") {
        Some(value) => {
            value
                .as_u64()
                .ok_or_else(|| MappingError::IndexPrefixesBoundNotUnsigned {
                    field: field.to_owned(),
                    bound: "min_chars".to_owned(),
                })? as usize
        }
        None => FieldPrefixes::DEFAULT_MIN_CHARS,
    };

    let max_chars = match object.get("max_chars") {
        Some(value) => {
            value
                .as_u64()
                .ok_or_else(|| MappingError::IndexPrefixesBoundNotUnsigned {
                    field: field.to_owned(),
                    bound: "max_chars".to_owned(),
                })? as usize
        }
        None => FieldPrefixes::DEFAULT_MAX_CHARS,
    };

    if min_chars < FieldPrefixes::DEFAULT_MIN_CHARS
        || max_chars > FieldPrefixes::MAX_BOUND
        || min_chars >= max_chars
    {
        return Err(MappingError::IndexPrefixesBoundsInvalid {
            field: field.to_owned(),
            min_chars,
            max_chars,
        });
    }

    Ok(FieldPrefixes {
        min_chars,
        max_chars,
    })
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
    #[error("field `{field}` format must be a string")]
    DateFormatNotString { field: String },
    #[error("field `{field}` format is only supported on date fields (got `{field_type}`)")]
    DateFormatNotSupported { field: String, field_type: String },
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
    #[error("field `{field}` index_prefixes must be an object")]
    IndexPrefixesNotObject { field: String },
    #[error("field `{field}` index_prefixes has unknown key `{key}`")]
    IndexPrefixesUnknownKey { field: String, key: String },
    #[error("field `{field}` index_prefixes.{bound} must be a non-negative integer")]
    IndexPrefixesBoundNotUnsigned { field: String, bound: String },
    #[error(
        "field `{field}` index_prefixes bounds invalid (min_chars={min_chars}, max_chars={max_chars}); expected 2 <= min_chars < max_chars <= 20"
    )]
    IndexPrefixesBoundsInvalid {
        field: String,
        min_chars: usize,
        max_chars: usize,
    },
    #[error(
        "field `{field}` index_prefixes is only supported on text fields (got `{field_type}`)"
    )]
    IndexPrefixesNotSupported { field: String, field_type: String },
    #[error("field `{field}` fields must be an object")]
    SubfieldsNotObject { field: String },
    #[error("field `{field}` declares an empty sub-field name")]
    EmptySubfieldName { field: String },
    #[error("field `{field}` nests `fields` more than one level deep, which is not allowed")]
    NestedSubfieldsNotSupported { field: String },
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

    // --- A6: `index_prefixes` field-level option (matchID §2.12) ---

    #[test]
    fn from_properties_value_accepts_empty_index_prefixes_with_defaults() {
        // ES default: `index_prefixes: {}` -> min_chars=2, max_chars=5.
        let properties = serde_json::json!({
            "NOM": { "type": "text", "index_prefixes": {} }
        });

        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("empty index_prefixes should default to min=2/max=5");
        let nom = mapping.field("NOM").expect("NOM field exists");
        assert_eq!(nom.field_type, FieldType::Text);
        let prefixes = nom.index_prefixes.expect("index_prefixes recorded on NOM");
        assert_eq!(prefixes.min_chars, FieldPrefixes::DEFAULT_MIN_CHARS);
        assert_eq!(prefixes.max_chars, FieldPrefixes::DEFAULT_MAX_CHARS);
        assert_eq!(prefixes.min_chars, 2);
        assert_eq!(prefixes.max_chars, 5);
    }

    #[test]
    fn from_properties_value_accepts_explicit_index_prefixes_bounds() {
        // Matches the matchID `PRENOMS: { index_prefixes: { min_chars: 2, max_chars: 5 } }` shape.
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "index_prefixes": { "min_chars": 2, "max_chars": 5 }
            }
        });

        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("explicit index_prefixes bounds should parse");
        let prefixes = mapping
            .field("NOM")
            .and_then(|field| field.index_prefixes)
            .expect("index_prefixes recorded on NOM");
        assert_eq!(prefixes.min_chars, 2);
        assert_eq!(prefixes.max_chars, 5);
    }

    #[test]
    fn from_properties_value_rejects_inverted_index_prefixes_bounds() {
        // ES rejects `min_chars >= max_chars`; we must reject the mapping
        // outright so the index never reaches a half-initialised state.
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "index_prefixes": { "min_chars": 20, "max_chars": 5 }
            }
        });

        let error = IndexMapping::from_properties_value(&properties)
            .expect_err("min_chars > max_chars should be rejected");
        assert!(
            matches!(
                &error,
                MappingError::IndexPrefixesBoundsInvalid {
                    field,
                    min_chars: 20,
                    max_chars: 5,
                } if field == "NOM"
            ),
            "expected IndexPrefixesBoundsInvalid for NOM, got {error:?}",
        );
    }

    // --- A2: `geo_point` field type (matchID intake §2.12) ---

    #[test]
    fn from_properties_value_accepts_geo_point_field_type() {
        // matchID's `GEOPOINT_NAISSANCE` / `GEOPOINT_DECES` are declared
        // as `{ type: geo_point }` (intake §2.12). The mapping parser
        // must accept the type name and round-trip it back through
        // `as_value`.
        let properties = serde_json::json!({
            "GEOPOINT_NAISSANCE": { "type": "geo_point" }
        });

        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("geo_point should be accepted as a field type");
        let field = mapping
            .field("GEOPOINT_NAISSANCE")
            .expect("GEOPOINT_NAISSANCE field exists");
        assert_eq!(field.field_type, FieldType::GeoPoint);
    }

    #[test]
    fn geo_point_field_round_trips_through_as_value() {
        // `IndexMapping::as_value` powers `GET /:index/_mapping`; the
        // wire shape must preserve `"type": "geo_point"` verbatim so
        // matchID can replay its mapping from the response.
        let properties = serde_json::json!({
            "GEOPOINT_DECES": { "type": "geo_point" }
        });

        let mapping = IndexMapping::from_properties_value(&properties).expect("geo_point parse");
        let value = mapping.as_value();
        assert_eq!(
            value["properties"]["GEOPOINT_DECES"]["type"],
            serde_json::Value::String("geo_point".to_owned()),
        );
    }

    #[test]
    fn geo_point_field_rejects_analyzer_and_normalizer() {
        // `analyzer` is text-only; declaring it on a `geo_point` must
        // fail at parse time so misuse is caught at `PUT /:index`.
        let properties = serde_json::json!({
            "GEOPOINT_NAISSANCE": { "type": "geo_point", "analyzer": "norm" }
        });
        let error = IndexMapping::from_properties_value(&properties)
            .expect_err("analyzer on geo_point must be rejected");
        assert!(
            matches!(
                &error,
                MappingError::AnalyzerNotSupported { field, field_type }
                    if field == "GEOPOINT_NAISSANCE" && field_type == "geo_point",
            ),
            "got {error:?}"
        );
    }

    // ---------------------------------------------------------------------
    // A7: `date{format:yyyyMMdd}` mapping (MVP keyword-backed)
    // ---------------------------------------------------------------------

    #[test]
    fn from_properties_value_accepts_date_with_explicit_yyyymmdd_format() {
        let properties = serde_json::json!({
            "DATE_NAISSANCE": { "type": "date", "format": "yyyyMMdd" }
        });
        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("date{format:yyyyMMdd} should parse");
        let field = mapping
            .field("DATE_NAISSANCE")
            .expect("DATE_NAISSANCE field exists");
        assert_eq!(field.field_type, FieldType::Date);
        assert_eq!(field.date_format.as_deref(), Some("yyyyMMdd"));
        assert_eq!(field.date_format(), Some("yyyyMMdd"));
    }

    #[test]
    fn from_properties_value_accepts_bare_date_with_default_format() {
        let properties = serde_json::json!({
            "DATE_NAISSANCE": { "type": "date" }
        });
        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("bare `type: date` should parse");
        let field = mapping
            .field("DATE_NAISSANCE")
            .expect("DATE_NAISSANCE field exists");
        assert_eq!(field.field_type, FieldType::Date);
        assert!(field.date_format.is_none());
        assert_eq!(field.date_format(), Some(DEFAULT_DATE_FORMAT));
        assert_eq!(field.date_format(), Some("yyyyMMdd"));
    }

    #[test]
    fn from_properties_value_accepts_arbitrary_date_format_string_for_mvp() {
        let properties = serde_json::json!({
            "WHEN": { "type": "date", "format": "nonsensical" }
        });
        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("unknown date format strings are accepted at parse time");
        let field = mapping.field("WHEN").expect("WHEN field exists");
        assert_eq!(field.field_type, FieldType::Date);
        assert_eq!(field.date_format.as_deref(), Some("nonsensical"));
        let rendered = mapping.as_value();
        let format = rendered
            .pointer("/properties/WHEN/format")
            .and_then(Value::as_str);
        assert_eq!(format, Some("nonsensical"));
    }

    #[test]
    fn from_properties_value_rejects_format_on_non_date_field() {
        let properties = serde_json::json!({
            "NOM": { "type": "text", "format": "yyyyMMdd" }
        });
        let error = IndexMapping::from_properties_value(&properties)
            .expect_err("format on non-date should error");
        assert!(
            matches!(error, MappingError::DateFormatNotSupported { .. }),
            "unexpected error: {error:?}"
        );
    }

    // ---------------------------------------------------------------------
    // A10 phase 2: multi-field sub-fields (matchID intake §2.12)
    // ---------------------------------------------------------------------

    #[test]
    fn from_properties_value_parses_multi_field_subfields() {
        // matchID maps NOM with a `fields.raw` keyword sub-field carrying
        // `normalizer: norm` (intake §2.12). The parser must record the
        // sub-field on the parent's `FieldMapping.fields` map.
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "analyzer": "norm",
                "fields": {
                    "raw": { "type": "keyword", "normalizer": "norm" }
                }
            }
        });
        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("multi-field mapping should parse");
        let nom = mapping.field("NOM").expect("NOM field exists");
        assert_eq!(nom.field_type, FieldType::Text);
        assert_eq!(nom.analyzer, Some(AnalyzerName::Norm));
        let raw = nom.fields.get("raw").expect("NOM.raw sub-field recorded");
        assert_eq!(raw.field_type, FieldType::Keyword);
        assert_eq!(raw.normalizer, Some(AnalyzerName::Norm));
    }

    #[test]
    fn multi_field_subfields_round_trip_through_as_value() {
        // `IndexMapping::as_value` powers `GET /:index/_mapping`; matchID
        // replays its mapping from this response, so the `fields` block
        // must round-trip verbatim.
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "analyzer": "norm",
                "fields": {
                    "raw": { "type": "keyword", "normalizer": "norm" }
                }
            }
        });
        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("multi-field mapping should parse");
        let value = mapping.as_value();
        let raw = value
            .pointer("/properties/NOM/fields/raw")
            .expect("NOM.raw rendered under fields.raw");
        assert_eq!(raw["type"], Value::String("keyword".to_owned()));
        assert_eq!(raw["normalizer"], Value::String("norm".to_owned()));
    }

    #[test]
    fn resolve_field_walks_into_multi_field_subfields() {
        // `IndexMapping::resolve_field("NOM.raw")` must return the
        // sub-field mapping so the sort path can pick up its normalizer.
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "fields": {
                    "raw": { "type": "keyword", "normalizer": "norm" }
                }
            }
        });
        let mapping = IndexMapping::from_properties_value(&properties)
            .expect("multi-field mapping should parse");
        let raw = mapping
            .resolve_field("NOM.raw")
            .expect("NOM.raw resolvable via dotted path");
        assert_eq!(raw.field_type, FieldType::Keyword);
        assert_eq!(
            mapping.subfield_normalizer("NOM.raw"),
            Some(AnalyzerName::Norm),
        );
        // Top-level paths are unaffected.
        assert!(mapping.resolve_field("NOM").is_some());
        assert!(mapping.resolve_field("UNKNOWN.sub").is_none());
        assert!(mapping.subfield_normalizer("NOM").is_none());
    }

    #[test]
    fn from_properties_value_rejects_nested_multi_field_subfields() {
        // ES forbids nesting `fields` more than one level deep. We mirror
        // that so misuse is caught at `PUT /:index`.
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "fields": {
                    "raw": {
                        "type": "keyword",
                        "fields": {
                            "inner": { "type": "keyword" }
                        }
                    }
                }
            }
        });
        let error = IndexMapping::from_properties_value(&properties)
            .expect_err("nested fields must be rejected");
        assert!(
            matches!(
                &error,
                MappingError::NestedSubfieldsNotSupported { field } if field == "NOM.raw"
            ),
            "unexpected error: {error:?}",
        );
    }

    #[test]
    fn from_properties_value_rejects_unsupported_multi_field_subfield_type() {
        // Sub-field validation must reuse the same type checks: an
        // unknown `type` under `fields:` is rejected with the
        // qualified field path.
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "fields": {
                    "raw": { "type": "bogus" }
                }
            }
        });
        let error = IndexMapping::from_properties_value(&properties)
            .expect_err("unsupported sub-field type must error");
        assert!(
            matches!(
                &error,
                MappingError::UnsupportedFieldType { field, field_type }
                    if field == "NOM.raw" && field_type == "bogus"
            ),
            "unexpected error: {error:?}",
        );
    }
}
