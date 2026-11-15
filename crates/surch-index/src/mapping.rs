use surch_analysis::{
    Analyzer, CharFilter, CustomAnalyzer, KeywordAnalyzer, NormAnalyzer, Normalizer,
    PatternReplaceCharFilter, SimpleAnalyzer, StandardAnalyzer, StopAnalyzer, Token, TokenChars,
    TokenFilter, Tokenizer, WhitespaceAnalyzer, DEFAULT_MAX_TOKEN_LENGTH,
};

use std::collections::{BTreeMap, BTreeSet};
use std::iter::FromIterator;
use std::sync::Arc;

use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Text,
    Keyword,
    Integer,
    Long,
    /// matchID `AGE_DECES` uses ES `short`; `byte` is the narrower sibling.
    /// Both are stored/compared as source JSON numbers exactly like
    /// `Integer` (no dedicated numeric path keys off the variant) and only
    /// round-trip distinctly in `_mapping` for ES parity.
    Short,
    Byte,
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
            Self::Short => "short",
            Self::Byte => "byte",
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
            "short" => Some(Self::Short),
            "byte" => Some(Self::Byte),
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

    fn token_stream(&self, text: &str) -> Vec<Token> {
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

    /// ES default analysis per field type: `text` uses the `standard`
    /// analyzer (UAX#29 + lowercase, digits kept), every other type indexes
    /// its value verbatim.
    pub fn default_for(field_type: FieldType) -> Self {
        match field_type {
            FieldType::Text => Self::Standard,
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
    /// Raw `analyzer` name when it is NOT one of the builtins (e.g. a
    /// user-defined `autocomplete_analyzer` declared in
    /// `settings.analysis.analyzer`). Kept verbatim so it round-trips in
    /// `GET _mapping` and resolves at index/query time against the index's
    /// [`AnalysisSettings`] via [`AnalysisSettings::resolve_analyzer`].
    /// Mutually exclusive with `analyzer` (a builtin sets `analyzer`, a
    /// custom name sets this).
    pub custom_analyzer: Option<String>,
    /// Field-level `search_analyzer` (builtin). When set it overrides
    /// `analyzer` at QUERY time only — the ES pattern for edge_ngram
    /// autocomplete: index with `edge_ngram`, search with `standard` so the
    /// query is not itself ngram-expanded. Absent => query uses `analyzer`.
    pub search_analyzer: Option<AnalyzerName>,
    /// Raw `search_analyzer` name when it is a non-builtin (resolved against
    /// [`AnalysisSettings`] at query time). Mutually exclusive with
    /// `search_analyzer`.
    pub custom_search_analyzer: Option<String>,
    pub norms: Option<bool>,
    /// Field-level `normalizer` reference for keyword fields when its name
    /// is one of the builtin names (`norm`, ...). At index/query time a
    /// normalizer of the same name declared in `settings.analysis.normalizer`
    /// takes precedence (see [`FieldMapping::index_analyzer`]); otherwise the
    /// legacy builtin lowercase + asciifolding chain applies.
    pub normalizer: Option<AnalyzerName>,
    /// Raw `normalizer` name when it is not a builtin name (e.g. `lowercase`
    /// or a user-defined normalizer). Mutually exclusive with `normalizer`.
    pub custom_normalizer: Option<String>,
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
    /// A10 phase 3 wires the write-time fan-out: at index time
    /// `DocumentIndex::index_subfields` re-analyzes the parent value with
    /// each sub-field's own chain and stores it under the `parent.subname`
    /// path (both as postings and as a per-doc stored projection). At
    /// query time `lookup_sort_value` still aliases `parent.subname` to the
    /// parent's stored value and normalises on read; threading the stored
    /// projection (`DocumentIndex::subfield_value`) into sort/agg is the
    /// remaining follow-up.
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
            custom_analyzer: None,
            search_analyzer: None,
            custom_search_analyzer: None,
            norms,
            normalizer: None,
            custom_normalizer: None,
            index_prefixes: None,
            date_format: None,
            fields: BTreeMap::new(),
        }
    }

    /// Sets the raw custom analyzer name (a non-builtin declared in
    /// `settings.analysis.analyzer`). See [`FieldMapping::custom_analyzer`].
    pub fn with_custom_analyzer(mut self, custom_analyzer: Option<String>) -> Self {
        self.custom_analyzer = custom_analyzer;
        self
    }

    /// Sets the query-time `search_analyzer` (builtin and/or custom name).
    /// See [`FieldMapping::search_analyzer`].
    pub fn with_search_analyzer(
        mut self,
        search_analyzer: Option<AnalyzerName>,
        custom_search_analyzer: Option<String>,
    ) -> Self {
        self.search_analyzer = search_analyzer;
        self.custom_search_analyzer = custom_search_analyzer;
        self
    }

    pub fn with_normalizer(mut self, normalizer: Option<AnalyzerName>) -> Self {
        self.normalizer = normalizer;
        self
    }

    /// Sets a non-builtin `normalizer` name (see
    /// [`FieldMapping::custom_normalizer`]).
    pub fn with_custom_normalizer(mut self, normalizer: Option<String>) -> Self {
        self.custom_normalizer = normalizer;
        self
    }

    /// The declared `normalizer` name, builtin or custom.
    pub fn normalizer_name(&self) -> Option<&str> {
        self.custom_normalizer
            .as_deref()
            .or_else(|| self.normalizer.map(|name| name.as_str()))
    }

    /// The declared index `analyzer` name, builtin or custom.
    pub fn analyzer_name(&self) -> Option<&str> {
        self.custom_analyzer
            .as_deref()
            .or_else(|| self.analyzer.map(|name| name.as_str()))
    }

    /// The declared `search_analyzer` name, builtin or custom.
    pub fn search_analyzer_name(&self) -> Option<&str> {
        self.custom_search_analyzer
            .as_deref()
            .or_else(|| self.search_analyzer.map(|name| name.as_str()))
    }

    /// Index-time analysis chain of this field, resolved against the index
    /// `settings.analysis` (ES semantics):
    /// - `text`: the declared `analyzer` (a `settings.analysis.analyzer`
    ///   definition shadows a builtin of the same name), else the index
    ///   `default` analyzer, else `standard`;
    /// - `keyword`: the declared `normalizer` applied to the whole value
    ///   (single token), else the value verbatim;
    /// - any other type: the value verbatim.
    ///
    /// An analyzer or normalizer name that resolves nowhere falls back to
    /// the type default (`standard` / verbatim) on both the index and the
    /// query side, so postings and query terms stay consistent. ES rejects
    /// such a mapping at creation instead (documented deviation).
    pub fn index_analyzer(&self, analysis: &AnalysisSettings) -> ResolvedAnalyzer {
        match self.field_type {
            FieldType::Text => match self.analyzer_name() {
                Some(name) => analysis
                    .resolve_analyzer(name)
                    .unwrap_or(ResolvedAnalyzer::Builtin(AnalyzerName::Standard)),
                None => analysis.default_index_analyzer(),
            },
            FieldType::Keyword => match self.normalizer_name() {
                Some(name) => analysis
                    .resolve_normalizer(name)
                    .unwrap_or(ResolvedAnalyzer::Builtin(AnalyzerName::Keyword)),
                None => ResolvedAnalyzer::Builtin(AnalyzerName::Keyword),
            },
            _ => ResolvedAnalyzer::Builtin(AnalyzerName::Keyword),
        }
    }

    /// Query-time analysis chain (`match` & co): a `text` field's
    /// `search_analyzer` when declared, else the index `default_search`
    /// analyzer when the field declares no analyzer, else its index
    /// analyzer. Keyword fields analyse the query with their normalizer
    /// (one term), as ES does.
    pub fn search_analyzer(&self, analysis: &AnalysisSettings) -> ResolvedAnalyzer {
        if self.field_type == FieldType::Text {
            if let Some(name) = self.search_analyzer_name() {
                return analysis
                    .resolve_analyzer(name)
                    .unwrap_or(ResolvedAnalyzer::Builtin(AnalyzerName::Standard));
            }
            if self.analyzer_name().is_none() {
                if let Some(default_search) = analysis.default_search_analyzer() {
                    return default_search;
                }
            }
        }
        self.index_analyzer(analysis)
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
        } else if let Some(custom) = &self.custom_analyzer {
            object.insert("analyzer".to_owned(), Value::String(custom.clone()));
        }

        if let Some(search_analyzer) = self.search_analyzer {
            object.insert(
                "search_analyzer".to_owned(),
                Value::String(search_analyzer.as_str().to_owned()),
            );
        } else if let Some(custom) = &self.custom_search_analyzer {
            object.insert("search_analyzer".to_owned(), Value::String(custom.clone()));
        }

        if let Some(norms) = self.norms {
            object.insert("norms".to_owned(), Value::Bool(norms));
        }

        if let Some(normalizer) = self.normalizer_name() {
            object.insert(
                "normalizer".to_owned(),
                Value::String(normalizer.to_owned()),
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

/// The five `settings.analysis` sections, in the order ES applies them.
const ANALYSIS_SECTIONS: [&str; 5] = [
    "char_filter",
    "tokenizer",
    "filter",
    "analyzer",
    "normalizer",
];

/// Parsed and compiled `settings.analysis` block of an index.
///
/// The declared JSON is kept verbatim (it is what `GET _settings` shows and
/// what persistence stores); every `analyzer` and `normalizer` is compiled
/// once, at parse time, into a [`CustomAnalyzer`] chain: char filters, then
/// the tokenizer, then token filters, in declaration order. A definition
/// that cannot be compiled (unsupported component type, invalid regex,
/// reference to an undeclared component) fails the parse, so index creation
/// fails closed instead of analysing differently from ES.
///
/// Supported components (others are rejected with
/// [`MappingError::AnalysisComponent`]):
/// - char filters: `pattern_replace` (`pattern`, `replacement`, `flags`);
/// - tokenizers: `standard` (`max_token_length`), `keyword`, `whitespace`,
///   `letter`, `lowercase`, `edge_ngram` (`min_gram`, `max_gram`,
///   `token_chars`, `custom_token_chars`);
/// - token filters: `lowercase`, `uppercase`, `asciifolding`
///   (`preserve_original`), `trim`, `stop` (`stopwords`, `ignore_case`);
/// - analyzer types: `custom`, `standard`, `simple`, `whitespace`,
///   `keyword`, `stop`; normalizer type: `custom`.
#[derive(Debug, Clone, Default)]
pub struct AnalysisSettings {
    source: Map<String, Value>,
    analyzers: BTreeMap<String, Arc<CustomAnalyzer>>,
    normalizers: BTreeMap<String, Arc<CustomAnalyzer>>,
}

impl PartialEq for AnalysisSettings {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for AnalysisSettings {}

impl AnalysisSettings {
    /// Parses and compiles an `analysis` object (the value of
    /// `settings.analysis`).
    pub fn from_analysis_value(analysis: &Value) -> Result<Self, MappingError> {
        let analysis = analysis
            .as_object()
            .ok_or(MappingError::AnalysisNotObject)?;
        let mut source = Map::new();
        for (section, entries) in analysis {
            if !ANALYSIS_SECTIONS.contains(&section.as_str()) {
                // Unknown sections are kept verbatim (forward compatible).
                source.insert(section.clone(), entries.clone());
                continue;
            }
            let entries =
                entries
                    .as_object()
                    .ok_or_else(|| MappingError::AnalysisSectionNotObject {
                        section: section.clone(),
                    })?;
            for (name, definition) in entries {
                if !definition.is_object() {
                    return Err(MappingError::AnalysisEntryNotObject {
                        section: section.clone(),
                        name: name.clone(),
                    });
                }
            }
            source.insert(section.clone(), Value::Object(entries.clone()));
        }

        let mut settings = Self {
            source,
            analyzers: BTreeMap::new(),
            normalizers: BTreeMap::new(),
        };
        let compiler = ChainCompiler {
            source: &settings.source,
        };
        let mut analyzers = BTreeMap::new();
        for (name, definition) in compiler.section("analyzer") {
            let chain = compiler.analyzer(name, definition)?;
            analyzers.insert(name.clone(), Arc::new(chain));
        }
        let mut normalizers = BTreeMap::new();
        for (name, definition) in compiler.section("normalizer") {
            let chain = compiler.normalizer(name, definition)?;
            normalizers.insert(name.clone(), Arc::new(chain));
        }
        settings.analyzers = analyzers;
        settings.normalizers = normalizers;
        Ok(settings)
    }

    /// The declared `analysis` object, verbatim.
    pub fn to_value(&self) -> Value {
        Value::Object(self.source.clone())
    }

    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    /// Names of the declared (compiled) analyzers.
    pub fn analyzer_names(&self) -> impl Iterator<Item = &str> {
        self.analyzers.keys().map(String::as_str)
    }

    /// Names of the declared (compiled) normalizers.
    pub fn normalizer_names(&self) -> impl Iterator<Item = &str> {
        self.normalizers.keys().map(String::as_str)
    }

    /// Resolves an analyzer name: a declared `settings.analysis.analyzer`
    /// first (it shadows a builtin of the same name, e.g. the deces `norm`),
    /// then the builtins (`standard`, `simple`, `stop`, `keyword`,
    /// `whitespace`, Surch's legacy `norm`).
    pub fn resolve_analyzer(&self, name: &str) -> Option<ResolvedAnalyzer> {
        if let Some(chain) = self.analyzers.get(name) {
            return Some(ResolvedAnalyzer::Custom(Arc::clone(chain)));
        }
        AnalyzerName::from_name(name).map(ResolvedAnalyzer::Builtin)
    }

    /// Resolves a normalizer name: a declared
    /// `settings.analysis.normalizer` first, then ES's builtin `lowercase`
    /// normalizer, then Surch's legacy builtin names (lowercase +
    /// asciifolding on the whole value).
    pub fn resolve_normalizer(&self, name: &str) -> Option<ResolvedAnalyzer> {
        if let Some(chain) = self.normalizers.get(name) {
            return Some(ResolvedAnalyzer::Custom(Arc::clone(chain)));
        }
        if name == "lowercase" {
            return Some(ResolvedAnalyzer::Custom(Arc::new(CustomAnalyzer::new(
                Vec::new(),
                Tokenizer::Keyword,
                vec![TokenFilter::Lowercase],
            ))));
        }
        AnalyzerName::from_name(name).map(|_| ResolvedAnalyzer::LegacyNormalizer)
    }

    /// Index analyzer of a `text` field that declares none: the index
    /// `default` analyzer when declared, else `standard`.
    pub fn default_index_analyzer(&self) -> ResolvedAnalyzer {
        self.analyzers
            .get("default")
            .map(|chain| ResolvedAnalyzer::Custom(Arc::clone(chain)))
            .unwrap_or(ResolvedAnalyzer::Builtin(AnalyzerName::Standard))
    }

    /// The index `default_search` analyzer, when declared.
    pub fn default_search_analyzer(&self) -> Option<ResolvedAnalyzer> {
        self.analyzers
            .get("default_search")
            .map(|chain| ResolvedAnalyzer::Custom(Arc::clone(chain)))
    }
}

/// Builds [`CustomAnalyzer`] chains from the declared sections.
struct ChainCompiler<'a> {
    source: &'a Map<String, Value>,
}

impl<'a> ChainCompiler<'a> {
    fn section(&self, section: &str) -> impl Iterator<Item = (&'a String, &'a Map<String, Value>)> {
        self.source
            .get(section)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .filter_map(|(name, definition)| definition.as_object().map(|object| (name, object)))
    }

    fn declared(&self, section: &str, name: &str) -> Option<&'a Map<String, Value>> {
        self.source.get(section)?.get(name)?.as_object()
    }

    fn analyzer(
        &self,
        name: &str,
        definition: &Map<String, Value>,
    ) -> Result<CustomAnalyzer, MappingError> {
        let error = |reason: String| component_error("analyzer", name, reason);
        let kind = match definition.get("type") {
            Some(value) => value
                .as_str()
                .ok_or_else(|| error("`type` must be a string".to_owned()))?,
            None if definition.contains_key("tokenizer") => "custom",
            None => {
                return Err(error(
                    "must specify either an analyzer type, or a tokenizer".to_owned(),
                ))
            }
        };
        match kind {
            "custom" => {
                let tokenizer_name = definition
                    .get("tokenizer")
                    .and_then(Value::as_str)
                    .ok_or_else(|| error("custom analyzer requires a `tokenizer`".to_owned()))?;
                let char_filters = names(definition.get("char_filter"))
                    .map_err(|reason| error(format!("`char_filter` {reason}")))?
                    .iter()
                    .map(|filter| self.char_filter(filter))
                    .collect::<Result<Vec<_>, _>>()?;
                let tokenizer = self.tokenizer(tokenizer_name)?;
                let filters = names(definition.get("filter"))
                    .map_err(|reason| error(format!("`filter` {reason}")))?
                    .iter()
                    .map(|filter| self.token_filter(filter))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(CustomAnalyzer::new(char_filters, tokenizer, filters))
            }
            "standard" => {
                let max_token_length = usize_param(definition, "max_token_length")
                    .map_err(error)?
                    .unwrap_or(DEFAULT_MAX_TOKEN_LENGTH);
                let stop = stop_words(definition.get("stopwords"), false).map_err(error)?;
                let stop = (!stop.is_empty()).then_some(TokenFilter::Stop {
                    words: stop,
                    ignore_case: false,
                });
                Ok(CustomAnalyzer::standard(max_token_length, stop))
            }
            "simple" => Ok(CustomAnalyzer::new(
                Vec::new(),
                Tokenizer::Lowercase,
                Vec::new(),
            )),
            "whitespace" => Ok(CustomAnalyzer::new(
                Vec::new(),
                Tokenizer::Whitespace,
                Vec::new(),
            )),
            "keyword" => Ok(CustomAnalyzer::new(
                Vec::new(),
                Tokenizer::Keyword,
                Vec::new(),
            )),
            "stop" => {
                let words = stop_words(definition.get("stopwords"), true).map_err(error)?;
                Ok(CustomAnalyzer::new(
                    Vec::new(),
                    Tokenizer::Lowercase,
                    vec![TokenFilter::Stop {
                        words,
                        ignore_case: false,
                    }],
                ))
            }
            other => Err(error(format!("analyzer type [{other}] is not supported"))),
        }
    }

    fn normalizer(
        &self,
        name: &str,
        definition: &Map<String, Value>,
    ) -> Result<CustomAnalyzer, MappingError> {
        let error = |reason: String| component_error("normalizer", name, reason);
        let kind = definition
            .get("type")
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| error("`type` must be a string".to_owned()))
            })
            .transpose()?
            .unwrap_or("custom");
        if kind != "custom" {
            return Err(error(format!("normalizer type [{kind}] is not supported")));
        }
        if definition.contains_key("tokenizer") {
            return Err(error("a normalizer cannot declare a tokenizer".to_owned()));
        }
        let char_filters = names(definition.get("char_filter"))
            .map_err(|reason| error(format!("`char_filter` {reason}")))?
            .iter()
            .map(|filter| self.char_filter(filter))
            .collect::<Result<Vec<_>, _>>()?;
        let filters = names(definition.get("filter"))
            .map_err(|reason| error(format!("`filter` {reason}")))?
            .iter()
            .map(|filter| self.token_filter(filter))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CustomAnalyzer::new(
            char_filters,
            Tokenizer::Keyword,
            filters,
        ))
    }

    fn char_filter(&self, name: &str) -> Result<CharFilter, MappingError> {
        let error = |reason: String| component_error("char_filter", name, reason);
        let Some(definition) = self.declared("char_filter", name) else {
            return Err(error(format!(
                "char_filter [{name}] is neither declared nor a supported builtin"
            )));
        };
        let kind = definition.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "pattern_replace" => {
                let pattern = definition
                    .get("pattern")
                    .and_then(Value::as_str)
                    .ok_or_else(|| error("pattern_replace requires a `pattern`".to_owned()))?;
                let replacement = definition
                    .get("replacement")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let flags = definition
                    .get("flags")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                PatternReplaceCharFilter::new(pattern, replacement, flags)
                    .map(CharFilter::PatternReplace)
                    .map_err(|analysis| error(analysis.message))
            }
            other => Err(error(format!(
                "char_filter type [{other}] is not supported"
            ))),
        }
    }

    fn tokenizer(&self, name: &str) -> Result<Tokenizer, MappingError> {
        let error = |reason: String| component_error("tokenizer", name, reason);
        let Some(definition) = self.declared("tokenizer", name) else {
            return builtin_tokenizer(name).ok_or_else(|| {
                error(format!(
                    "tokenizer [{name}] is neither declared nor a supported builtin"
                ))
            });
        };
        let kind = definition.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "standard" => Ok(Tokenizer::Standard {
                max_token_length: usize_param(definition, "max_token_length")
                    .map_err(error)?
                    .unwrap_or(DEFAULT_MAX_TOKEN_LENGTH),
            }),
            "keyword" => Ok(Tokenizer::Keyword),
            "whitespace" => Ok(Tokenizer::Whitespace),
            "letter" => Ok(Tokenizer::Letter),
            "lowercase" => Ok(Tokenizer::Lowercase),
            "edge_ngram" | "edgeNGram" => {
                let min_gram = usize_param(definition, "min_gram")
                    .map_err(error)?
                    .unwrap_or(1);
                let max_gram = usize_param(definition, "max_gram")
                    .map_err(error)?
                    .unwrap_or(2);
                if min_gram == 0 || min_gram > max_gram {
                    return Err(error(format!(
                        "edge_ngram requires 1 <= min_gram <= max_gram (got {min_gram}, {max_gram})"
                    )));
                }
                let token_chars = token_chars(definition).map_err(error)?;
                Ok(Tokenizer::EdgeNgram {
                    min_gram,
                    max_gram,
                    token_chars,
                })
            }
            other => Err(error(format!("tokenizer type [{other}] is not supported"))),
        }
    }

    fn token_filter(&self, name: &str) -> Result<TokenFilter, MappingError> {
        let error = |reason: String| component_error("filter", name, reason);
        let Some(definition) = self.declared("filter", name) else {
            return builtin_token_filter(name).ok_or_else(|| {
                error(format!(
                    "filter [{name}] is neither declared nor a supported builtin"
                ))
            });
        };
        let kind = definition.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "lowercase" if definition.get("language").is_none() => Ok(TokenFilter::Lowercase),
            "uppercase" => Ok(TokenFilter::Uppercase),
            "trim" => Ok(TokenFilter::Trim),
            "asciifolding" => Ok(TokenFilter::AsciiFolding {
                preserve_original: bool_param(definition, "preserve_original")
                    .map_err(error)?
                    .unwrap_or(false),
            }),
            "stop" => {
                let ignore_case = bool_param(definition, "ignore_case")
                    .map_err(error)?
                    .unwrap_or(false);
                let mut words = stop_words(definition.get("stopwords"), true).map_err(error)?;
                if ignore_case {
                    words = words
                        .iter()
                        .map(|word| surch_analysis::java_lowercase(word))
                        .collect();
                }
                Ok(TokenFilter::Stop { words, ignore_case })
            }
            other => Err(error(format!("filter type [{other}] is not supported"))),
        }
    }
}

