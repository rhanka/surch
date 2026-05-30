use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{Arc, RwLock},
};

use serde_json::Value;
use surch_index::{
    document_index::DocumentIndex,
    mapping::{AnalysisSettings, FieldMapping, FieldType, IndexMapping},
    memory::{document_index_memory_usage, stored_fields_bytes_for, MemoryUsage},
    postings::{BlockMeta, Posting, PostingsBlockSkipIter, PostingsList},
};

use crate::scroll::ScrollTable;
use crate::stats::{clear_memory_gauges, refresh_memory_gauges};

/// Shared in-memory API state used by API handlers.
#[derive(Clone, Default)]
pub struct AppState {
    store: Arc<RwLock<MemoryStore>>,
    search_cache: Arc<RwLock<BTreeMap<String, IndexSearchCache>>>,
    /// Server-side state backing `_search?scroll=…` and
    /// `POST /_search/scroll`. Shared across handlers; lazy GC.
    pub scroll_table: Arc<ScrollTable>,
}

const SEARCH_CACHE_CAPACITY: usize = 256;

#[derive(Default)]
struct IndexSearchCache {
    entries: HashMap<u64, Vec<u8>>,
    order: VecDeque<u64>,
}

#[derive(Default)]
struct MemoryStore {
    indices: BTreeMap<String, InMemoryIndex>,
    aliases: BTreeMap<String, BTreeMap<String, Value>>,
    component_templates: BTreeMap<String, StoredComponentTemplate>,
    index_templates: BTreeMap<String, StoredIndexTemplate>,
}

#[derive(Debug, Default, Clone)]
struct InMemoryIndex {
    /// `_source` payloads, refcounted so the search hot path
    /// (`build_hit`, `score_documents`, `lookup_sort_value`, …) can
    /// hand each reader a fresh [`StoredDocument`] without cloning
    /// the entire JSON. Multiple concurrent reads on the same doc
    /// share the same `Arc<Value>`; writes always allocate a fresh
    /// `Arc` so an in-flight reader's snapshot stays untouched. The
    /// Prometheus gauge `surch_index_stored_fields_bytes` keeps
    /// counting the `Value` payload size once (regardless of the
    /// strong count), so the gauge tracks unique stored bytes —
    /// which is what capacity planning cares about.
    documents: BTreeMap<String, Arc<Value>>,
    document_ids: BTreeMap<String, u32>,
    reverse_document_ids: BTreeMap<u32, String>,
    next_doc_id: u32,
    mapping: IndexMapping,
    settings: Value,
    index: DocumentIndex,
    /// Track A `wp-a-perf-followups.md` Lot 1.5: the `_refresh`
    /// handler drops the in-memory `PostingsBuilder` snapshot via
    /// `DocumentIndex::finalize_postings()` to recover the ~1 GiB
    /// it carries on long-text corpora (BEIR TREC-COVID 171 k
    /// observed). Subsequent `append_to_index` calls cannot extend
    /// a finalized term dictionary, so they fall back to a one-shot
    /// `rebuild_index()` to preserve the previously-indexed
    /// postings. The flag is reset by any rebuild or append.
    terms_finalized: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredDocument {
    pub index: String,
    pub id: String,
    /// Refcounted handle on the stored `_source`. Cloning a
    /// `StoredDocument` only bumps the [`Arc`] strong count instead
    /// of duplicating the underlying JSON tree, which is the main
    /// driver of the matchID INSEE RAM footprint (~1.3 M docs).
    /// Consumers that need a `&Value` get one via deref coercion.
    pub source: Arc<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldScoringStats<'a> {
    pub doc_count: u64,
    pub avg_doc_len: f64,
    pub norms_enabled: bool,
    /// Dense per-`doc_id` length (`0` = absent), borrowed ZERO-COPY straight
    /// from the index's `FieldLengthStats::doc_len_dense` (optimisation mirrors
    /// the zero-copy `TermScoringView` #7). Empty slice when `norms_enabled` is
    /// false. `doc_len(doc_id)` is an O(1) cache-friendly index. The former
    /// owned `Vec` made `SearchScoringContext::new` copy the whole per-doc
    /// length array (~8 B × n_docs) per query per scored field; borrowing it
    /// removes that per-query allocation entirely (deces touches PRENOM + NOM,
    /// so it was ~2 × the full corpus length array copied on every query).
    pub doc_len_dense: &'a [u64],
}

impl<'a> FieldScoringStats<'a> {
    pub fn doc_len(&self, doc_id: u32) -> Option<u64> {
        self.doc_len_dense
            .get(doc_id as usize)
            .copied()
            .filter(|&len| len > 0)
    }

    pub fn min_doc_len(&self) -> Option<u64> {
        self.doc_len_dense
            .iter()
            .copied()
            .filter(|&len| len > 0)
            .min()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TermScoringStats {
    pub doc_freq: u64,
    /// Sorted ascending by `doc_id`.
    pub term_freq_by_doc_id: Vec<(u32, u64)>,
    /// Per-block stats aligned with `term_freq_by_doc_id.chunks(128)` —
    /// computed once at `PostingsBuilder::build()` time and copied here
    /// when the scoring context is built, so `maxscore_match` does not
    /// have to re-iterate the postings to recompute the per-block max
    /// term frequency at every query.
    pub block_metas: Vec<BlockMeta>,
}

impl TermScoringStats {
    pub fn term_freq(&self, doc_id: u32) -> u64 {
        self.term_freq_by_doc_id
            .binary_search_by_key(&doc_id, |(id, _)| *id)
            .ok()
            .map(|idx| self.term_freq_by_doc_id[idx].1)
            .unwrap_or(0)
    }

    pub fn max_term_freq(&self) -> u64 {
        self.term_freq_by_doc_id
            .iter()
            .map(|(_, tf)| *tf)
            .max()
            .unwrap_or(0)
    }
}

/// Zero-copy borrowed counterpart of [`TermScoringStats`] (optimisation
/// #7). A [`TermScoringStats`] copies the whole posting list into an owned
/// `Vec<(u32, u64)>` (widening `freq` `u32` → `u64`) and clones the
/// `block_metas` on every query for every distinct token. The scoring hot
/// path does not need owned data — it only reads it while the single
/// search read guard (optimisation #8) is held. `TermScoringView` borrows
/// the live [`Posting`] slice and [`BlockMeta`] slice straight out of the
/// in-memory term dictionary, eliminating both per-token allocations.
///
/// Parity: the postings come from the `TermDictionary` in ascending
/// `doc_id` order with exactly one [`Posting`] per `(doc_id, field, term)`
/// triple (the `analyzed_terms` invariant in
/// `DocumentIndex::add_validated_document`). So this borrowed slice is
/// element-for-element the same sequence the owned `term_freq_by_doc_id`
/// held, only with `freq` kept as `u32` (widened to `u64` at the exact
/// points the scorer consumes it). `doc_freq` equals `postings.len()`,
/// matching the owned struct's `term_freq_by_doc_id.len()`.
#[derive(Clone, Copy, Debug)]
pub struct TermScoringView<'a> {
    pub doc_freq: u64,
    /// Borrowed postings, sorted ascending by `doc_id`, one entry per doc.
    pub postings: &'a [Posting],
    /// Borrowed per-block stats aligned with `postings.chunks(BLOCK_SIZE)`.
    pub block_metas: &'a [BlockMeta],
}

impl<'a> TermScoringView<'a> {
    /// Empty view (term absent / field unknown). `doc_freq == 0` so the
    /// scorer skips it exactly as it skipped the default
    /// [`TermScoringStats`].
    pub fn empty() -> Self {
        Self {
            doc_freq: 0,
            postings: &[],
            block_metas: &[],
        }
    }

    /// Term frequency for `doc_id`, or 0 when the doc is absent. Binary
    /// search over the ascending-`doc_id` postings — identical lookup
    /// semantics to [`TermScoringStats::term_freq`], widening the stored
    /// `u32` freq to `u64`.
    pub fn term_freq(&self, doc_id: u32) -> u64 {
        self.postings
            .binary_search_by_key(&doc_id, |posting| posting.doc_id)
            .ok()
            .map(|idx| u64::from(self.postings[idx].freq))
            .unwrap_or(0)
    }

