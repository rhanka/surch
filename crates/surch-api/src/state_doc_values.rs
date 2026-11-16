//! MVP L1c: doc-values support for aggregations served without `_source`.
//!
//! Aggregations read, per matched document, the field value as
//! [`crate::search`]'s `FieldValueView` exposes it: the stored multi-field
//! sub-field projection (`NOM.raw`), else the `_source` value passed
//! through the field's keyword normalizer. Serving them from the index
//! instead (term dictionary x postings, sub-field columns) is exact only
//! when every indexed value of the field maps to exactly one index term
//! that rebuilds the same JSON value. [`FieldValueKinds`] records, at index
//! time, which JSON kinds each top-level scalar field carried, so the
//! query side can prove that per field (and otherwise keep the `_source`
//! path).

use std::collections::BTreeMap;

use serde_json::Value;
use surch_index::mapping::{FieldType, IndexMapping, ResolvedAnalyzer};

/// Every value was a JSON string indexed as exactly one term equal to the
/// value the aggregation reads (normalised for a keyword with normalizer,
/// verbatim otherwise).
pub(crate) const KIND_STRING: u8 = 1;
/// Every value was a JSON integer (`|n| <= 2^53`) indexed as exactly one
/// term equal to its decimal text.
pub(crate) const KIND_INTEGER: u8 = 2;
/// Anything else: array, object, boolean, float, a string analysed to zero
/// or several terms, ...
pub(crate) const KIND_OTHER: u8 = 4;

/// Largest integer magnitude a `terms` key keeps distinct: keys compare as
/// `f64` (`TermsKey::Number`), so two integers beyond `2^53` may collide.
const MAX_EXACT_INTEGER: u64 = 1 << 53;

/// How the values of one field rebuild from its index terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocValueKind {
    /// No document carries a non-null value.
    NoValue,
    /// Each term is the JSON string value.
    String,
    /// Each term is the decimal text of the JSON integer value.
    Integer,
}

/// Per top-level field, the union of the value kinds of every indexed
/// document (MVP L1c). Recomputed from scratch by a full rebuild, extended
/// by appends, persisted in the index `META` file (format 3).
#[derive(Debug, Clone, Default)]
pub(crate) struct FieldValueKinds {
    /// `true` when the index was reopened from a data directory written
    /// before the kinds existed (`META` format 2): nothing is known until
    /// the next full rebuild.
    unknown: bool,
    by_field: BTreeMap<String, u8>,
}

impl FieldValueKinds {
    pub(crate) fn unknown() -> Self {
        Self {
            unknown: true,
            by_field: BTreeMap::new(),
        }
    }

    pub(crate) fn from_parts(by_field: BTreeMap<String, u8>) -> Self {
        Self {
            unknown: false,
            by_field,
        }
    }

    pub(crate) fn is_unknown(&self) -> bool {
        self.unknown
    }

    pub(crate) fn by_field(&self) -> &BTreeMap<String, u8> {
        &self.by_field
    }

    /// OR the per-document bits produced by `tracker` into the table.
    pub(crate) fn merge(&mut self, tracker: &KindTracker, bits: &[u8]) {
        for ((field, _, _), &bit) in tracker.fields.iter().zip(bits) {
            if bit != 0 {
                *self.by_field.entry((*field).to_owned()).or_insert(0) |= bit;
            }
        }
    }

    /// The kind of `field`'s values when they rebuild exactly from the
    /// index terms; `None` when unknown or mixed.
    pub(crate) fn kind(&self, field: &str) -> Option<DocValueKind> {
        if self.unknown {
            return None;
        }
        match self.by_field.get(field).copied().unwrap_or(0) {
            0 => Some(DocValueKind::NoValue),
            KIND_STRING => Some(DocValueKind::String),
            KIND_INTEGER => Some(DocValueKind::Integer),
            _ => None,
        }
    }
}