fn builtin_tokenizer(name: &str) -> Option<Tokenizer> {
    Some(match name {
        "standard" => Tokenizer::Standard {
            max_token_length: DEFAULT_MAX_TOKEN_LENGTH,
        },
        "keyword" => Tokenizer::Keyword,
        "whitespace" => Tokenizer::Whitespace,
        "letter" => Tokenizer::Letter,
        "lowercase" => Tokenizer::Lowercase,
        "edge_ngram" | "edgeNGram" => Tokenizer::EdgeNgram {
            min_gram: 1,
            max_gram: 2,
            token_chars: TokenChars::default(),
        },
        _ => return None,
    })
}

fn builtin_token_filter(name: &str) -> Option<TokenFilter> {
    Some(match name {
        "lowercase" => TokenFilter::Lowercase,
        "uppercase" => TokenFilter::Uppercase,
        "asciifolding" => TokenFilter::AsciiFolding {
            preserve_original: false,
        },
        "trim" => TokenFilter::Trim,
        "stop" => TokenFilter::english_stop(),
        _ => return None,
    })
}

fn component_error(section: &str, name: &str, reason: String) -> MappingError {
    MappingError::AnalysisComponent {
        section: section.to_owned(),
        name: name.to_owned(),
        reason,
    }
}

/// A component list: a JSON array of names, or a single name.
fn names(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(name)) => Ok(vec![name.clone()]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "entries must be strings".to_owned())
            })
            .collect(),
        Some(_) => Err("must be a string or an array of strings".to_owned()),
    }
}

