use std::collections::{BTreeMap, BTreeSet};

use surch_analysis::{
    Analyzer, KeywordAnalyzer, NormAnalyzer, SimpleAnalyzer, StandardAnalyzer, StopAnalyzer,
    WhitespaceAnalyzer,
};
use thiserror::Error;

use crate::mapping::{AnalyzerName, FieldPrefixes, FieldType, IndexMapping};
use crate::postings::{
    BlockMeta, PostingsBuilder, PostingsEnum, PostingsError, TermDictionary, TermsEnum,
};
use crate::stored_fields::{StoredDocument, StoredFieldsError};

pub type Result<T> = std::result::Result<T, DocumentIndexError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DocumentIndexError {
    #[error("duplicate doc id: {doc_id}")]
    DuplicateDocId { doc_id: u32 },
    #[error("document field name must not be empty")]
    EmptyFieldName,
    #[error(transparent)]
    StoredFields(#[from] StoredFieldsError),
    #[error(transparent)]
    Postings(#[from] PostingsError),
}

#[derive(Debug, Default, Clone)]
pub struct DocumentIndex {
    /// Live document ids in this generation. The full source is held by
    /// the caller (surch-api `InMemoryIndex`) so this is just a presence
    /// set, not a copy of `StoredDocument`.
    live_docs: BTreeSet<u32>,
    postings_builder: PostingsBuilder,
    terms: TermDictionary,
    field_stats: BTreeMap<String, FieldLengthStats>,
    /// A6 phase 2: per-field write-time prefix expansion. Populated only for
    /// fields whose `FieldMapping::index_prefixes` is `Some(_)`. The inner map
    /// is keyed by the normalized prefix (length in `[min_chars..=max_chars]`)
    /// and the value is the set of doc ids that contain at least one token
    /// starting with that prefix. Kept separate from the regular postings so
    /// the BM25 hot path (`doc_freq`, `term_freq`, norms) is unaffected.
    prefix_postings: BTreeMap<String, BTreeMap<String, BTreeSet<u32>>>,
}

/// Per-field length statistics recorded during analysis for BM25 norms.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FieldLengthStats {
    pub doc_count: u64,
    pub total_terms: u64,
    pub doc_len_by_doc_id: BTreeMap<u32, u64>,
}

impl FieldLengthStats {
    fn record_doc_len(&mut self, doc_id: u32, doc_len: u64, norms_enabled: bool) {
        if doc_len == 0 {
            return;
        }
        if !norms_enabled {
            self.doc_count += 1;
            return;
        }
        if let Some(previous) = self.doc_len_by_doc_id.insert(doc_id, doc_len) {
            self.total_terms -= previous;
        } else {
            self.doc_count += 1;
        }
        self.total_terms += doc_len;
    }

    pub fn avg_doc_len(&self) -> Option<f64> {
        (self.doc_count > 0 && self.total_terms > 0)
            .then(|| self.total_terms as f64 / self.doc_count as f64)
    }

    pub fn doc_len(&self, doc_id: u32) -> Option<u64> {
        self.doc_len_by_doc_id.get(&doc_id).copied()
    }
}

impl DocumentIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_document<I, K, V>(&mut self, doc_id: u32, fields: I) -> Result<()>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.add_document_with_mapping(doc_id, fields, &IndexMapping::default())
    }

    pub fn add_document_with_mapping<I, K, V>(
        &mut self,
        doc_id: u32,
        fields: I,
        mapping: &IndexMapping,
    ) -> Result<()>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.add_documents_with_mapping([(doc_id, fields)], mapping)
    }

    pub fn add_documents_with_mapping<D, I, K, V>(
        &mut self,
        documents: D,
        mapping: &IndexMapping,
    ) -> Result<()>
    where
        D: IntoIterator<Item = (u32, I)>,
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut seen = BTreeSet::new();
        let mut documents = documents
            .into_iter()
            .map(|(doc_id, fields)| {
                if self.live_docs.contains(&doc_id) || !seen.insert(doc_id) {
                    return Err(DocumentIndexError::DuplicateDocId { doc_id });
                }

                let fields = fields
                    .into_iter()
                    .map(|(name, value)| (name.into(), value.into()))
                    .collect::<Vec<_>>();

                for (name, _) in &fields {
                    if name.trim().is_empty() {
                        return Err(DocumentIndexError::EmptyFieldName);
                    }
                }

                Ok((doc_id, fields))
            })
            .collect::<Result<Vec<_>>>()?;

        for (doc_id, fields) in documents.drain(..) {
            self.add_validated_document(doc_id, fields, mapping)?;
        }

        self.terms = self.postings_builder.clone().build();

        Ok(())
    }

    /// Drop the internal `PostingsBuilder` state once the caller is done
    /// adding documents in the current generation. The builder holds a
    /// duplicate of every indexed posting (~half of the index RAM on BAN
    /// Paris 25k), so freeing it after a batch is a big memory win.
    ///
    /// After this call any further `add_document*` call starts a fresh
    /// builder, and the previously indexed postings remain accessible via
    /// `terms()` / `postings()`. Mixing `finalize_postings` with
    /// incremental adds therefore breaks the snapshot semantics — only
    /// callers that follow a `clear()` + batch + finalize lifecycle
    /// should use it.
    pub fn finalize_postings(&mut self) {
        self.postings_builder = PostingsBuilder::new();
    }

    pub fn clear(&mut self) {
        self.live_docs.clear();
        self.postings_builder = PostingsBuilder::new();
        self.terms = TermDictionary::default();
        self.field_stats.clear();
        self.prefix_postings.clear();
    }

    pub fn doc_ids(&self) -> Vec<u32> {
        self.live_docs.iter().copied().collect()
    }

    /// Stored field retrieval is the caller's responsibility (sources live
    /// in `surch-api::AppState`); this method only returns the previously
    /// indexed analyzed fields when a stored-fields writer has been wired
    /// in, which is not the in-memory path. Always returns `None` for the
    /// current `DocumentIndex` layout.
    pub fn stored_document(&self, _doc_id: u32) -> Option<&StoredDocument> {
        None
    }

    pub fn terms(&self, field: &str) -> TermsEnum {
        self.terms.terms(field)
    }

    pub fn postings(&self, field: &str, term: &str) -> Option<PostingsEnum<'_>> {
        self.terms.postings(field, term)
    }

    /// Returns the pre-computed per-block stats for `(field, term)`,
    /// aligned with [`postings`] chunks of 128 entries. See
    /// [`crate::postings::BlockMeta`] for the schema.
    pub fn block_metas(&self, field: &str, term: &str) -> Option<&[BlockMeta]> {
        self.terms.block_metas(field, term)
    }

    pub fn field_stats(&self, field: &str) -> Option<&FieldLengthStats> {
        self.field_stats.get(field)
    }

    /// Returns the in-memory `field -> FieldLengthStats` map. Used by the
    /// memory accounting helper (`crate::memory`) to size the BM25 norms
    /// payload without exposing the underlying `BTreeMap` everywhere.
    pub fn field_stats_map(&self) -> &BTreeMap<String, FieldLengthStats> {
        &self.field_stats
    }

    /// Returns the names of every field that currently has indexed
    /// postings, in lexicographic order. Used by the memory accounting
    /// helper to enumerate every `(field, term)` pair.
    pub fn field_names(&self) -> Vec<String> {
        self.terms.field_names()
    }

    /// Returns the in-memory prefix-postings side table. Empty for fields
    /// that did not declare `index_prefixes`. Used by the memory
    /// accounting helper.
    pub fn prefix_postings_map(&self) -> &BTreeMap<String, BTreeMap<String, BTreeSet<u32>>> {
        &self.prefix_postings
    }

    /// A6 phase 2: lookup the write-time prefix postings for `(field, prefix)`.
    ///
    /// Returns `Some(&BTreeSet<u32>)` iff `field` was indexed with
    /// `index_prefixes` AND `prefix` has a length (in chars) within
    /// `[min_chars..=max_chars]` of that mapping. Callers that fall outside
    /// the bounds must fall back to the source-scan path.
    pub fn prefix_postings(&self, field: &str, prefix: &str) -> Option<&BTreeSet<u32>> {
        self.prefix_postings.get(field)?.get(prefix)
    }

    /// Whether `field` carries an `index_prefixes` write-time postings list.
    /// Used by the query path to decide between the postings-backed lookup
    /// and the source-scan fallback.
    pub fn field_has_prefix_postings(&self, field: &str) -> bool {
        self.prefix_postings.contains_key(field)
    }

    pub fn live_doc_count(&self) -> usize {
        self.live_docs.len()
    }

    pub fn live_docs(&self) -> Vec<u32> {
        self.doc_ids()
    }

    fn add_validated_document(
        &mut self,
        doc_id: u32,
        fields: Vec<(String, String)>,
        mapping: &IndexMapping,
    ) -> Result<()> {
        let mut field_lengths = BTreeMap::<String, (u64, bool)>::new();
        for (field, value) in &fields {
            let field_mapping = mapping.field(field);
            let analyzer = field_mapping
                .map_or(AnalyzerName::default_for(FieldType::Text), |field| {
                    field.analyzer()
                });
            let norms_enabled = field_mapping.is_none_or(|field| field.norms_enabled());
            let index_prefixes = field_mapping.and_then(|m| m.index_prefixes);

            let analyzed_terms = analyzed_terms(analyzer, field, value);
            let token_count = analyzed_terms
                .values()
                .map(|positions| positions.len() as u64)
                .sum::<u64>();
            if token_count > 0 {
                let (doc_len, field_norms_enabled) = field_lengths
                    .entry(field.clone())
                    .or_insert((0, norms_enabled));
                *doc_len += token_count;
                *field_norms_enabled = *field_norms_enabled || norms_enabled;
            }

            // A6 phase 2: fan-out each analyzed token into its [min..=max]
            // length prefixes, stored in a side-table so BM25 stays unaffected.
            if let Some(prefixes) = index_prefixes {
                self.index_prefix_terms(doc_id, field, &analyzed_terms, prefixes);
            }

            for ((field, term), positions) in analyzed_terms {
                self.postings_builder.add(field, term, doc_id, positions)?;
            }
        }

        for (field, (doc_len, norms_enabled)) in field_lengths {
            self.field_stats.entry(field).or_default().record_doc_len(
                doc_id,
                doc_len,
                norms_enabled,
            );
        }

        self.live_docs.insert(doc_id);
        Ok(())
    }

    /// A6 phase 2: for each analyzed token of `field`, generate the prefixes
    /// of length `k` for every `k` in `[prefixes.min_chars..=prefixes.max_chars]`
    /// and record `doc_id` under each prefix in `prefix_postings`.
    ///
    /// Operates on `char` boundaries so multibyte UTF-8 (`É`, etc.) is sliced
    /// safely. Tokens shorter than `min_chars` contribute no entries; tokens
    /// longer than `max_chars` only contribute prefixes up to `max_chars`
    /// (ES `index_prefixes` semantics, see `MapperBuilder` in Lucene).
    fn index_prefix_terms(
        &mut self,
        doc_id: u32,
        field: &str,
        analyzed_terms: &BTreeMap<(String, String), Vec<u32>>,
        prefixes: FieldPrefixes,
    ) {
        let entry = self
            .prefix_postings
            .entry(field.to_owned())
            .or_default();
        for (_field, term) in analyzed_terms.keys() {
            let chars: Vec<char> = term.chars().collect();
            let token_len = chars.len();
            if token_len < prefixes.min_chars {
                continue;
            }
            let upper = token_len.min(prefixes.max_chars);
            for k in prefixes.min_chars..=upper {
                let prefix: String = chars[..k].iter().collect();
                entry.entry(prefix).or_default().insert(doc_id);
            }
        }
    }
}

