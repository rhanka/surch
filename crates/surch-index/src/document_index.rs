use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

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
    /// A10 (Phase 4): write-time fan-out of multi-field sub-fields. When a
    /// parent field declares `fields: { <sub>: { … } }` (matchID's
    /// `NOM.raw: { type: keyword, normalizer: norm }`), the parent's source
    /// value is stored here under the qualified `parent.sub` path with the
    /// sub-field's analyzer/normalizer already applied
    /// ([`FieldMapping::effective_analyzer`]).
    ///
    /// Outer key is the qualified field path (`"NOM.raw"`), inner key is the
    /// doc id, value is the normalized token. This is the durable storage
    /// the query side (`sort: NOM.raw`, `agg.cardinality` on `.raw`) reads
    /// directly, instead of aliasing the sub-field path back to the parent's
    /// `_source` and normalizing on read. Sub-field tokens are ALSO indexed
    /// into the regular postings (under the same qualified path) so a
    /// `term`/`match` on the sub-field resolves through the FST like any
    /// other field. This side-table only holds the per-doc projected value
    /// for the read paths that need a stored value rather than postings.
    subfield_values: BTreeMap<String, BTreeMap<u32, String>>,
    /// Track A `wp-a-perf-followups.md` Lot 1.6: deferred-FST-build flag.
    /// `true` when `postings_builder` has accumulated writes that the
    /// `terms` FST does not yet reflect. A subsequent
    /// `materialize_terms()` rebuilds `terms` from the builder and
    /// clears the flag. Reads of `terms`/`postings`/`block_metas` on a
    /// dirty index return a stale snapshot, so the caller
    /// (surch-api `AppState`) is expected to materialize before
    /// exposing the index to a search request — the bulk-then-search
    /// `bulk_router_*` tests guard this contract.
    terms_dirty: bool,
    /// Track A `wp-a-perf-followups.md` Lot 1.6: per-index instrumentation
    /// counter incremented every time `materialize_terms` actually rebuilt
    /// `terms` (i.e. when `terms_dirty` was set). Wrapped in an `Arc` so a
    /// `Clone` of the index keeps observing the same counter — the
    /// `bulk_router_*` test snapshots the counter pre-bulk and asserts
    /// the rebuild count stayed ~constant across many chunks.
    terms_build_count: Arc<AtomicU64>,
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
        self.add_documents_with_mapping_internal(documents, mapping, /* defer */ false)
    }

    /// Track A `wp-a-perf-followups.md` Lot 1.6: bulk-path variant of
    /// [`add_documents_with_mapping`] that does NOT rebuild the FST
    /// term dictionary. The caller is responsible for invoking
    /// [`materialize_terms`] before any read of `terms`/`postings`
    /// that requires post-write visibility.
    ///
    /// Used by the bulk write path
    /// (`surch-api::AppState::apply_document_writes` →
    /// `IndexData::append_to_index` / `rebuild_index`) so 21
    /// consecutive `_bulk` POSTs only trigger one cumulative rebuild
    /// (at the next `_refresh` or first search) instead of 21
    /// quadratic per-chunk rebuilds. The single-doc and integration
    /// tests keep using the materializing variant so the existing
    /// `index.terms()` / `index.postings()` call sites stay correct
    /// without a materialize call.
    pub fn add_documents_with_mapping_deferred<D, I, K, V>(
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
        self.add_documents_with_mapping_internal(documents, mapping, /* defer */ true)
    }

    fn add_documents_with_mapping_internal<D, I, K, V>(
        &mut self,
        documents: D,
        mapping: &IndexMapping,
        defer_terms_build: bool,
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

        if defer_terms_build {
            // Lot 1.6: defer the FST rebuild. Reads of `terms` /
            // `postings` see the pre-write snapshot until the caller
            // invokes `materialize_terms()` (e.g. at refresh time, or
            // right before exposing the index to a search request).
            // The bulk path used to call
            // `self.postings_builder.clone().build()` here, which
            // rebuilt the entire FST from cumulative postings on
            // every chunk — O(total_terms_so_far) per `_bulk` POST.
            self.terms_dirty = true;
        } else {
            // Legacy path: materialize immediately so direct callers
            // (single-doc paths and unit tests) can read `terms` /
            // `postings` without an explicit `materialize_terms()`.
            self.terms = self.postings_builder.clone().build();
            self.terms_dirty = false;
            self.terms_build_count.fetch_add(1, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Track A `wp-a-perf-followups.md` Lot 1.6: rebuild the term
    /// dictionary FST from the live `PostingsBuilder` snapshot if and
    /// only if writes have happened since the last rebuild. No-op when
    /// the index is clean (idempotent), so callers can call it
    /// liberally at the top of any read path without paying the
    /// rebuild cost on a quiet index.
    ///
    /// Must be called before any read of `terms`/`postings`/`block_metas`
    /// that requires post-write visibility. The bulk write path in
    /// `surch-api::AppState::apply_document_writes` skips this call so
    /// 21 consecutive `_bulk` POSTs only trigger one cumulative
    /// rebuild — either at `_refresh` (via `finalize_postings`'s
    /// preamble) or at the first search after the writes.
    pub fn materialize_terms(&mut self) {
        if !self.terms_dirty {
            return;
        }
        self.terms = self.postings_builder.clone().build();
        self.terms_dirty = false;
        self.terms_build_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Lot 1.6: whether `terms` reflects every write seen so far. A
    /// `true` return means a `materialize_terms()` call is required
    /// before any read that depends on the post-write FST state.
    pub fn terms_dirty(&self) -> bool {
        self.terms_dirty
    }

    /// Lot 1.6: number of times `materialize_terms` actually rebuilt
    /// the FST on this index instance. Used by the `bulk_router_*`
    /// tests to prove that N `_bulk` chunks no longer trigger N FST
    /// rebuilds. Cheap atomic load; not on any hot path.
    pub fn terms_build_count(&self) -> u64 {
        self.terms_build_count.load(Ordering::Relaxed)
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
    ///
    /// Lot 1.6: callers MUST `materialize_terms()` first so the FST
    /// reflects every pending write before the builder is dropped.
    /// Otherwise the deferred writes are lost.
    pub fn finalize_postings(&mut self) {
        debug_assert!(
            !self.terms_dirty,
            "finalize_postings called while terms_dirty=true — caller must materialize_terms() first \
             to avoid losing pending writes"
        );
        self.postings_builder = PostingsBuilder::new();
    }

    pub fn clear(&mut self) {
        self.live_docs.clear();
        self.postings_builder = PostingsBuilder::new();
        self.terms = TermDictionary::default();
        self.field_stats.clear();
        self.prefix_postings.clear();
        self.subfield_values.clear();
        // The fresh `TermDictionary::default()` is in sync with the
        // fresh `PostingsBuilder::new()` (both empty), so the index is
        // clean as far as the deferred-rebuild contract is concerned.
        // Keep the per-index counter (an `Arc<AtomicU64>`) untouched
        // so cumulative diagnostics across rebuilds remain coherent.
        self.terms_dirty = false;
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

    /// A6 phase 3: union of doc ids across every term of `field` whose
    /// bytes start with `prefix`. Delegates to
    /// [`TermDictionary::prefix_doc_ids`]; see that method for the cost
    /// model. The keyword-prefix iterator uses this on fields that did
    /// not declare `index_prefixes` (e.g. matchID's `DATE_NAISSANCE`).
    pub fn term_prefix_doc_ids(&self, field: &str, prefix: &str) -> BTreeSet<u32> {
        self.terms.prefix_doc_ids(field, prefix)
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

            // A10 (Phase 4): write-time fan-out of declared multi-field
            // sub-fields. The parent's source value is re-analyzed with each
            // sub-field's own analyzer/normalizer and indexed under the
            // qualified `parent.sub` path (both as postings and as a stored
            // projected value), so sort / agg / composite read the sub-field
            // directly instead of aliasing back to the parent at read time.
            if let Some(field_mapping) = field_mapping {
                if field_mapping.has_subfields() {
                    self.index_subfields(doc_id, field, value, field_mapping)?;
                }
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
        let entry = self.prefix_postings.entry(field.to_owned()).or_default();
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

    /// A10 (Phase 4): index the parent `value` under each declared sub-field
    /// of `parent_mapping`.
    ///
    /// For every `parent.sub` sub-field, the parent's raw source value is
    /// re-analyzed with the sub-field's
    /// [`effective_analyzer`](crate::mapping::FieldMapping::effective_analyzer)
    /// — the `normalizer` for a `keyword` sub-field carrying one (so
    /// `NOM.raw` stores the lowercased / asciifolded value as a single
    /// keyword token), the declared analyzer for a `text` sub-field. The
    /// resulting tokens are added to the regular postings under the
    /// qualified path so `term`/`match` on the sub-field resolves through
    /// the FST, and the first (lowest-position) token is also recorded in
    /// `subfield_values` as the per-doc stored projection that sort / agg
    /// read directly.
    fn index_subfields(
        &mut self,
        doc_id: u32,
        parent: &str,
        value: &str,
        parent_mapping: &crate::mapping::FieldMapping,
    ) -> Result<()> {
        for (sub_name, sub_mapping) in parent_mapping.subfields() {
            let path = format!("{parent}.{sub_name}");
            let analyzer = sub_mapping.effective_analyzer();
            let analyzed = analyzed_terms(analyzer, &path, value);

            // Record the stored projection: the term at the lowest position
            // (the whole normalized value for a keyword/normalizer sub-field,
            // the first token for an analyzed text sub-field). Mirrors the
            // single value matchID's sort / agg expect on `NOM.raw`.
            if let Some(stored) = lowest_position_term(&analyzed) {
                self.subfield_values
                    .entry(path.clone())
                    .or_default()
                    .insert(doc_id, stored);
            }

            for ((field, term), positions) in analyzed {
                self.postings_builder.add(field, term, doc_id, positions)?;
            }
        }
        Ok(())
    }

    /// A10 (Phase 4): stored projected value for `(field_path, doc_id)`.
    ///
    /// Returns `Some(&str)` when `field_path` is a declared multi-field
    /// sub-field (`parent.sub`) that was fanned out at write time for this
    /// doc, with the sub-field's analyzer/normalizer already applied. The
    /// query side uses this for `sort`/`agg`/`composite` on `.raw`/`.norm`
    /// without re-normalizing the parent's `_source` on read. Returns `None`
    /// for top-level fields and for docs missing the sub-field value.
    pub fn subfield_value(&self, field_path: &str, doc_id: u32) -> Option<&str> {
        self.subfield_values
            .get(field_path)?
            .get(&doc_id)
            .map(String::as_str)
    }

    /// A10 (Phase 4): whether `field_path` carries write-time fanned-out
    /// sub-field projections. Used by the query side to choose between the
    /// stored sub-field and the legacy `lookup_sort_value` parent alias.
    pub fn has_subfield_values(&self, field_path: &str) -> bool {
        self.subfield_values.contains_key(field_path)
    }

    /// A10 (Phase 4): the full per-doc stored sub-field projection map.
    /// Empty when no field in the mapping declared sub-fields. Exposed for
    /// memory accounting and for the query side to enumerate projections.
    pub fn subfield_values_map(&self) -> &BTreeMap<String, BTreeMap<u32, String>> {
        &self.subfield_values
    }
}

/// A10 (Phase 4): the analyzed term carrying the lowest position increment,
/// i.e. the first token of the analyzed stream. For a keyword/normalizer
/// sub-field this is the whole normalized value (single token); for a text
/// sub-field it is the leading token. `None` when the value analyzed to no
/// tokens (empty / whitespace-only source).
fn lowest_position_term(analyzed: &BTreeMap<(String, String), Vec<u32>>) -> Option<String> {
    analyzed
        .iter()
        .filter_map(|((_, term), positions)| positions.iter().min().map(|pos| (*pos, term.clone())))
        .min_by_key(|(pos, _)| *pos)
        .map(|(_, term)| term)
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
        let field_mapping =
            FieldMapping::new(FieldType::Text, None).with_index_prefixes(Some(prefixes));
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