/// ES accepts numbers and numeric strings for integer parameters.
fn usize_param(definition: &Map<String, Value>, key: &str) -> Result<Option<usize>, String> {
    match definition.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .map(|value| Some(value as usize))
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
        Some(Value::String(text)) => text
            .parse::<usize>()
            .map(Some)
            .map_err(|_| format!("`{key}` must be a non-negative integer")),
        Some(_) => Err(format!("`{key}` must be a non-negative integer")),
    }
}

fn bool_param(definition: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match definition.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::String(text)) if text == "true" || text == "false" => Ok(Some(text == "true")),
        Some(_) => Err(format!("`{key}` must be a boolean")),
    }
}

/// `stopwords`: a list, `_english_`, or `_none_`. `english_default` is the
/// value when absent (`stop` filter / analyzer: `_english_`; `standard`
/// analyzer: `_none_`).
fn stop_words(value: Option<&Value>, english_default: bool) -> Result<BTreeSet<String>, String> {
    let english = || {
        surch_analysis::ENGLISH_STOP_WORDS
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<BTreeSet<_>>()
    };
    match value {
        None | Some(Value::Null) => Ok(if english_default {
            english()
        } else {
            BTreeSet::new()
        }),
        Some(Value::String(name)) if name == "_english_" => Ok(english()),
        Some(Value::String(name)) if name == "_none_" => Ok(BTreeSet::new()),
        Some(Value::String(name)) => Err(format!("stopwords set [{name}] is not supported")),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "`stopwords` entries must be strings".to_owned())
            })
            .collect(),
        Some(_) => Err("`stopwords` must be a list or a predefined set".to_owned()),
    }
}