    /// Greatest term frequency across the postings (widened to `u64`).
    /// Matches [`TermScoringStats::max_term_freq`].
    pub fn max_term_freq(&self) -> u64 {
        self.postings
            .iter()
            .map(|posting| u64::from(posting.freq))
            .max()
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexMetadata {
    pub aliases: BTreeMap<String, Value>,
    pub mapping: Value,
    pub settings: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredComponentTemplate {
    pub component_template: Value,
    pub mapping: IndexMapping,
    pub settings: Value,
    pub aliases: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredIndexTemplate {
    pub index_template: Value,
    pub index_patterns: Vec<String>,
    pub composed_of: Vec<String>,
    pub mapping: IndexMapping,
    pub settings: Value,
    pub aliases: BTreeMap<String, Value>,
    pub priority: i64,
}

impl InMemoryIndex {
    fn new(mapping: IndexMapping, settings: Value) -> Self {
        Self {
            mapping,
            settings,
            next_doc_id: 0,
            ..Self::default()
        }
    }

    fn upsert_document(&mut self, id: &str, source: Value) {
        self.upsert_document_deferred(id, source);
        self.rebuild_index();
    }

    fn upsert_document_deferred(&mut self, id: &str, source: Value) {
        self.document_ids.entry(id.to_owned()).or_insert_with(|| {
            let doc_id = self.next_doc_id;
            self.next_doc_id += 1;
            self.reverse_document_ids.insert(doc_id, id.to_owned());
            doc_id
        });

        // Wrap once per upsert so concurrent readers that already
        // hold the previous `Arc` keep observing their consistent
        // snapshot; the write replaces the slot with a fresh handle.
        self.documents.insert(id.to_owned(), Arc::new(source));
        let inserted_source = self
            .documents
            .get(id)
            .expect("document must exist after insertion");
        self.mapping.ensure_fields(inserted_source);
    }

    fn delete_document(&mut self, id: &str) {
        if self.delete_document_deferred(id) {
            self.rebuild_index();
        }
    }

    fn delete_document_deferred(&mut self, id: &str) -> bool {
        if let Some(doc_id) = self.document_ids.remove(id) {
            self.documents.remove(id);
            self.reverse_document_ids.remove(&doc_id);
            return true;
        }
        false
    }

    fn mapping_value(&self) -> Value {
        self.mapping.as_value()
    }

    fn settings_value(&self) -> Value {
        self.settings.clone()
    }

    fn has_document(&self, id: &str) -> bool {
        self.document_ids.contains_key(id)
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        let documents = self
            .documents
            .iter()
            .filter_map(|(id, source)| {
                self.document_ids
                    .get(id)
                    .map(|doc_id| (*doc_id, indexed_fields_for_document(source, &self.mapping)))
            })
            .collect::<Vec<_>>();
        // Lot 1.6: defer the FST rebuild. The bulk path then chains
        // several rebuild/append calls without paying the per-call
        // build cost; the next `_refresh` or first search will
        // materialize once via `ensure_terms_ready`.
        let _ = self
            .index
            .add_documents_with_mapping_deferred(documents, &self.mapping);
        // Lot 1.5: keep the postings builder live across rebuilds. It is
        // the source of truth for `append_to_index`, and `refresh_index`
        // is now responsible for dropping it via `finalize_postings()`
        // once the index is declared read-mostly.
        self.terms_finalized = false;
    }

    /// Track A `wp-a-perf-followups.md` Lot 1: incremental append path
    /// used by `apply_document_writes` when a bulk batch only inserts
    /// fresh doc ids (no update of an existing id, no delete). Skips
    /// the quadratic `rebuild_index()` over the cumulative document
    /// store and re-tokenises only the freshly inserted docs.
    ///
    /// Caller contract: every `doc_id` in `new_doc_ids` must have just
    /// been inserted by `upsert_document_deferred` and must not be
    /// present in the index's `live_docs` yet — otherwise
    /// `DocumentIndex::add_documents_with_mapping` will reject the
    /// batch with `DuplicateDocId`. Update and delete paths must keep
    /// using `rebuild_index()` because the term dictionary cannot
    /// detach old postings without a full rebuild today.
    ///
    /// Unlike `rebuild_index()` this method does not call
    /// `finalize_postings()`: the postings builder must stay live for
    /// the next incremental append. The trade-off is an extra builder
    /// snapshot in RAM until the next full `rebuild_index()` (e.g. on
    /// `set_mapping`, single-doc PUT/DELETE, or a bulk with updates).
    fn append_to_index(&mut self, new_doc_ids: &[u32]) {
        if new_doc_ids.is_empty() {
            return;
        }
        if self.terms_finalized {
            // Lot 1.5: the `PostingsBuilder` snapshot was dropped by a
            // previous `refresh_index`. We cannot extend a finalized
            // term dictionary, so fall back to a one-shot full rebuild
            // that re-populates the builder with every cumulative doc;
            // subsequent appends within the same bulk batch run
            // incrementally on top of the now-live builder.
            self.rebuild_index();
            return;
        }
        let documents = new_doc_ids
            .iter()
            .filter_map(|&doc_id| {
                let id = self.reverse_document_ids.get(&doc_id)?;
                let source = self.documents.get(id)?;
                Some((doc_id, indexed_fields_for_document(source, &self.mapping)))
            })
            .collect::<Vec<_>>();
        // Lot 1.6: defer the FST rebuild on the bulk hot path. Reads
        // arriving between two `_bulk` POSTs go through
        // `AppState::ensure_terms_ready`, which materializes lazily.
        let _ = self
            .index
            .add_documents_with_mapping_deferred(documents, &self.mapping);
    }

    /// Track A `wp-a-perf-followups.md` Lot 1.5: free the in-memory
    /// `PostingsBuilder` snapshot once the index is declared
    /// read-mostly via `POST /:index/_refresh`. The next write on the
    /// index falls back to `rebuild_index()` via `append_to_index`'s
    /// finalized-state guard.
    ///
    /// Lot 1.6: any writes that landed between the previous refresh
    /// and this one are still pending in `postings_builder` — they
    /// have not been folded into `terms` yet because the bulk path
    /// defers the FST rebuild. We materialize once here so the
    /// caller's post-refresh searches see every previously-bulked
    /// doc, then drop the builder.
    fn finalize_terms_for_refresh(&mut self) {
        if self.terms_finalized {
            return;
        }
        self.index.materialize_terms();
        self.index.finalize_postings();
        self.terms_finalized = true;
    }

    fn set_mapping(&mut self, mapping: IndexMapping) {
        self.mapping = mapping;
        self.rebuild_index();
    }

    fn term_hits(&self, field: &str, value: &str) -> Vec<String> {
        if field.trim().is_empty() || value.is_empty() {
            return Vec::new();
        }

        let token = normalized_term_for_field(value, field, &self.mapping);
        if token.is_empty() {
            return Vec::new();
        }

        self.index
            .postings(field, &token)
            .into_iter()
            .flat_map(|postings| postings.map(|posting| posting.doc_id))
            .filter_map(|doc_id| self.reverse_document_ids.get(&doc_id).cloned())
            .collect()
    }

    fn count_term_hits(&self, field: &str, value: &str) -> usize {
        self.term_hits(field, value).len()
    }

    /// Optimisation #10 (beat-ES): internal `u32` doc-ids for a term, WITHOUT
    /// the per-doc public-`_id` `String` clone `term_hits` pays. Candidate
    /// resolution intersects these dense ints; public ids are resolved only for
    /// the final top-K window. Same doc set as `term_hits` (parity-safe).
    fn term_hits_internal(&self, field: &str, value: &str) -> Vec<u32> {
        if field.trim().is_empty() || value.is_empty() {
            return Vec::new();
        }
        let token = normalized_term_for_field(value, field, &self.mapping);
        if token.is_empty() {
            return Vec::new();
        }
        self.index
            .postings(field, &token)
            .into_iter()
            .flat_map(|postings| postings.map(|posting| posting.doc_id))
            .collect()
    }

    /// A6 phases 2 & 3: postings-backed prefix lookup.
    ///
    /// Three branches, in priority order:
    ///
    /// 1. **Text field with `index_prefixes` (phase 2)** — the field
    ///    carries a write-time prefix postings table; the normalized
    ///    prefix length must fall inside `[min_chars..=max_chars]`. The
    ///    lookup is O(1) on the side table.
    /// 2. **Keyword / Date field (phase 3)** — no `index_prefixes`
    ///    (matchID forbids it on non-text mappings, parity with ES 7.x:
    ///    see `mapping.rs::parse_field_mapping`). We FST-range-scan the
    ///    term dictionary for every term starting with the prefix and
    ///    union their doc id sets. Cost: O(matching_terms +
    ///    matching_postings). On the matchID `DATE_NAISSANCE`
    ///    autocomplete contract (`< 8 chars`, year + month range), the
    ///    cardinality is bounded by ~365 dates per matching year.
    /// 3. **Otherwise** — returns `None` so the candidate-set path falls
    ///    back to source-scan via
    ///    [`crate::search::prefix_field_matches`].
    ///
    /// `Some(vec)` always means the result is exact (possibly empty);
    /// `None` strictly means "the postings path is not applicable here".
    fn prefix_hits(&self, field: &str, prefix: &str) -> Option<Vec<String>> {
        if field.trim().is_empty() || prefix.is_empty() {
            return None;
        }
        let field_mapping = self.mapping.field(field)?;

        // Phase 2 path: text with index_prefixes.
        if let Some(bounds) = field_mapping.index_prefixes {
            let normalized = normalized_term_for_field(prefix, field, &self.mapping);
            if normalized.is_empty() {
                return None;
            }
            let prefix_len = normalized.chars().count();
            if prefix_len < bounds.min_chars || prefix_len > bounds.max_chars {
                return None;
            }

            let hits = self
                .index
                .prefix_postings(field, &normalized)
                .map(|set| {
                    set.iter()
                        .filter_map(|doc_id| self.reverse_document_ids.get(doc_id).cloned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return Some(hits);
        }

        // Phase 3 path: keyword / date — FST range scan over the term
        // dictionary. We deliberately scope this to types whose default
        // analyzer is `KeywordAnalyzer` (whole-value token) so the FST
        // bytes line up with the user-supplied prefix without surprise:
        // a `text` field with `Simple`/`Standard` analysis would have
        // tokenized & folded the source before indexing, so a raw FST
        // prefix scan would diverge from `prefix_field_matches`. Those
        // fields stay on the source-scan fallback.
        match field_mapping.field_type {
            FieldType::Keyword | FieldType::Date => {
                let normalized = normalized_term_for_field(prefix, field, &self.mapping);
                if normalized.is_empty() {
                    return None;
                }
                let doc_ids = self.index.term_prefix_doc_ids(field, &normalized);
                let hits = doc_ids
                    .iter()
                    .filter_map(|doc_id| self.reverse_document_ids.get(doc_id).cloned())
                    .collect::<Vec<_>>();
                Some(hits)
            }
            _ => None,
        }
    }

    /// Optimisation #10 (beat-ES): internal `u32` doc-ids for a prefix, mirror
    /// of `prefix_hits` without the public-`_id` `String` clone. Same branches,
    /// same doc set (parity-safe).
    fn prefix_hits_internal(&self, field: &str, prefix: &str) -> Option<Vec<u32>> {
        if field.trim().is_empty() || prefix.is_empty() {
            return None;
        }
        let field_mapping = self.mapping.field(field)?;
        if let Some(bounds) = field_mapping.index_prefixes {
            let normalized = normalized_term_for_field(prefix, field, &self.mapping);
            if normalized.is_empty() {
                return None;
            }
            let prefix_len = normalized.chars().count();
            if prefix_len < bounds.min_chars || prefix_len > bounds.max_chars {
                return None;
            }
            let hits = self
                .index
                .prefix_postings(field, &normalized)
                .map(|set| set.iter().copied().collect::<Vec<u32>>())
                .unwrap_or_default();
            return Some(hits);
        }
        match field_mapping.field_type {
            FieldType::Keyword | FieldType::Date => {
                let normalized = normalized_term_for_field(prefix, field, &self.mapping);
                if normalized.is_empty() {
                    return None;
                }
                Some(
                    self.index
                        .term_prefix_doc_ids(field, &normalized)
                        .into_iter()
                        .collect(),
                )
            }
            _ => None,
        }
    }

    fn field_scoring_stats(&self, field: &str) -> Option<FieldScoringStats<'_>> {
        let stats = self.index.field_stats(field)?;
        let norms_enabled = self.mapping.norms_enabled(field);
        let avg_doc_len = if norms_enabled {
            stats.avg_doc_len()?
        } else {
            1.0
        };
        // Borrow the index's dense slice zero-copy — no per-query allocation,
        // O(1) cache-friendly doc_id indexing in the hot loop.
        let doc_len_dense: &[u64] = if norms_enabled {
            stats.doc_len_dense()
        } else {
            &[]
        };

        Some(FieldScoringStats {
            doc_count: stats.doc_count,
            avg_doc_len,
            norms_enabled,
            doc_len_dense,
        })
    }

    fn term_scoring_stats(&self, field: &str, term: &str) -> TermScoringStats {
        // Postings come from `TermDictionary` in ascending `doc_id` order
        // (see `PostingsBuilder::build`), so a single pass produces a sorted
        // accumulator without re-sorting. We merge same-doc postings (rare
        // unless multiple positions push the same doc id repeatedly) by
        // checking the tail.
        let mut term_freq_by_doc_id: Vec<(u32, u64)> = Vec::new();
        for posting in self.index.postings(field, term).into_iter().flatten() {
            let freq = u64::from(posting.freq);
            match term_freq_by_doc_id.last_mut() {
                Some((id, current)) if *id == posting.doc_id => {
                    *current += freq;
                }
                _ => term_freq_by_doc_id.push((posting.doc_id, freq)),
            }
        }

        // Pre-built per-block stats live next to the postings in
        // `DocumentIndex` (FST-indexed parallel `Vec<Vec<BlockMeta>>`).
        // We copy the slice into the scoring stats so the scoring loop
        // does not have to keep a reference into the index — and so
        // the data path matches the on-disk codec we're building
        // toward, where these metas live in their own block.
        //
        // The 128-block alignment between `block_metas` (built from raw
        // postings) and `term_freq_by_doc_id.chunks(128)` (built here)
        // relies on the `analyzed_terms` invariant in
        // `DocumentIndex::add_validated_document`: a given
        // `(doc_id, field, term)` triple produces exactly one posting,
        // so the merge branch above is a defensive no-op and both Vecs
        // have the same length. The `debug_assert_eq!` below catches a
        // regression as soon as it happens.
        let block_metas = self
            .index
            .block_metas(field, term)
            .map(<[BlockMeta]>::to_vec)
            .unwrap_or_default();
        debug_assert_eq!(
            block_metas.len(),
            term_freq_by_doc_id.len().div_ceil(128),
            "block_metas alignment with term_freq_by_doc_id chunks broken \
             (field={field}, term={term}, postings={}, metas={})",
            term_freq_by_doc_id.len(),
            block_metas.len(),
        );

        TermScoringStats {
            doc_freq: term_freq_by_doc_id.len() as u64,
            term_freq_by_doc_id,
            block_metas,
        }
    }

    /// Zero-copy borrowed term stats (optimisation #7). Returns a
    /// [`TermScoringView`] that borrows the postings + block metas
    /// directly from the term dictionary instead of copying them into
    /// owned `Vec`s like [`Self::term_scoring_stats`] does.
    ///
    /// Parity with the owned path: `doc_freq` is `postings.len()` (one
    /// posting per `(doc_id, field, term)` triple — see the invariant
    /// documented on `term_scoring_stats`), so it equals the owned
    /// struct's `term_freq_by_doc_id.len()`. The `debug_assert_eq!`
    /// mirrors the owned path's block-meta alignment guard so a codec
    /// regression is caught in debug builds. An absent field/term yields
    /// an empty view (`doc_freq == 0`), exactly like the default
    /// `TermScoringStats`.
    fn term_scoring_view(&self, field: &str, term: &str) -> TermScoringView<'_> {
        match self.index.postings_with_block_metas(field, term) {
            Some(list) => {
                let postings = list.postings();
                let block_metas = list.block_metas();
                debug_assert_eq!(
                    block_metas.len(),
                    postings.len().div_ceil(128),
                    "block_metas alignment with postings chunks broken \
                     (field={field}, term={term}, postings={}, metas={})",
                    postings.len(),
                    block_metas.len(),
                );
                TermScoringView {
                    doc_freq: postings.len() as u64,
                    postings,
                    block_metas,
                }
            }
            None => TermScoringView::empty(),
        }
    }

    fn match_hits(&self, field: &str, value: &str, require_all_terms: bool) -> Vec<String> {
        self.match_hits_internal(field, value, require_all_terms)
            .into_iter()
            .filter_map(|doc_id| self.reverse_document_ids.get(&doc_id).cloned())
            .collect()
    }

    fn match_hits_internal(&self, field: &str, value: &str, require_all_terms: bool) -> Vec<u32> {
        if field.trim().is_empty() || value.is_empty() {
            return Vec::new();
        }

        let terms = normalized_terms_for_field(value, field, &self.mapping);
        if terms.is_empty() {
            return Vec::new();
        }

        // Single-token fast path: postings are stored ascending by doc_id with
        // exactly one entry per doc (the `analyzed_terms` invariant), so the
        // matched doc set IS the posting list — collecting straight into a Vec
        // skips the `BTreeSet` round-trip, which on a common term costs
        // O(df log df) inserts plus a node allocation per doc. This is the
        // single-clause candidate-resolution path (deces `match NOM=…` and each
        // leapfrog conjunction clause). Parity: identical ascending-unique set.
        if terms.len() == 1 {
            return self
                .index
                .postings(field, &terms[0])
                .into_iter()
                .flat_map(|postings| postings.map(|posting| posting.doc_id))
                .collect();
        }

        let mut matches: Option<BTreeSet<u32>> = None;
        for term in terms {
            let current = self
                .index
                .postings(field, &term)
                .into_iter()
                .flat_map(|postings| postings.map(|posting| posting.doc_id))
                .collect::<BTreeSet<_>>();

            matches = Some(match matches {
                None => current,
                Some(mut previous) if require_all_terms => {
                    previous.retain(|doc_id| current.contains(doc_id));
                    previous
                }
                Some(mut previous) => {
                    previous.extend(current);
                    previous
                }
            });

            if require_all_terms && matches.as_ref().is_some_and(BTreeSet::is_empty) {
                break;
            }
        }

        matches.unwrap_or_default().into_iter().collect()
    }

    /// Optimisation #11 (beat-ES): leapfrog/galloping intersection of several
    /// single-term posting lists WITHOUT materialising any of them. Drives the
    /// rarest term and `advance_to`s the others over their block skip-lists
    /// (Lucene's `ConjunctionScorer`). The decomposition showed the deces cost
    /// is O(df) per-term posting materialisation (a single `match` on a common
    /// term is ~36 ms; the `bool` conjunction ~2x that); leapfrog avoids
    /// touching the common term's full list when one term is rarer.
    ///
    /// `terms` is the FULL conjunction — every `(field, term)` must match.
    /// Returns matched internal doc-ids in ascending order — byte-identical to
    /// the `BTreeSet` intersection of the same lists (parity-safe).
    fn conjunction_hits_internal(&self, terms: &[(String, String)]) -> Vec<u32> {
        if terms.is_empty() {
            return Vec::new();
        }
        // Resolve every required term's posting list; a missing/empty term
        // makes the whole AND empty.
        let mut lists: Vec<PostingsList<'_>> = Vec::with_capacity(terms.len());
        for (field, term) in terms {
            match self.index.postings_with_block_metas(field, term) {
                Some(list) if !list.postings().is_empty() => lists.push(list),
                _ => return Vec::new(),
            }
        }
        // Drive the rarest term; advance_to the others.
        lists.sort_by_key(|l| l.postings().len());
        let mut iters: Vec<PostingsBlockSkipIter<'_>> = Vec::with_capacity(lists.len() - 1);
        for l in &lists[1..] {
            match l.skip_iter() {
                Ok(Some(it)) => iters.push(it),
                // No skip list (tiny list) or codec hiccup -> exact materialised
                // intersection (correctness over speed for this rare case).
                _ => return Self::materialised_conjunction(&lists),
            }
        }
        // `advance_to` RETURNS-AND-CONSUMES (position moves past the posting it
        // returns), so we must HOLD each iterator's current doc-id in `cur[i]`
        // and only re-advance when the driver target strictly exceeds it.
        // Otherwise a posting `p > target` returned for a missed target would be
        // skipped and a later equal driver doc would be lost (parity bug).
        // `cur[i] = None` means that iterator is exhausted -> no further matches.
        let mut cur: Vec<Option<u32>> = iters
            .iter_mut()
            .map(|it| it.advance_to(0).map(|p| p.doc_id))
            .collect();
        let mut out = Vec::new();
        'docs: for posting in lists[0].postings() {
            let target = posting.doc_id;
            for (i, it) in iters.iter_mut().enumerate() {
                if cur[i].is_some_and(|c| c < target) {
                    cur[i] = it.advance_to(target).map(|p| p.doc_id);
                }
                if cur[i] != Some(target) {
                    continue 'docs;
                }
            }
            out.push(target);
        }
        out
    }

    /// Exact `BTreeSet` intersection of the lists' doc-ids (ascending). Fallback
    /// for `conjunction_hits_internal` when a list has no skip list.
    fn materialised_conjunction(lists: &[PostingsList<'_>]) -> Vec<u32> {
        let mut acc: Option<BTreeSet<u32>> = None;
        for l in lists {
            let set: BTreeSet<u32> = l.postings().iter().map(|p| p.doc_id).collect();
            acc = Some(match acc {
                None => set,
                Some(prev) => prev.intersection(&set).copied().collect(),
            });
        }
        acc.unwrap_or_default().into_iter().collect()
    }

    fn documents_by_internal_ids(&self, index: &str, internal_ids: &[u32]) -> Vec<StoredDocument> {
        internal_ids
            .iter()
            .filter_map(|doc_id| {
                let id = self.reverse_document_ids.get(doc_id)?;
                self.documents.get(id).map(|source| StoredDocument {
                    index: index.to_owned(),
                    id: id.clone(),
                    // Refcount bump, not a JSON deep clone.
                    source: Arc::clone(source),
                })
            })
            .collect()
    }
}

/// Borrowed read-only view of one index, scoped to a single
/// `store.read()` guard (optimisation #8). The search path used to take
/// one read lock per candidate-resolution call, per scoring-stats lookup
/// (one per distinct query token, each ALSO re-running
/// `ensure_terms_ready`), and again per `_source` hydration — `~2N+`
/// acquisitions on a writer-preferring `std::sync::RwLock`. `IndexReader`
/// borrows `&InMemoryIndex` once and threads it through the whole query:
/// candidate resolution, scoring-context construction, term-stats lookup,
/// and hydration all read through this single borrow, so the lock is
/// acquired exactly once (after `ensure_terms_ready` has run up front).
///
/// Because it is a plain borrow of the live index, the term-stats lookups
/// it exposes ([`Self::term_scoring_view`]) hand out zero-copy
/// [`TermScoringView`]s (optimisation #7) instead of the owned
/// [`TermScoringStats`] copies the lock-per-token path produced.
pub struct IndexReader<'a> {
    index: &'a str,
    data: &'a InMemoryIndex,
}

impl<'a> IndexReader<'a> {
    /// The index mapping (borrowed). Threading this avoids the separate
    /// `index_mapping` read-lock acquisition the scoring context used to
    /// take.
    pub fn mapping(&self) -> &'a IndexMapping {
        &self.data.mapping
    }

    /// Per-field scoring stats (doc count, avg doc len, norms). The `doc_len`
    /// slice is borrowed zero-copy from the live index (like
    /// [`Self::term_scoring_view`]), so no per-query allocation is paid.
    pub fn field_scoring_stats(&self, field: &str) -> Option<FieldScoringStats<'a>> {
        self.data.field_scoring_stats(field)
    }

    /// Zero-copy borrowed term stats (optimisation #7). Equivalent data to
    /// [`AppState::term_scoring_stats`] but borrowed from the live term
    /// dictionary instead of copied into owned `Vec`s.
    pub fn term_scoring_view(&self, field: &str, term: &str) -> TermScoringView<'a> {
        self.data.term_scoring_view(field, term)
    }

    /// Internal candidate ids for an OR/AND `match` over `field`.
    /// Identical to [`AppState::documents_for_match_internal`] but reads
    /// through the shared guard.
    pub fn match_hits_internal(
        &self,
        field: &str,
        value: &str,
        require_all_terms: bool,
    ) -> Vec<u32> {
        self.data
            .match_hits_internal(field, value, require_all_terms)
    }

    /// Hydrate `_source` documents for internal ids through the shared
    /// guard. Identical to [`AppState::documents_by_internal_ids`].
    pub fn documents_by_internal_ids(&self, internal_ids: &[u32]) -> Vec<StoredDocument> {
        self.data
            .documents_by_internal_ids(self.index, internal_ids)
    }

    /// Map public ids to internal doc ids through the shared guard.
    /// Identical to [`AppState::internal_doc_ids`].
    pub fn internal_doc_ids(&self, public_ids: &[&str]) -> Vec<Option<u32>> {
        public_ids
            .iter()
            .map(|id| self.data.document_ids.get(*id).copied())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DocumentWriteOperation {
    Index {
        index: String,
        id: String,
        source: Value,
        status: u16,
    },
    Create {
        index: String,
        id: String,
        source: Value,
    },
    Delete {
        index: String,
        id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentWriteResult {
    Applied {
        index: String,
        id: String,
        status: u16,
    },
    VersionConflict {
        index: String,
        id: String,
    },
}

fn normalized_term_for_field(value: &str, field: &str, mapping: &IndexMapping) -> String {
    mapping.analyzer(field).first_term(value)
}

fn normalized_terms_for_field(value: &str, field: &str, mapping: &IndexMapping) -> Vec<String> {
    // A1/A13: a field with a custom analyzer or explicit `search_analyzer`
    // resolves its query tokens against the index analysis settings (e.g. an
    // edge_ngram autocomplete sub-field searched with `standard`). Every
    // builtin-only field returns `None` here and keeps the legacy path.
    if let Some(terms) = mapping.custom_search_terms_for_field(value, field) {
        return terms;
    }
    mapping.analyzer(field).terms(value)
}

fn indexed_fields_for_document(document: &Value, mapping: &IndexMapping) -> Vec<(String, String)> {
    let Some(object) = document.as_object() else {
        return Vec::new();
    };

    object
        .iter()
        .flat_map(|(name, value)| {
            let values = scalar_values(value, mapping, name);
            values.into_iter().map(move |value| (name.clone(), value))
        })
        .collect()
}

fn scalar_values(document: &Value, mapping: &IndexMapping, field: &str) -> Vec<String> {
    match document {
        Value::String(value) => vec![value.clone()],
        Value::Number(value) => vec![value.to_string()],
        Value::Bool(value) => vec![value.to_string()],
        Value::Array(values) => values
            .iter()
            .flat_map(|value| scalar_values(value, mapping, field))
            .collect(),
        Value::Object(value) if mapping.field(field).is_some() => {
            serde_json::to_string(value).map_or_else(|_| Vec::new(), |encoded| vec![encoded])
        }
        Value::Object(_) => Vec::new(),
        Value::Null => Vec::new(),
    }
}

impl AppState {
    pub fn search_cache_get(&self, index: &str, key: u64) -> Option<Vec<u8>> {
        let cache = self
            .search_cache
            .read()
            .expect("search cache lock should not be poisoned");
        cache
            .get(index)
            .and_then(|entry| entry.entries.get(&key).cloned())
    }

    pub fn search_cache_put(&self, index: &str, key: u64, value: Vec<u8>) {
        let mut cache = self
            .search_cache
            .write()
            .expect("search cache lock should not be poisoned");
        let entry = cache.entry(index.to_owned()).or_default();
        if entry.entries.insert(key, value).is_none() {
            entry.order.push_back(key);
            while entry.entries.len() > SEARCH_CACHE_CAPACITY {
                if let Some(oldest) = entry.order.pop_front() {
                    entry.entries.remove(&oldest);
                } else {
                    break;
                }
            }
        }
    }

    fn invalidate_search_cache(&self, index: &str) {
        let mut cache = self
            .search_cache
            .write()
            .expect("search cache lock should not be poisoned");
        cache.remove(index);
    }

    pub fn create_index(
        &self,
        index: &str,
        mapping: Option<IndexMapping>,
        settings: Value,
        aliases: BTreeMap<String, Value>,
    ) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");

        create_index_if_missing(
            &mut store,
            index,
            mapping.unwrap_or_default(),
            settings,
            aliases,
        );
        drop(store);
        // Empty index, but seed the gauges at zero so the scrape advertises
        // the index from the moment it exists rather than only after the
        // first write.
        refresh_memory_gauges(self, index);
    }

    pub fn put_index_template(
        &self,
        name: &str,
        mut template: StoredIndexTemplate,
    ) -> Result<(), String> {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        snapshot_component_templates(&mut template, &store.component_templates)?;
        store.index_templates.insert(name.to_owned(), template);
        Ok(())
    }

    pub fn index_template(&self, name: &str) -> Option<StoredIndexTemplate> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.index_templates.get(name).cloned()
    }

    pub fn all_index_templates(&self) -> BTreeMap<String, StoredIndexTemplate> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.index_templates.clone()
    }

    pub fn delete_index_template(&self, name: &str) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.index_templates.remove(name).is_some()
    }

    pub fn put_component_template(&self, name: &str, template: StoredComponentTemplate) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.component_templates.insert(name.to_owned(), template);
    }