fn analyzed_terms(
    analyzer: AnalyzerName,
    field: &str,
    value: &str,
) -> BTreeMap<(String, String), Vec<u32>> {
    let tokenized = match analyzer {
        AnalyzerName::Standard => StandardAnalyzer.token_stream(value),
        AnalyzerName::Simple => SimpleAnalyzer.token_stream(value),
        AnalyzerName::Stop => StopAnalyzer.token_stream(value),
        AnalyzerName::Keyword => KeywordAnalyzer.token_stream(value),
        AnalyzerName::Whitespace => WhitespaceAnalyzer.token_stream(value),
        AnalyzerName::Norm => NormAnalyzer.token_stream(value),
    };

    let mut terms = BTreeMap::<(String, String), Vec<u32>>::new();
    let mut position = 0_u32;

    for token in tokenized {
        position += token.position_increment;
        terms
            .entry((field.to_owned(), token.term))
            .or_default()
            .push(position - 1);
    }

    terms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{FieldMapping, FieldPrefixes, FieldType};

    fn mapping_with_prefixes(field: &str, prefixes: FieldPrefixes) -> IndexMapping {
        let mut mapping = IndexMapping::new();
        let field_mapping = FieldMapping::new(FieldType::Text, None)
            .with_index_prefixes(Some(prefixes));
        mapping.set_field_mapping(field, field_mapping);
        mapping
    }

    #[test]
    fn prefix_postings_populated_for_indexed_prefixes_field() {
        // ES default: min=2, max=5. Each analyzed token contributes prefixes
        // of length 2..=min(len, 5).
        let mapping = mapping_with_prefixes(
            "name",
            FieldPrefixes {
                min_chars: 2,
                max_chars: 5,
            },
        );
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("name", "DUPONT")], &mapping)
            .expect("doc 1");
        index
            .add_document_with_mapping(2, [("name", "DUPRE")], &mapping)
            .expect("doc 2");
        index
            .add_document_with_mapping(3, [("name", "MARTIN")], &mapping)
            .expect("doc 3");

        // "DUP" (3 chars, in [2..=5]): docs 1 and 2.
        let hits = index
            .prefix_postings("name", "dup")
            .expect("prefix postings present for dup");
        assert_eq!(hits.iter().copied().collect::<Vec<_>>(), vec![1, 2]);

        // "MAR" (3 chars, in [2..=5]): doc 3.
        let hits = index
            .prefix_postings("name", "mar")
            .expect("prefix postings present for mar");
        assert_eq!(hits.iter().copied().collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn prefix_postings_clipped_at_max_chars() {
        // Prefix at exactly `max_chars` is recorded; longer prefixes are not.
        let mapping = mapping_with_prefixes(
            "name",
            FieldPrefixes {
                min_chars: 2,
                max_chars: 4,
            },
        );
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("name", "DUPONT")], &mapping)
            .expect("doc 1");

        assert!(index.prefix_postings("name", "dupo").is_some());
        // 5 chars > max_chars=4: not recorded; caller must fall back to scan.
        assert!(index.prefix_postings("name", "dupon").is_none());
    }

    #[test]
    fn prefix_postings_absent_when_field_lacks_index_prefixes() {
        // No `index_prefixes` on field => no prefix postings table populated.
        let mapping = IndexMapping::new();
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("name", "DUPONT")], &mapping)
            .expect("doc 1");
        assert!(!index.field_has_prefix_postings("name"));
        assert!(index.prefix_postings("name", "dup").is_none());
    }
}