fn token_chars(definition: &Map<String, Value>) -> Result<TokenChars, String> {
    let mut chars = TokenChars::default();
    for class in
        names(definition.get("token_chars")).map_err(|reason| format!("`token_chars` {reason}"))?
    {
        match class.as_str() {
            "letter" => chars.letter = true,
            "digit" => chars.digit = true,
            "whitespace" => chars.whitespace = true,
            "punctuation" => chars.punctuation = true,
            "symbol" => chars.symbol = true,
            "custom" => {
                let custom = definition
                    .get("custom_token_chars")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        "token_chars `custom` requires `custom_token_chars`".to_owned()
                    })?;
                chars.custom.extend(custom.chars());
            }
            other => return Err(format!("unknown token_chars class [{other}]")),
        }
    }
    Ok(chars)
}

/// A resolved, runnable analysis chain for one field.
#[derive(Debug, Clone)]
pub enum ResolvedAnalyzer {
    /// One of Surch's builtin analyzers.
    Builtin(AnalyzerName),
    /// Surch's legacy builtin normalizer (lowercase + asciifolding on the
    /// whole value), used when a keyword field names a builtin normalizer
    /// that `settings.analysis` does not declare.
    LegacyNormalizer,
    /// A chain compiled from `settings.analysis`.
    Custom(Arc<CustomAnalyzer>),
}