    pub fn component_template(&self, name: &str) -> Option<StoredComponentTemplate> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.component_templates.get(name).cloned()
    }

    pub fn all_component_templates(&self) -> BTreeMap<String, StoredComponentTemplate> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.component_templates.clone()
    }

    pub fn delete_component_template(&self, name: &str) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.component_templates.remove(name).is_some()
    }

    pub fn delete_index(&self, index: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.remove(index);
        let stale_aliases: Vec<String> = store
            .aliases
            .iter_mut()
            .filter_map(|(alias, indices)| {
                indices.remove(index);
                indices.is_empty().then(|| alias.clone())
            })
            .collect();
        for alias in stale_aliases {
            store.aliases.remove(&alias);
        }
        drop(store);
        self.invalidate_search_cache(index);
        // Index gone: zero out its gauges so dashboards do not advertise
        // stale RAM for a vanished tenant.
        clear_memory_gauges(index);
    }

    /// Track A `wp-a-perf-followups.md` Lot 1.6: per-index instrumentation
    /// for the number of FST rebuilds that have actually run on the
    /// named index. Returns `0` for an unknown index or one that has
    /// never been written. Used by the `bulk_router_*` test suite to
    /// prove that N `_bulk` POSTs no longer trigger N FST rebuilds.
    pub fn index_terms_build_count(&self, index: &str) -> u64 {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or(0, |data| data.index.terms_build_count())
    }

    /// Track A `wp-a-perf-followups.md` Lot 1.6: lazily rebuild the
    /// FST term dictionary on the named index iff writes are pending.
    /// Search entry points on `AppState` (search/match/term/prefix
    /// lookups, scoring stats) call this before grabbing the read
    /// lock so the deferred-build invariant in `DocumentIndex` is
    /// upheld without forcing every read path to take a write lock.
    ///
    /// Implementation: fast-path read-lock probes the `terms_dirty`
    /// flag; slow-path takes the write lock to actually materialize.
    /// Both paths are cheap when the index is clean. The
    /// double-checked pattern below is safe because
    /// `materialize_terms` is idempotent: a racing writer that flips
    /// the flag while we drop the read lock will see the materialize
    /// run, and a racing materializer will short-circuit the second
    /// call as a no-op.
    pub fn ensure_terms_ready(&self, index: &str) {
        {
            let store = self
                .store
                .read()
                .expect("in-memory API state lock should not be poisoned");
            match store.indices.get(index) {
                None => return,
                Some(data) if !data.index.terms_dirty() => return,
                Some(_) => {}
            }
        }
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            // `materialize_terms` is idempotent: a racing materializer
            // that ran while we were upgrading the lock will have
            // flipped `terms_dirty` back to `false`, and this call
            // becomes a no-op.
            data.index.materialize_terms();
        }
    }

    /// Track A `wp-a-perf-followups.md` Lot 1.5: drop the in-memory
    /// `PostingsBuilder` snapshot on the named index so the long-text
    /// bulk RAM overhead (~1 GiB observed on BEIR TREC-COVID) is
    /// released once the caller stops writing. A subsequent
    /// `_bulk`/single-doc write triggers a one-shot `rebuild_index()`
    /// (via `IndexData::append_to_index`'s finalized-state guard) to
    /// preserve the previously-indexed postings.
    pub fn refresh_index(&self, index: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            data.finalize_terms_for_refresh();
        }
        drop(store);
        // Match the post-bulk cache + gauge maintenance contract so a
        // refresh that frees the builder is observable through the
        // `surch_index_*` Prometheus gauges.
        self.invalidate_search_cache(index);
        refresh_memory_gauges(self, index);
    }

    pub fn index_exists(&self, index: &str) -> bool {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.contains_key(index)
    }

    pub fn index_document(&self, index: &str, id: &str, source: Value) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");

        create_index_if_missing(
            &mut store,
            index,
            IndexMapping::default(),
            Value::Object(Default::default()),
            BTreeMap::new(),
        );
        let data = store
            .indices
            .get_mut(index)
            .expect("index must exist after implicit creation");
        data.upsert_document(id, source);
        drop(store);
        self.invalidate_search_cache(index);
        refresh_memory_gauges(self, index);
    }

    pub fn create_document(&self, index: &str, id: &str, source: Value) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");

        create_index_if_missing(
            &mut store,
            index,
            IndexMapping::default(),
            Value::Object(Default::default()),
            BTreeMap::new(),
        );
        let data = store
            .indices
            .get_mut(index)
            .expect("index must exist after implicit creation");
        if data.has_document(id) {
            return false;
        }

        data.upsert_document(id, source);
        drop(store);
        self.invalidate_search_cache(index);
        refresh_memory_gauges(self, index);
        true
    }

    pub fn set_mapping(&self, index: &str, mapping: IndexMapping) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .entry(index.to_owned())
            .or_insert_with(|| {
                InMemoryIndex::new(IndexMapping::default(), Value::Object(Default::default()))
            })
            .set_mapping(mapping);
        drop(store);
        self.invalidate_search_cache(index);
        // A mapping change triggers a `rebuild_index()` on the
        // DocumentIndex, so postings/prefix-postings sizes can swing
        // wildly — refresh gauges.
        refresh_memory_gauges(self, index);
    }

    /// Merge the supplied field mappings into the existing index mapping.
    ///
    /// Returns the field name on the first type conflict; new fields are appended.
    pub fn merge_field_mappings(
        &self,
        index: &str,
        new_fields: &[(String, FieldMapping)],
    ) -> Result<(), String> {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        let Some(data) = store.indices.get_mut(index) else {
            return Err(format!("index [{index}] missing"));
        };

        let mut merged = data.mapping.clone();
        for (field, mapping) in new_fields {
            if let Some(existing) = merged.field(field) {
                if existing.field_type != mapping.field_type {
                    return Err(format!(
                        "mapper [{field}] of different type, current_type [{}], merged_type [{}]",
                        existing.field_type.as_str(),
                        mapping.field_type.as_str(),
                    ));
                }
            }
            merged.set_field_mapping(field.clone(), mapping.clone());
        }

        data.set_mapping(merged);
        drop(store);
        self.invalidate_search_cache(index);
        refresh_memory_gauges(self, index);
        Ok(())
    }

    pub fn delete_document(&self, index: &str, id: &str) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            data.delete_document(id);
        }
        drop(store);
        self.invalidate_search_cache(index);
        refresh_memory_gauges(self, index);
    }

    pub fn apply_document_writes(
        &self,
        operations: Vec<DocumentWriteOperation>,
    ) -> Vec<DocumentWriteResult> {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        let mut touched = BTreeSet::new();
        // Track A `wp-a-perf-followups.md` Lot 1: bulk batches that only
        // insert fresh doc ids skip the cumulative `rebuild_index()` via
        // `append_to_index`. Any update of an existing id or any delete
        // forces the full rebuild because the term dictionary cannot
        // detach old postings incrementally today.
        let mut new_doc_ids_per_index: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        let mut needs_full_rebuild: BTreeSet<String> = BTreeSet::new();
        let mut results = Vec::with_capacity(operations.len());

        for operation in operations {
            match operation {
                DocumentWriteOperation::Index {
                    index,
                    id,
                    source,
                    status,
                } => {
                    create_index_if_missing(
                        &mut store,
                        &index,
                        IndexMapping::default(),
                        Value::Object(Default::default()),
                        BTreeMap::new(),
                    );
                    let data = store
                        .indices
                        .get_mut(&index)
                        .expect("index must exist after implicit creation");
                    let was_present = data.has_document(&id);
                    data.upsert_document_deferred(&id, source);
                    touched.insert(index.clone());
                    if was_present {
                        needs_full_rebuild.insert(index.clone());
                    } else if let Some(&doc_id) = data.document_ids.get(&id) {
                        new_doc_ids_per_index
                            .entry(index.clone())
                            .or_default()
                            .push(doc_id);
                    }
                    results.push(DocumentWriteResult::Applied { index, id, status });
                }
                DocumentWriteOperation::Create { index, id, source } => {
                    create_index_if_missing(
                        &mut store,
                        &index,
                        IndexMapping::default(),
                        Value::Object(Default::default()),
                        BTreeMap::new(),
                    );
                    let data = store
                        .indices
                        .get_mut(&index)
                        .expect("index must exist after implicit creation");
                    if data.has_document(&id) {
                        results.push(DocumentWriteResult::VersionConflict { index, id });
                    } else {
                        data.upsert_document_deferred(&id, source);
                        touched.insert(index.clone());
                        if let Some(&doc_id) = data.document_ids.get(&id) {
                            new_doc_ids_per_index
                                .entry(index.clone())
                                .or_default()
                                .push(doc_id);
                        }
                        results.push(DocumentWriteResult::Applied {
                            index,
                            id,
                            status: 201,
                        });
                    }
                }
                DocumentWriteOperation::Delete { index, id } => {
                    if let Some(data) = store.indices.get_mut(&index) {
                        if data.delete_document_deferred(&id) {
                            touched.insert(index.clone());
                            needs_full_rebuild.insert(index.clone());
                        }
                    }
                    results.push(DocumentWriteResult::Applied {
                        index,
                        id,
                        status: 200,
                    });
                }
            }
        }

        for index in &touched {
            if let Some(data) = store.indices.get_mut(index) {
                if needs_full_rebuild.contains(index) {
                    data.rebuild_index();
                } else if let Some(new_ids) = new_doc_ids_per_index.get(index) {
                    data.append_to_index(new_ids);
                } else {
                    // No new docs and no update/delete on this index — nothing
                    // to do (e.g. a Create that collided with a version
                    // conflict touched the index entry via implicit creation
                    // but not its postings).
                }
            }
        }
        drop(store);
        for index in &touched {
            self.invalidate_search_cache(index);
            // Lot 1.6: skip `refresh_memory_gauges` between bulk
            // chunks. Calling it here would force the FST to
            // materialize after every `_bulk` POST (because the
            // postings accounting walks the dictionary), which is
            // exactly the per-chunk rebuild cost this lot is meant
            // to eliminate. The gauges are refreshed at the next
            // `/_surch/stats` query, the next `_refresh`, or any
            // single-doc PUT/DELETE — all of which call
            // `ensure_terms_ready` and then re-snapshot accurate
            // numbers. The bench scenario (21 `_bulk` POSTs followed
            // by one `_refresh`) sees one materialize total instead
            // of 21.
        }

        results
    }

    pub fn count(&self, index: &str) -> u64 {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or(0, |index| index.documents.len() as u64)
    }

    pub fn mapping(&self, index: &str) -> Option<Value> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.get(index).map(|data| data.mapping_value())
    }

    pub fn index_mapping(&self, index: &str) -> Option<IndexMapping> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.get(index).map(|data| data.mapping.clone())
    }

    /// A10 → A12 (Phase 4): per-document stored projection of a
    /// multi-field sub-field, keyed by the public `_id`.
    ///
    /// Returns `Some(map)` iff `field_path` is a declared multi-field
    /// sub-field (`parent.sub`) that the write-time fan-out
    /// ([`DocumentIndex::index_subfields`]) materialised — i.e. when
    /// [`DocumentIndex::has_subfield_values`] is `true`. Each entry maps a
    /// public document id to the sub-field's stored value, with the
    /// sub-field's analyzer/normalizer already applied at index time
    /// (`NOM.raw` → lowercased + asciifolded keyword token). The map only
    /// contains documents that actually carried the parent field.
    ///
    /// The query side (`sort` / `agg` on `.raw` / `.norm`) uses this to
    /// read the A10 storage directly instead of re-scanning `_source` via
    /// `lookup_sort_value` and re-normalising on read. Returns `None` for
    /// top-level fields and for sub-fields with no stored projection
    /// (e.g. an index without an explicit multi-field mapping), so the
    /// caller transparently falls back to the legacy `_source` alias.
    ///
    /// Computed once per query (one read-lock acquisition) so the sort
    /// comparator and the aggregation loop do not re-take the lock per
    /// document.
    pub fn subfield_projection(
        &self,
        index: &str,
        field_path: &str,
    ) -> Option<BTreeMap<String, String>> {
        // Lot 1.6: the side-table is populated at write time, but a
        // pending deferred FST build must be materialised so reads see a
        // consistent post-write snapshot (mirrors the other read paths).
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let data = store.indices.get(index)?;
        let per_doc = data.index.subfield_values_map().get(field_path)?;
        let projection = per_doc
            .iter()
            .filter_map(|(doc_id, value)| {
                data.reverse_document_ids
                    .get(doc_id)
                    .map(|public_id| (public_id.clone(), value.clone()))
            })
            .collect();
        Some(projection)
    }

    pub fn index_metadata(&self, index: &str) -> Option<IndexMetadata> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let data = store.indices.get(index)?;
        let aliases = store
            .aliases
            .iter()
            .filter_map(|(alias, indices)| {
                indices
                    .get(index)
                    .map(|definition| (alias.clone(), definition.clone()))
            })
            .collect();
        Some(IndexMetadata {
            aliases,
            mapping: data.mapping_value(),
            settings: data.settings_value(),
        })
    }

    pub fn index_names(&self) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.keys().cloned().collect()
    }

    pub fn add_alias(&self, index: &str, alias: &str) -> bool {
        self.add_alias_with_definition(index, alias, Value::Object(Default::default()))
    }

    pub fn add_alias_with_definition(&self, index: &str, alias: &str, definition: Value) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if !store.indices.contains_key(index) {
            return false;
        }
        store
            .aliases
            .entry(alias.to_owned())
            .or_default()
            .insert(index.to_owned(), definition);
        true
    }

    pub fn remove_alias(&self, index: &str, alias: &str) -> bool {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        let mut removed = false;
        if let Some(entry) = store.aliases.get_mut(alias) {
            removed = entry.remove(index).is_some();
            if entry.is_empty() {
                store.aliases.remove(alias);
            }
        }
        removed
    }

    pub fn alias_exists(&self, alias: &str) -> bool {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.aliases.contains_key(alias)
    }

    pub fn aliases_for_index(&self, index: &str) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .aliases
            .iter()
            .filter(|(_, indices)| indices.contains_key(index))
            .map(|(alias, _)| alias.clone())
            .collect()
    }

    pub fn alias_definitions_for_index(&self, index: &str) -> BTreeMap<String, Value> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .aliases
            .iter()
            .filter_map(|(alias, indices)| {
                indices
                    .get(index)
                    .map(|definition| (alias.clone(), definition.clone()))
            })
            .collect()
    }

    pub fn indices_for_alias(&self, alias: &str) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .aliases
            .get(alias)
            .map(|indices| indices.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Resolve a write-side path target to a single physical index name.
    ///
    /// - Existing index → returns that index.
    /// - Unknown name (will be implicitly created) → returns the name as-is.
    /// - Alias pointing to exactly one index → returns that index.
    /// - Alias pointing to several indices → `Err` with the OpenSearch reason.
    pub fn resolve_write_target(&self, target: &str) -> Result<String, String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        if store.indices.contains_key(target) {
            return Ok(target.to_owned());
        }
        if let Some(indices) = store.aliases.get(target) {
            return match indices.len() {
                1 => Ok(indices.keys().next().expect("non-empty alias map").clone()),
                _ => {
                    let write_indices = indices
                        .iter()
                        .filter(|(_, definition)| {
                            definition
                                .get("is_write_index")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                        })
                        .map(|(index, _)| index.clone())
                        .collect::<Vec<_>>();
                    if write_indices.len() == 1 {
                        return Ok(write_indices[0].clone());
                    }
                    Err(format!(
                    "no write index is defined for alias [{target}], target alias must point to a single index"
                    ))
                }
            };
        }
        Ok(target.to_owned())
    }

    /// Resolve a path-level target into the set of physical indices it points to.
    ///
    /// - Existing index name → `[name]`.
    /// - Known alias → the list of indices the alias points to.
    /// - Unknown name → empty.
    pub fn resolve_index(&self, target: &str) -> Vec<String> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        if store.indices.contains_key(target) {
            return vec![target.to_owned()];
        }
        store
            .aliases
            .get(target)
            .map(|indices| indices.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn all_aliases(&self) -> BTreeMap<String, Vec<String>> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .aliases
            .iter()
            .map(|(alias, indices)| (alias.clone(), indices.keys().cloned().collect()))
            .collect()
    }

    pub fn all_mappings(&self) -> BTreeMap<String, Value> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .iter()
            .map(|(index, data)| (index.clone(), data.mapping_value()))
            .collect()
    }

    pub fn get_document(&self, index: &str, id: &str) -> Option<Value> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .and_then(|data| data.documents.get(id).map(|source| (**source).clone()))
    }

    pub fn documents(&self, index: &str) -> Vec<StoredDocument> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .into_iter()
            .flat_map(|data| {
                data.documents.iter().map(|(id, source)| StoredDocument {
                    index: index.to_owned(),
                    id: id.clone(),
                    // Refcount bump per hit, not a JSON deep clone.
                    source: Arc::clone(source),
                })
            })
            .collect()
    }

    /// Number of stored documents in `index`, or 0 when the index does
    /// not exist. Avoids the O(N) clone that `documents(index).len()`
    /// would incur — the `match_all` hot path uses this to compute
    /// `total` without materialising every `_source`.
    pub fn document_count(&self, index: &str) -> u64 {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or(0, |data| data.documents.len() as u64)
    }

    /// Returns documents at positions `[from, from + size)` in the
    /// index's stable iteration order (BTreeMap key order on the
    /// public `_id`). Only the requested window is cloned, so the
    /// `match_all` top-K shortcut clones K sources instead of N.
    /// Returns an empty vec when `index` does not exist or when `from`
    /// lands past the last document.
    pub fn documents_paginated(
        &self,
        index: &str,
        from: usize,
        size: usize,
    ) -> Vec<StoredDocument> {
        if size == 0 {
            return Vec::new();
        }
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let Some(data) = store.indices.get(index) else {
            return Vec::new();
        };
        data.documents
            .iter()
            .skip(from)
            .take(size)
            .map(|(id, source)| StoredDocument {
                index: index.to_owned(),
                id: id.clone(),
                // Refcount bump per hit, not a JSON deep clone.
                source: Arc::clone(source),
            })
            .collect()
    }

    pub fn documents_by_ids(&self, index: &str, ids: &[String]) -> Vec<StoredDocument> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let Some(data) = store.indices.get(index) else {
            return Vec::new();
        };

        ids.iter()
            .filter_map(|id| {
                data.documents.get(id).map(|source| StoredDocument {
                    index: index.to_owned(),
                    id: id.clone(),
                    // Refcount bump per hit, not a JSON deep clone.
                    source: Arc::clone(source),
                })
            })
            .collect()
    }

    pub fn documents_for_term(&self, index: &str, field: &str, value: &str) -> Vec<String> {
        // Lot 1.6: lazy FST rebuild before the read sees `terms`.
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or_else(Vec::new, |data| data.term_hits(field, value))
    }

    /// Optimisation #10: internal `u32` doc-ids for a term (no `String` clone).
    pub fn documents_for_term_internal(&self, index: &str, field: &str, value: &str) -> Vec<u32> {
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or_else(Vec::new, |data| data.term_hits_internal(field, value))
    }

    /// Optimisation #10: internal `u32` doc-ids for a prefix (no `String` clone).
    pub fn documents_for_prefix_internal(
        &self,
        index: &str,
        field: &str,
        prefix: &str,
    ) -> Option<Vec<u32>> {
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .and_then(|data| data.prefix_hits_internal(field, prefix))
    }

    /// A6 phase 2: postings-backed prefix lookup. Returns `Some(ids)` iff
    /// `field` declares `index_prefixes` AND the prefix length falls in the
    /// `[min_chars..=max_chars]` window — in that case the result is the
    /// exact set of matching document ids. Returns `None` when the
    /// accelerated path is not applicable, in which case the caller must
    /// fall back to the source-scan path.
    pub fn documents_for_prefix(
        &self,
        index: &str,
        field: &str,
        prefix: &str,
    ) -> Option<Vec<String>> {
        // Lot 1.6: prefix-hits walks the FST range for keyword/date
        // fields without `index_prefixes`, so terms must be live.
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .and_then(|data| data.prefix_hits(field, prefix))
    }

    pub fn documents_for_match(
        &self,
        index: &str,
        field: &str,
        value: &str,
        require_all_terms: bool,
    ) -> Vec<String> {
        // Lot 1.6: match_hits consumes the FST postings.
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.get(index).map_or_else(Vec::new, |data| {
            data.match_hits(field, value, require_all_terms)
        })
    }

    pub fn documents_for_match_internal(
        &self,
        index: &str,
        field: &str,
        value: &str,
        require_all_terms: bool,
    ) -> Vec<u32> {
        // Lot 1.6: same as `documents_for_match` — consumes FST postings.
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.get(index).map_or_else(Vec::new, |data| {
            data.match_hits_internal(field, value, require_all_terms)
        })
    }

    /// Optimisation #11: leapfrog conjunction over single-term clauses.
    /// `clauses` is the FULL conjunction — every `(field, value)` must match.
    /// Returns `Some(intersected internal doc-ids)` when EVERY clause analyses
    /// to exactly one term (so it maps to a single posting list and the
    /// galloping walk applies); `None` when any clause is multi-token or empty,
    /// in which case the caller falls back to the generic `BTreeSet` candidate
    /// path. Parity: the result equals the intersection of the clauses' match
    /// sets — `conjunction_hits_internal` enforces that.
    pub fn conjunction_leapfrog(&self, index: &str, clauses: &[(&str, &str)]) -> Option<Vec<u32>> {
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let data = store.indices.get(index)?;
        let mut terms: Vec<(String, String)> = Vec::with_capacity(clauses.len());
        for &(field, value) in clauses {
            let mut toks = normalized_terms_for_field(value, field, &data.mapping);
            // Single-token only: a multi-token match is a per-token OR/AND that
            // does not reduce to one posting list — let the caller fall back.
            if toks.len() != 1 {
                return None;
            }
            terms.push((field.to_string(), toks.remove(0)));
        }
        Some(data.conjunction_hits_internal(&terms))
    }

    pub fn documents_by_internal_ids(
        &self,
        index: &str,
        internal_ids: &[u32],
    ) -> Vec<StoredDocument> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.get(index).map_or_else(Vec::new, |data| {
            data.documents_by_internal_ids(index, internal_ids)
        })
    }

    pub fn internal_doc_ids(&self, index: &str, public_ids: &[&str]) -> Vec<Option<u32>> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let Some(data) = store.indices.get(index) else {
            return vec![None; public_ids.len()];
        };
        public_ids
            .iter()
            .map(|id| data.document_ids.get(*id).copied())
            .collect()
    }

    pub fn term_scoring_stats(&self, index: &str, field: &str, term: &str) -> TermScoringStats {
        // Lot 1.6: scoring stats read `block_metas` + `postings` from
        // the FST; rebuild before snapshotting if writes are pending.
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or_else(TermScoringStats::default, |data| {
                data.term_scoring_stats(field, term)
            })
    }

    /// Optimisation #7 + #8: run `f` against a single scoped read guard.
    ///
    /// `ensure_terms_ready` is invoked FIRST (it may take the write lock to
    /// materialise the deferred FST term dictionary) so the subsequent read
    /// guard is held over a consistent, materialised snapshot. Because
    /// `std::sync::RwLock` is writer-preferring and non-reentrant, doing the
    /// (possibly write-locking) materialisation before acquiring the read
    /// guard is what keeps this deadlock-free: `f` must never call back into
    /// an `AppState` method that takes `store.read()` or `store.write()`
    /// while the guard is live — it should read exclusively through the
    /// [`IndexReader`] it is handed.
    ///
    /// The closure receives `Some(reader)` when `index` exists, `None`
    /// otherwise. Threading the whole query (candidate resolution, scoring
    /// context, term-stats lookup, hydration) through this single guard
    /// collapses the prior `~2N+` per-query read-lock acquisitions (one per
    /// scoring-stats lookup, each also re-running `ensure_terms_ready`, plus
    /// candidate + hydration reads) down to one materialise + one read.
    pub fn with_search_reader<R>(
        &self,
        index: &str,
        f: impl FnOnce(Option<IndexReader<'_>>) -> R,
    ) -> R {
        // Materialise the deferred FST build up front (may write-lock).
        // MUST happen before we hold the read guard below: the lock is
        // non-reentrant and writer-preferring, so taking the write lock
        // while a read guard is live would deadlock.
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let reader = store
            .indices
            .get(index)
            .map(|data| IndexReader { index, data });
        f(reader)
    }

    /// Approximate memory usage for `index`. Returns `None` when the
    /// index does not exist. `stored_fields_bytes` is filled here from
    /// the API-owned `_source` documents (which live outside
    /// [`DocumentIndex`]).
    ///
    /// Lot 1.6: the postings accounting walks the FST term dictionary,
    /// so we materialize any pending deferred build before snapshotting.
    /// Callers on the bulk hot path that don't need accurate gauges
    /// between writes should skip this method until `_refresh`.
    pub fn index_memory_usage(&self, index: &str) -> Option<MemoryUsage> {
        // Lot 1.6: rebuild the FST if writes are pending so the
        // accounting walk does not see a stale snapshot. The
        // `bulk_router_*` path skips `refresh_memory_gauges` between
        // chunks for precisely this reason.
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let data = store.indices.get(index)?;
        let mut usage = document_index_memory_usage(&data.index);
        // Each stored payload is counted once regardless of the
        // outstanding `Arc` strong count: the gauge tracks the
        // unique RAM held by `_source` JSON, not the per-reader
        // cumulative footprint.
        usage.stored_fields_bytes =
            stored_fields_bytes_for(data.documents.values().map(Arc::as_ref));
        Some(usage)
    }

    /// Doc count for `index`. Returns `None` for an unknown index, so
    /// callers can distinguish "missing" from "empty".
    pub fn index_doc_count(&self, index: &str) -> Option<u64> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map(|data| data.documents.len() as u64)
    }

    pub fn term_matches_count(&self, index: &str, field: &str, value: &str) -> usize {
        // Lot 1.6: term_hits uses `index.postings(...)`.
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or(0, |data| data.count_term_hits(field, value))
    }
}