/// Whether a mapped top-level field type is indexed as one verbatim /
/// normalised term per value (the types a `terms` / `date_histogram` /
/// `composite` / `cardinality` aggregation reads as doc values).
pub(crate) fn is_doc_value_type(field_type: FieldType) -> bool {
    matches!(
        field_type,
        FieldType::Keyword
            | FieldType::Integer
            | FieldType::Long
            | FieldType::Short
            | FieldType::Byte
            | FieldType::Float
            | FieldType::Double
            | FieldType::Boolean
            | FieldType::Date
    )
}

/// The fields of one mapping whose value kinds are tracked, with the index
/// analyzer each one is indexed with. Built once per indexing batch.
pub(crate) struct KindTracker<'m> {
    /// `(field, index analyzer, keyword normalizer applies on read)`.
    fields: Vec<(&'m str, ResolvedAnalyzer, bool)>,
}

impl<'m> KindTracker<'m> {
    pub(crate) fn new(mapping: &'m IndexMapping) -> Self {
        let fields = mapping
            .fields()
            .filter(|(_, field)| is_doc_value_type(field.field_type))
            .map(|(name, field)| {
                let normalized =
                    field.field_type == FieldType::Keyword && field.normalizer_name().is_some();
                (name, field.index_analyzer(mapping.analysis()), normalized)
            })
            .collect();
        Self { fields }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// One bit set per tracked field for `source` (aligned with the
    /// tracker's fields).
    pub(crate) fn classify(&self, source: &Value) -> Vec<u8> {
        let Some(object) = source.as_object() else {
            return vec![0; self.fields.len()];
        };
        self.fields
            .iter()
            .map(|(name, analyzer, normalized)| match object.get(*name) {
                None | Some(Value::Null) => 0,
                Some(Value::String(text)) => {
                    let terms = analyzer.terms(text);
                    // A blank term is never indexed (`PostingsBuilder::add`
                    // rejects `term.trim().is_empty()`; ES indexes `""`), so a
                    // value normalised to blanks has no term to read.
                    let indexed = terms.len() == 1 && !terms[0].trim().is_empty();
                    if indexed && (*normalized || terms[0] == *text) {
                        KIND_STRING
                    } else {
                        KIND_OTHER
                    }
                }
                Some(Value::Number(number)) => {
                    let exact = number
                        .as_u64()
                        .map(|n| n <= MAX_EXACT_INTEGER)
                        .or_else(|| {
                            number
                                .as_i64()
                                .map(|n| n.unsigned_abs() <= MAX_EXACT_INTEGER)
                        })
                        .unwrap_or(false);
                    if !exact {
                        return KIND_OTHER;
                    }
                    let text = number.to_string();
                    let terms = analyzer.terms(&text);
                    if terms.len() == 1 && terms[0] == text {
                        KIND_INTEGER
                    } else {
                        KIND_OTHER
                    }
                }
                Some(_) => KIND_OTHER,
            })
            .collect()
    }
}

/// Rebuild the JSON value an aggregation reads from one index term of a
/// field of `kind`. `None` when the term does not parse back (never for a
/// term written under that kind).
pub(crate) fn value_from_term(term: &str, kind: DocValueKind) -> Option<Value> {
    match kind {
        DocValueKind::String => Some(Value::String(term.to_owned())),
        DocValueKind::Integer => serde_json::from_str::<serde_json::Number>(term)
            .ok()
            .filter(|number| number.is_i64() || number.is_u64())
            .map(Value::Number),
        DocValueKind::NoValue => None,
    }
}

/// A dense set of internal doc ids `[0, len)`.
#[derive(Clone, Debug, Default)]
pub struct DocBitSet {
    words: Vec<u64>,
    len: u32,
    count: u64,
}

impl DocBitSet {
    pub fn with_len(len: u32) -> Self {
        Self {
            words: vec![0; (len as usize).div_ceil(64)],
            len,
            count: 0,
        }
    }