impl PartialEq for ResolvedAnalyzer {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Builtin(left), Self::Builtin(right)) => left == right,
            (Self::LegacyNormalizer, Self::LegacyNormalizer) => true,
            (Self::Custom(left), Self::Custom(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl ResolvedAnalyzer {
    /// The token stream (terms, offsets, position increments).
    pub fn tokens(&self, text: &str) -> Vec<Token> {
        match self {
            Self::Builtin(name) => name.token_stream(text),
            Self::LegacyNormalizer => Normalizer.token_stream(text),
            Self::Custom(chain) => chain.token_stream(text),
        }
    }

    /// All terms the analyzer emits for `text`, in emission order.
    pub fn terms(&self, text: &str) -> Vec<String> {
        self.tokens(text)
            .into_iter()
            .map(|token| token.term)
            .collect()
    }

    /// The first emitted term (or empty string), for single-value projections.
    pub fn first_term(&self, text: &str) -> String {
        self.tokens(text)
            .into_iter()
            .next()
            .map(|token| token.term)
            .unwrap_or_default()
    }
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

    /// Index-time analysis chain of `field` (a top-level field or a
    /// multi-field sub-field `parent.sub`), see
    /// [`FieldMapping::index_analyzer`]. An unmapped field is analysed as a
    /// `text` field without analyzer (index `default`, else `standard`).
    pub fn index_analyzer(&self, field: &str) -> ResolvedAnalyzer {
        match self.resolve_field(field) {
            Some(mapping) => mapping.index_analyzer(&self.analysis),
            None => self.analysis.default_index_analyzer(),
        }
    }

    /// Query-time analysis chain of `field` (`match`, `match_phrase`,
    /// `multi_match`, fuzzy `match`), see [`FieldMapping::search_analyzer`].
    pub fn search_analyzer(&self, field: &str) -> ResolvedAnalyzer {
        match self.resolve_field(field) {
            Some(mapping) => mapping.search_analyzer(&self.analysis),
            None => self
                .analysis
                .default_search_analyzer()
                .unwrap_or_else(|| self.analysis.default_index_analyzer()),
        }
    }

    /// The terms the index holds for `value` in `field`.
    pub fn index_terms(&self, field: &str, value: &str) -> Vec<String> {
        self.index_analyzer(field).terms(value)
    }

    /// The terms a full-text query on `field` looks up for `value`.
    pub fn search_terms(&self, field: &str, value: &str) -> Vec<String> {
        self.search_analyzer(field).terms(value)
    }

    /// The single term a term-level query (`term`, `terms`, `prefix`) looks
    /// up for `value` in `field`. On a `keyword` field this is the value
    /// passed through the field's normalizer (ES `normalize`), verbatim
    /// without one. On a `text` field ES does not analyse term-level
    /// queries; Surch keeps its historical behaviour there (first term of
    /// the search analyzer), a documented deviation.
    pub fn term_query_value(&self, field: &str, value: &str) -> String {
        match self.resolve_field(field) {
            Some(mapping) if mapping.field_type != FieldType::Text => {
                mapping.index_analyzer(&self.analysis).first_term(value)
            }
            _ => self.search_analyzer(field).first_term(value),
        }
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
    pub fn subfield_normalizer(&self, path: &str) -> Option<ResolvedAnalyzer> {
        let (parent, sub) = path.rsplit_once('.')?;
        let sub = self.fields.get(parent)?.fields.get(sub)?;
        sub.normalizer_name()?;
        Some(sub.index_analyzer(&self.analysis))
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

    /// Parses and compiles the `analysis` block of a `PUT /:index`
    /// `settings` object (`settings.analysis`, or the long form
    /// `settings.index.analysis`). See [`AnalysisSettings`] for the
    /// supported components; an absent block yields empty settings.
    pub fn from_index_settings_value(value: &Value) -> Result<AnalysisSettings, MappingError> {
        let settings = value.as_object().ok_or(MappingError::SettingsNotObject)?;
        let analysis = settings.get("analysis").or_else(|| {
            settings
                .get("index")
                .and_then(Value::as_object)
                .and_then(|index| index.get("analysis"))
        });
        match analysis {
            Some(analysis) => AnalysisSettings::from_analysis_value(analysis),
            None => Ok(AnalysisSettings::default()),
        }
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

    // A1/A13: a non-builtin analyzer name (e.g. `autocomplete_analyzer`) is
    // accepted and kept verbatim as `custom_analyzer`; it resolves at
    // index/query time against the index `settings.analysis` block (which
    // is not in scope here). Builtins still resolve eagerly to `analyzer`.
    let (analyzer, custom_analyzer) = match object.get("analyzer") {
        Some(value) => {
            let value = value
                .as_str()
                .ok_or_else(|| MappingError::AnalyzerNotString {
                    field: field.to_owned(),
                })?;
            match AnalyzerName::from_name(value) {
                Some(builtin) => (Some(builtin), None),
                None => (None, Some(value.to_owned())),
            }
        }
        None => (None, None),
    };

    // A1/A13: `search_analyzer` overrides `analyzer` at query time. Same
    // builtin-vs-custom split as `analyzer`.
    let (search_analyzer, custom_search_analyzer) = match object.get("search_analyzer") {
        Some(value) => {
            let value = value
                .as_str()
                .ok_or_else(|| MappingError::AnalyzerNotString {
                    field: field.to_owned(),
                })?;
            match AnalyzerName::from_name(value) {
                Some(builtin) => (Some(builtin), None),
                None => (None, Some(value.to_owned())),
            }
        }
        None => (None, None),
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

    if field_type != FieldType::Text
        && (analyzer.is_some()
            || custom_analyzer.is_some()
            || search_analyzer.is_some()
            || custom_search_analyzer.is_some())
    {
        return Err(MappingError::AnalyzerNotSupported {
            field: field.to_owned(),
            field_type: field_type.as_str().to_owned(),
        });
    }

    // A builtin name (`norm`, ...) is kept as `normalizer`; any other name
    // (ES builtin `lowercase`, a user-defined normalizer) as
    // `custom_normalizer`. Both resolve against `settings.analysis` at
    // index/query time.
    let (normalizer, custom_normalizer) = match object.get("normalizer") {
        Some(value) => {
            let value = value
                .as_str()
                .ok_or_else(|| MappingError::NormalizerNotString {
                    field: field.to_owned(),
                })?;
            if field_type != FieldType::Keyword {
                return Err(MappingError::UnsupportedNormalizer {
                    field: field.to_owned(),
                    normalizer: value.to_owned(),
                });
            }
            match AnalyzerName::from_name(value) {
                Some(builtin) => (Some(builtin), None),
                None => (None, Some(value.to_owned())),
            }
        }
        None => (None, None),
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
        .with_custom_analyzer(custom_analyzer)
        .with_search_analyzer(search_analyzer, custom_search_analyzer)
        .with_normalizer(normalizer)
        .with_custom_normalizer(custom_normalizer)
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
    #[error("settings.analysis.{section}.{name}: {reason}")]
    AnalysisComponent {
        section: String,
        name: String,
        reason: String,
    },
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

    fn deces_autocomplete_settings() -> AnalysisSettings {
        let value = serde_json::json!({
            "analysis": {
                "tokenizer": {
                    "edge_ngram_tokenizer": {
                        "type": "edge_ngram",
                        "min_gram": 2,
                        "max_gram": 4,
                        "token_chars": ["letter", "digit"]
                    }
                },
                "analyzer": {
                    "autocomplete_analyzer": {
                        "tokenizer": "edge_ngram_tokenizer",
                        "filter": ["lowercase", "asciifolding"]
                    }
                }
            }
        });
        IndexMapping::from_index_settings_value(&value).expect("settings parse")
    }

    #[test]
    fn resolve_analyzer_returns_builtin_for_known_names() {
        let settings = AnalysisSettings::default();
        assert_eq!(
            settings.resolve_analyzer("standard"),
            Some(ResolvedAnalyzer::Builtin(AnalyzerName::Standard))
        );
        assert_eq!(
            settings.resolve_analyzer("norm"),
            Some(ResolvedAnalyzer::Builtin(AnalyzerName::Norm))
        );
    }

    #[test]
    fn resolve_analyzer_composes_custom_edge_ngram_chain() {
        let settings = deces_autocomplete_settings();
        let resolved = settings
            .resolve_analyzer("autocomplete_analyzer")
            .expect("custom analyzer resolves");
        assert!(matches!(resolved, ResolvedAnalyzer::Custom(_)));
        // Edge-ngram prefixes of "Élan" (len 2..=4) then lowercase +
        // asciifold: "Él"/"Éla"/"Élan" -> "el"/"ela"/"elan".
        assert_eq!(resolved.terms("Élan"), vec!["el", "ela", "elan"]);
        assert_eq!(resolved.first_term("Élan"), "el");
    }

    #[test]
    fn resolve_analyzer_unknown_name_is_none() {
        let settings = deces_autocomplete_settings();
        assert_eq!(settings.resolve_analyzer("does_not_exist"), None);
    }

    fn nom_autocomplete_index_mapping() -> IndexMapping {
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "analyzer": "norm",
                "fields": {
                    "autocomplete": {
                        "type": "text",
                        "analyzer": "autocomplete_analyzer",
                        "search_analyzer": "standard"
                    }
                }
            }
        });
        let mut mapping = IndexMapping::from_properties_value(&properties).expect("mapping parse");
        mapping.set_analysis(deces_autocomplete_settings());
        mapping
    }

    #[test]
    fn search_terms_use_search_analyzer_not_edge_ngram() {
        let mapping = nom_autocomplete_index_mapping();
        // The autocomplete sub-field indexes edge_ngram prefixes, but its
        // `search_analyzer: standard` tokenizes the QUERY whole (lowercased),
        // so "Dupont" -> ["dupont"], NOT the ngram expansion.
        assert_eq!(
            mapping.search_terms("NOM.autocomplete", "Dupont"),
            vec!["dupont".to_owned()]
        );
        assert_eq!(
            mapping.index_terms("NOM.autocomplete", "Dupont"),
            vec!["du", "dup", "dupo"]
        );
    }

    #[test]
    fn custom_analyzer_without_search_analyzer_uses_index_analyzer_at_query() {
        // No `search_analyzer` => ES defaults query analysis to the index
        // analyzer, so the query IS edge_ngram-expanded (the caller must set
        // search_analyzer to avoid this, as the autocomplete test does).
        let properties = serde_json::json!({
            "NOM": {
                "type": "text",
                "fields": {
                    "ac": { "type": "text", "analyzer": "autocomplete_analyzer" }
                }
            }
        });
        let mut mapping = IndexMapping::from_properties_value(&properties).expect("mapping parse");
        mapping.set_analysis(deces_autocomplete_settings());
        assert_eq!(
            mapping.search_terms("NOM.ac", "Dup"),
            vec!["du".to_owned(), "dup".to_owned()]
        );
    }

    /// `settings.analysis` of matchID's `deces_index.yml` (main `6732755`),
    /// as the Python YAML loader hands it to ES: the duplicated
    /// `autocomplete_analyzer.tokenizer` key keeps its last value
    /// (`keyword`).
    fn deces_real_settings() -> Value {
        serde_json::json!({
            "analysis": {
                "analyzer": {
                    "autocomplete_analyzer": {
                        "filter": ["asciifolding", "lowercase"],
                        "tokenizer": "keyword"
                    },
                    "norm": {
                        "tokenizer": "standard",
                        "char_filter": ["alphanum"],
                        "filter": ["asciifolding", "lowercase"]
                    }
                },
                "normalizer": {
                    "norm": {
                        "type": "custom",
                        "char_filter": ["alphanum"],
                        "filter": ["asciifolding", "lowercase"]
                    }
                },
                "tokenizer": {
                    "edge_tokenizer2": {
                        "type": "edge_ngram",
                        "min_gram": 2,
                        "max_gram": 10,
                        "token_chars": ["letter", "digit"]
                    }
                },
                "char_filter": {
                    "alphanum": {
                        "type": "pattern_replace",
                        "pattern": "[^\\p{L}\\d]",
                        "replacement": " "
                    }
                }
            }
        })
    }

    fn deces_real_mapping() -> IndexMapping {
        let mut mapping = IndexMapping::from_properties_value(&serde_json::json!({
            "NOM": {"type": "text", "analyzer": "norm",
                    "fields": {"raw": {"type": "keyword", "normalizer": "norm"}}},
            "CODE_INSEE_NAISSANCE": {"type": "keyword", "normalizer": "norm"},
            "SEXE": {"type": "keyword"},
            "DATE_NAISSANCE": {"type": "text",
                "index_prefixes": {"min_chars": 4, "max_chars": 6},
                "fields": {"raw": {"type": "keyword"}}}
        }))
        .expect("deces properties parse");
        mapping.set_analysis(
            IndexMapping::from_index_settings_value(&deces_real_settings())
                .expect("deces settings compile"),
        );
        mapping
    }

    #[test]
    fn deces_settings_compile_every_declared_chain() {
        let analysis = IndexMapping::from_index_settings_value(&deces_real_settings())
            .expect("settings.analysis should compile");
        assert_eq!(
            analysis.analyzer_names().collect::<Vec<_>>(),
            vec!["autocomplete_analyzer", "norm"]
        );
        assert_eq!(
            analysis.normalizer_names().collect::<Vec<_>>(),
            vec!["norm"]
        );
        // The declared JSON round-trips verbatim (persistence, _settings).
        assert_eq!(analysis.to_value(), deces_real_settings()["analysis"]);
        let autocomplete = analysis
            .resolve_analyzer("autocomplete_analyzer")
            .expect("declared");
        assert_eq!(autocomplete.terms("Saint-Étienne"), vec!["saint-etienne"]);
    }

    #[test]
    fn deces_custom_norm_shadows_the_legacy_builtin() {
        let mapping = deces_real_mapping();
        // Custom `norm` analyzer: alphanum char filter splits apostrophes.
        assert_eq!(
            mapping.index_terms("NOM", "D'ARTAGNAN"),
            vec!["d", "artagnan"]
        );
        // Custom `norm` normalizer on the `.raw` sub-field: one term.
        assert_eq!(
            mapping.index_terms("NOM.raw", "Saint-Étienne"),
            vec!["saint etienne"]
        );
        // A `match` on the keyword sub-field analyses the query with the
        // same normalizer.
        assert_eq!(
            mapping.search_terms("NOM.raw", "SAINT-ETIENNE"),
            vec!["saint etienne"]
        );
        // Top-level keyword with a normalizer: index and term query agree.
        assert_eq!(
            mapping.index_terms("CODE_INSEE_NAISSANCE", "2A004"),
            vec!["2a004"]
        );
        assert_eq!(
            mapping.term_query_value("CODE_INSEE_NAISSANCE", "2A004"),
            "2a004"
        );
        // Keyword without normalizer: verbatim.
        assert_eq!(mapping.term_query_value("SEXE", "M"), "M");
    }

    #[test]
    fn deces_dates_use_the_standard_default_and_keep_digits() {
        let mapping = deces_real_mapping();
        assert_eq!(
            mapping.index_terms("DATE_NAISSANCE", "19290107"),
            vec!["19290107"]
        );
        assert_eq!(
            mapping.search_terms("DATE_NAISSANCE", "19290107"),
            vec!["19290107"]
        );
        assert_eq!(
            mapping.search_terms("DATE_NAISSANCE.raw", "19290107"),
            vec!["19290107"]
        );
        assert_eq!(mapping.term_query_value("DATE_NAISSANCE", "1929"), "1929");
    }

    #[test]
    fn analysis_rejects_unsupported_or_dangling_components() {
        let cases = [
            serde_json::json!({"analysis": {"char_filter": {"h": {"type": "html_strip"}},
                "analyzer": {"a": {"tokenizer": "standard", "char_filter": ["h"]}}}}),
            serde_json::json!({"analysis": {"analyzer": {"a": {"tokenizer": "missing"}}}}),
            serde_json::json!({"analysis": {"analyzer": {"a": {"tokenizer": "standard",
                "filter": ["snowball"]}}}}),
            serde_json::json!({"analysis": {"char_filter": {"c": {"type": "pattern_replace",
                "pattern": "(?<=a)b"}}, "normalizer": {"n": {"char_filter": ["c"]}}}}),
            serde_json::json!({"analysis": {"analyzer": {"a": {"filter": ["lowercase"]}}}}),
        ];
        for settings in cases {
            let error =
                IndexMapping::from_index_settings_value(&settings).expect_err("must fail closed");
            assert!(
                matches!(error, MappingError::AnalysisComponent { .. }),
                "{settings}: {error:?}"
            );
        }
        // An unsupported component nobody references is accepted.
        IndexMapping::from_index_settings_value(&serde_json::json!({
            "analysis": {"char_filter": {"h": {"type": "html_strip"}}}
        }))
        .expect("unreferenced components are kept verbatim");
    }

    #[test]
    fn settings_index_analysis_long_form_is_read() {
        let analysis = IndexMapping::from_index_settings_value(&serde_json::json!({
            "index": {"analysis": {"normalizer": {"n": {"filter": ["lowercase"]}}}}
        }))
        .expect("long form parses");
        assert_eq!(analysis.normalizer_names().collect::<Vec<_>>(), vec!["n"]);
    }

    #[test]
    fn es_lowercase_builtin_normalizer_and_default_analyzer() {
        let mut mapping = IndexMapping::from_properties_value(&serde_json::json!({
            "code": {"type": "keyword", "normalizer": "lowercase"},
            "body": {"type": "text"}
        }))
        .expect("parse");
        assert_eq!(mapping.index_terms("code", "AbC-1"), vec!["abc-1"]);
        mapping.set_analysis(
            IndexMapping::from_index_settings_value(&serde_json::json!({
                "analysis": {"analyzer": {"default": {"tokenizer": "whitespace"}}}
            }))
            .expect("parse"),
        );
        // `default` replaces `standard` for text fields declaring no analyzer.
        assert_eq!(
            mapping.index_terms("body", "Jean-Paul X"),
            vec!["Jean-Paul", "X"]
        );
    }

    #[test]
    fn normalizer_is_rejected_on_text_fields() {
        let error = IndexMapping::from_properties_value(&serde_json::json!({
            "NOM": {"type": "text", "normalizer": "norm"}
        }))
        .expect_err("ES rejects a normalizer on text");
        assert!(matches!(error, MappingError::UnsupportedNormalizer { .. }));
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
            Some(ResolvedAnalyzer::LegacyNormalizer),
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