fn create_index_if_missing(
    store: &mut MemoryStore,
    index: &str,
    explicit_mapping: IndexMapping,
    explicit_settings: Value,
    explicit_aliases: BTreeMap<String, Value>,
) {
    if store.indices.contains_key(index) {
        return;
    }

    let templates = matching_index_templates(index, &store.index_templates);
    let defaults = template_defaults_for_new_index(&templates);
    let mut mapping = defaults.mapping;
    merge_mapping_fields(&mut mapping, &explicit_mapping);
    let mut settings = defaults.settings;
    merge_settings(&mut settings, &explicit_settings);
    let mut aliases = defaults.aliases;
    aliases.extend(explicit_aliases);
    store
        .indices
        .insert(index.to_owned(), InMemoryIndex::new(mapping, settings));

    for (alias, definition) in aliases {
        store
            .aliases
            .entry(alias)
            .or_default()
            .insert(index.to_owned(), definition);
    }
}

fn matching_index_templates<'a>(
    index: &str,
    index_templates: &'a BTreeMap<String, StoredIndexTemplate>,
) -> Vec<(&'a String, &'a StoredIndexTemplate)> {
    let mut matching_templates = index_templates
        .iter()
        .filter(|(_, template)| {
            template
                .index_patterns
                .iter()
                .any(|pattern| index_pattern_matches(pattern, index))
        })
        .collect::<Vec<_>>();

    matching_templates.sort_by(|(left_name, left), (right_name, right)| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left_name.cmp(right_name))
    });
    matching_templates
}