    /// Number of slots (`next_doc_id` of the index).
    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Number of members.
    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn insert(&mut self, doc: u32) {
        if doc >= self.len {
            return;
        }
        let word = &mut self.words[(doc / 64) as usize];
        let bit = 1u64 << (doc % 64);
        if *word & bit == 0 {
            *word |= bit;
            self.count += 1;
        }
    }

    #[inline]
    pub fn contains(&self, doc: u32) -> bool {
        doc < self.len && self.words[(doc / 64) as usize] & (1u64 << (doc % 64)) != 0
    }

    /// Whether any member lies in `[start, end)`.
    pub fn any_in(&self, start: u32, end: u32) -> bool {
        let end = end.min(self.len);
        if start >= end {
            return false;
        }
        let (first, last) = ((start / 64) as usize, ((end - 1) / 64) as usize);
        (first..=last).any(|index| {
            let mut word = self.words[index];
            if index == first {
                word &= u64::MAX << (start % 64);
            }
            if index == last {
                let top = (end - 1) % 64;
                if top < 63 {
                    word &= (1u64 << (top + 1)) - 1;
                }
            }
            word != 0
        })
    }

    /// Members in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.words.iter().enumerate().flat_map(|(index, &word)| {
            let base = (index as u32) * 64;
            let mut rest = word;
            std::iter::from_fn(move || {
                if rest == 0 {
                    return None;
                }
                let bit = rest.trailing_zeros();
                rest &= rest - 1;
                Some(base + bit)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bitset_ranges_and_iteration() {
        let mut set = DocBitSet::with_len(200);
        for doc in [0, 63, 64, 130, 199, 250] {
            set.insert(doc);
        }
        set.insert(64);
        assert_eq!(set.count(), 5);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![0, 63, 64, 130, 199]);
        assert!(set.any_in(60, 64));
        assert!(!set.any_in(1, 63));
        assert!(set.any_in(65, 131));
        assert!(!set.any_in(131, 199));
        assert!(set.any_in(131, 300));
        assert!(!set.contains(250));
    }

    #[test]
    fn kinds_classify_strings_integers_and_the_rest() {
        let mapping = IndexMapping::from_properties_value(&json!({
            "SEXE": {"type": "keyword"},
            "AGE": {"type": "short"},
            "TITLE": {"type": "text"},
        }))
        .expect("mapping");
        let tracker = KindTracker::new(&mapping);
        let mut kinds = FieldValueKinds::default();
        for source in [
            json!({"SEXE": "M", "AGE": 85, "TITLE": "a b"}),
            json!({"SEXE": null, "AGE": 7}),
        ] {
            kinds.merge(&tracker, &tracker.classify(&source));
        }
        assert_eq!(kinds.kind("SEXE"), Some(DocValueKind::String));
        assert_eq!(kinds.kind("AGE"), Some(DocValueKind::Integer));
        assert_eq!(kinds.kind("TITLE"), Some(DocValueKind::NoValue));
        for (source, field) in [
            (json!({"SEXE": ""}), "SEXE"),
            (json!({"SEXE": ["M"]}), "SEXE"),
            (json!({"SEXE": 1}), "SEXE"),
            (json!({"AGE": 8.5}), "AGE"),
            (json!({"AGE": "85"}), "AGE"),
        ] {
            let mut kinds = FieldValueKinds::default();
            kinds.merge(&tracker, &tracker.classify(&json!({"SEXE": "F", "AGE": 1})));
            kinds.merge(&tracker, &tracker.classify(&source));
            assert_eq!(kinds.kind(field), None, "{source}");
        }
        assert_eq!(FieldValueKinds::unknown().kind("SEXE"), None);
        assert_eq!(
            value_from_term("85", DocValueKind::Integer),
            Some(json!(85))
        );
        assert_eq!(value_from_term("8.5", DocValueKind::Integer), None);
    }
}