#[derive(Default)]
struct TemplateDefaults {
    mapping: IndexMapping,
    settings: Value,
    aliases: BTreeMap<String, Value>,
}

fn template_defaults_for_new_index(
    matching_templates: &[(&String, &StoredIndexTemplate)],
) -> TemplateDefaults {
    let mut defaults = TemplateDefaults {
        mapping: IndexMapping::default(),
        settings: Value::Object(Default::default()),
        aliases: BTreeMap::new(),
    };

    for (_, template) in matching_templates {
        merge_mapping_fields(&mut defaults.mapping, &template.mapping);
        merge_settings(&mut defaults.settings, &template.settings);
        defaults.aliases.extend(template.aliases.clone());
    }
    defaults
}

fn snapshot_component_templates(
    template: &mut StoredIndexTemplate,
    component_templates: &BTreeMap<String, StoredComponentTemplate>,
) -> Result<(), String> {
    let inline_mapping = template.mapping.clone();
    let inline_settings = template.settings.clone();
    let inline_aliases = template.aliases.clone();

    template.mapping = IndexMapping::default();
    template.settings = Value::Object(Default::default());
    template.aliases.clear();

    for component_name in &template.composed_of {
        let component = component_templates
            .get(component_name)
            .ok_or_else(|| component_name.clone())?;
        merge_mapping_fields(&mut template.mapping, &component.mapping);
        merge_settings(&mut template.settings, &component.settings);
        template.aliases.extend(component.aliases.clone());
    }

    merge_mapping_fields(&mut template.mapping, &inline_mapping);
    merge_settings(&mut template.settings, &inline_settings);
    template.aliases.extend(inline_aliases);
    Ok(())
}

fn merge_mapping_fields(target: &mut IndexMapping, source: &IndexMapping) {
    for (field, mapping) in source.fields() {
        target.set_field_mapping(field.to_owned(), mapping.clone());
    }
    // A1/A13: carry the `settings.analysis` block (edge_ngram tokenizers,
    // user-defined analyzers/normalizers) onto the stored mapping so the
    // custom analyzers its fields reference resolve at index + query time.
    // Without this the create path dropped analysis and edge_ngram
    // sub-fields silently fell back to the default analyzer.
    let analysis = source.analysis();
    if analysis != &AnalysisSettings::default() {
        target.set_analysis(analysis.clone());
    }
}

fn merge_settings(target: &mut Value, source: &Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) else {
        return;
    };

    for (key, value) in source {
        match (target.get_mut(key), value) {
            (Some(target_value), Value::Object(_)) if target_value.is_object() => {
                merge_settings(target_value, value);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn index_pattern_matches(pattern: &str, index: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let index = index.chars().collect::<Vec<_>>();
    let mut matches = vec![vec![false; index.len() + 1]; pattern.len() + 1];
    matches[0][0] = true;

    for pattern_index in 1..=pattern.len() {
        if pattern[pattern_index - 1] == '*' {
            matches[pattern_index][0] = matches[pattern_index - 1][0];
        }
    }

    for pattern_index in 1..=pattern.len() {
        for index_index in 1..=index.len() {
            matches[pattern_index][index_index] = match pattern[pattern_index - 1] {
                '*' => {
                    matches[pattern_index - 1][index_index]
                        || matches[pattern_index][index_index - 1]
                }
                '?' => matches[pattern_index - 1][index_index - 1],
                character => {
                    character == index[index_index - 1]
                        && matches[pattern_index - 1][index_index - 1]
                }
            };
        }
    }

    matches[pattern.len()][index.len()]
}
