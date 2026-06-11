use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use rayon::prelude::*;
use surch_analysis::{
    Analyzer, KeywordAnalyzer, NormAnalyzer, Normalizer, SimpleAnalyzer, StandardAnalyzer,
    StopAnalyzer, WhitespaceAnalyzer,
};
use thiserror::Error;

use crate::mapping::{AnalysisSettings, AnalyzerName, FieldType, IndexMapping};
use crate::postings::{
    BlockMeta, PostingsBuilder, PostingsEnum, PostingsError, PostingsList, TermDictionary,
    TermsEnum,
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
    /// sub-field's analyzer/normalizer already applied (see
    /// [`subfield_terms`]).
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

/// Lucene-compatible 1-byte length quantization used by [`FieldLengthStats`]
/// to store the per-doc field length the BM25 scorer reads. Bit-identical
/// mirror of `surch_search::small_float`; the canonical reference, full
/// test vectors and documentation live there. We duplicate the encoder
/// here only because `surch-search` already depends on `surch-index`
/// (cannot import upward); a CI parity test
/// (`crates/surch-search/tests/small_float_parity.rs`) asserts the two
/// implementations produce byte-identical output for every input the
/// indexer can record.
mod small_float {
    /// Lucene `NUM_FREE_VALUES = 255 - longToInt4(Integer.MAX_VALUE) = 24`.
    /// Inputs strictly below this round-trip lossless.
    const NUM_FREE_VALUES: u32 = 24;

    #[inline]
    fn long_to_int4(value: u64) -> u32 {
        let num_bits = 64 - value.leading_zeros();
        if num_bits < 4 {
            return value as u32;
        }
        let shift = num_bits - 4;
        let mantissa = (value >> shift) as u32 & 0x07;
        let exponent = shift + 1;
        mantissa | (exponent << 3)
    }

    #[inline]
    fn int4_to_long(encoded: u32) -> u64 {
        let bits = (encoded & 0x07) as u64;
        let exp = encoded >> 3;
        if exp == 0 {
            bits
        } else {
            (bits | 0x08) << (exp - 1)
        }
    }

    #[inline]
    pub(super) fn int_to_byte4(value: u32) -> u8 {
        if value < NUM_FREE_VALUES {
            return value as u8;
        }
        let offset = (value - NUM_FREE_VALUES) as u64;
        let encoded = long_to_int4(offset);
        (NUM_FREE_VALUES + encoded) as u8
    }

    #[inline]
    pub(super) fn byte4_to_int(byte: u8) -> u32 {
        let i = byte as u32;
        if i < NUM_FREE_VALUES {
            return i;
        }
        let decoded = NUM_FREE_VALUES as u64 + int4_to_long(i - NUM_FREE_VALUES);
        decoded.min(u32::MAX as u64) as u32
    }
}

/// Per-field length statistics recorded during analysis for BM25 norms.
///
/// `doc_len_dense` is indexed directly by internal `doc_id` (which is dense:
/// `0..n`), with `0` as the "absent" sentinel — a real recorded `doc_len`
/// always quantizes to a non-zero byte (`record_doc_len` skips `0` raw
/// lengths and `int_to_byte4(1) = 1`).
///
/// Each entry is a **Lucene `SmallFloat`-quantized 1-byte length** (mirrors
/// `BM25Similarity.computeNorm`), not the raw token count. This:
///
/// 1. closes the TREC-COVID NDCG@10 parity gap (Surch 0.4750 → ≥ 0.4902 OS,
///    see `docs/paper/ndcg-trec-covid-rootcause-22.md` #22 — the gap is
///    entirely caused by Surch scoring with exact `doc_len` while Lucene
///    scored with the quantized bucket, so the top-K ordering inside the
///    same candidate set diverged);
/// 2. cuts `field_stats_bytes` from 8 B/doc to 1 B/doc — ~65 MiB freed on
///    the deces 1.36 M × ~6 indexed fields corpus, half of the
///    `field_stats` memory ledger.
///
/// `total_terms` continues to track the **raw** sum (so `avg_doc_len`
/// matches Lucene's `CollectionStatistics.sumTotalTermFreq / docCount`,
/// which is unaffected by per-doc quantization).
///
/// `doc_len(doc_id)` returns the quantized-then-reconstructed length —
/// the same value Lucene feeds to BM25 — so callers see the SAME number
/// Lucene would on the same input. Same for `min_doc_len()`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FieldLengthStats {
    pub doc_count: u64,
    pub total_terms: u64,
    /// One byte per `doc_id`, `0` = absent, otherwise the Lucene
    /// `SmallFloat` encoded length (see [`small_float`]).
    doc_len_dense: Vec<u8>,
    /// Smallest **reconstructed** `doc_len` (`0` = none), maintained
    /// incrementally so the WAND block-max upper bound does not re-scan
    /// the whole dense slice per query. Kept in the same domain as
    /// `doc_len(doc_id)` so the scorer can feed it directly to
    /// `Bm25TermScorer::score`.
    min_doc_len: u64,
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
        let idx = doc_id as usize;
        if idx >= self.doc_len_dense.len() {
            self.doc_len_dense.resize(idx + 1, 0);
        }
        // Saturate the raw length into the `u32` domain the Lucene
        // encoder expects — field lengths are bounded by token counts
        // in a single doc, which never approach `u32::MAX` in
        // practice; saturation is a defensive no-op that keeps the
        // function infallible.
        let raw = doc_len.min(u32::MAX as u64) as u32;
        let encoded = small_float::int_to_byte4(raw);
        let previous_byte = self.doc_len_dense[idx];
        let previous_reconstructed = if previous_byte == 0 {
            0
        } else {
            small_float::byte4_to_int(previous_byte) as u64
        };
        if previous_byte != 0 {
            // We do not store the raw `doc_len` (only the quantized
            // byte), so on the rare in-place overwrite path we
            // subtract the reconstructed bucket length. The bulk-load
            // path never overwrites (each `doc_id` is recorded once,
            // `previous_byte` is `0`), so this branch is dead in
            // production. On the rare update path the drift is
            // bounded by one bucket of quantization error on a single
            // doc — negligible vs `total_terms` of the whole corpus.
            self.total_terms = self.total_terms.saturating_sub(previous_reconstructed);
        } else {
            self.doc_count += 1;
        }
        self.doc_len_dense[idx] = encoded;
        self.total_terms += doc_len;
        let reconstructed = small_float::byte4_to_int(encoded) as u64;
        // Maintain the running minimum on the reconstructed (Lucene-
        // bucketed) length, so it matches the value `doc_len(doc_id)`
        // returns and the scorer consumes directly. The common path
        // (lower minimum, or first record) is O(1); the rare overwrite
        // that RAISES the minimum forces a recompute scan.
        if self.min_doc_len == 0 || reconstructed < self.min_doc_len {
            self.min_doc_len = reconstructed;
        } else if previous_reconstructed == self.min_doc_len
            && reconstructed > previous_reconstructed
        {
            self.min_doc_len = self
                .doc_len_dense
                .iter()
                .copied()
                .filter(|&byte| byte > 0)
                .map(|byte| small_float::byte4_to_int(byte) as u64)
                .min()
                .unwrap_or(0);
        }
    }

    pub fn avg_doc_len(&self) -> Option<f64> {
        (self.doc_count > 0 && self.total_terms > 0)
            .then(|| self.total_terms as f64 / self.doc_count as f64)
    }

    /// The smallest reconstructed `doc_len` (Lucene quantized, O(1)
    /// lookup), or `None` when no length was recorded (e.g. norms
    /// disabled).
    pub fn min_doc_len(&self) -> Option<u64> {
        (self.min_doc_len > 0).then_some(self.min_doc_len)
    }

    /// Lucene-quantized `doc_len` for `doc_id`, or `None` when no
    /// length was recorded. Same value `BM25Similarity` would feed its
    /// scoring formula on the same input.
    pub fn doc_len(&self, doc_id: u32) -> Option<u64> {
        self.doc_len_dense
            .get(doc_id as usize)
            .copied()
            .filter(|&byte| byte > 0)
            .map(|byte| small_float::byte4_to_int(byte) as u64)
    }

    /// The dense per-`doc_id` quantized length byte slice (`0` =
    /// absent), for zero-copy consumption by the scoring context.
    /// Indexed by `doc_id`. Each byte is the Lucene `SmallFloat`
    /// encoding; callers must reconstruct via
    /// [`decode_doc_len_byte`] before feeding it to BM25.
    pub fn doc_len_dense(&self) -> &[u8] {
        &self.doc_len_dense
    }
}

/// Decode a single `doc_len_dense` byte to its reconstructed Lucene
/// `SmallFloat` length. Returns `0` when the byte is `0` (absent),
/// otherwise the same value `doc_len(doc_id)` would return.
#[inline]
pub fn decode_doc_len_byte(byte: u8) -> u64 {
    if byte == 0 {
        0
    } else {
        small_float::byte4_to_int(byte) as u64
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
        let documents = documents
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
            .collect::<Result<Vec<(u32, Vec<(String, String)>)>>>()?;

        // Track A: parallelise the CPU-heavy per-document analysis (tokenise,
        // asciifold/lowercase, prefix fan-out, sub-field re-analysis) across
        // cores with rayon — it is a pure function of (doc, mapping). Only the
        // merge into the shared `postings_builder`/side-tables stays serial.
        // Document order is preserved by `collect`, so the postings are
        // byte-identical to the previous serial path (parity-preserving).
        let analyzed: Vec<AnalyzedDocument> = documents
            .into_par_iter()
            .map(|(doc_id, fields)| analyze_document(doc_id, &fields, mapping))
            .collect();
        for document in analyzed {
            self.merge_analyzed(document)?;
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

    /// Runtime view that ties a term's postings to its FoR-aligned block
    /// metadata in a single lookup. The search scoring path prefers this
    /// over separate [`postings`]/[`block_metas`] calls so it can borrow
    /// both zero-copy from the live term dictionary.
    pub fn postings_with_block_metas(&self, field: &str, term: &str) -> Option<PostingsList<'_>> {
        self.terms.postings_with_block_metas(field, term)
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

    /// #17 memory accounting: total FST byte size across fields.
    pub fn fst_bytes(&self) -> u64 {
        self.terms.fst_bytes()
    }

    /// #17 memory accounting: total bytes held by precomputed roaring bitmaps.
    pub fn roaring_bytes(&self) -> u64 {
        self.terms.roaring_bytes()
    }

    /// #17 memory accounting: per-term `Vec<BlockMeta>` capacity bytes.
    pub fn block_metas_bytes(&self) -> u64 {
        self.terms.block_metas_bytes()
    }

    /// #17c memory accounting: Vec capacity slack across every term's
    /// `Vec<Posting>` and `Vec<u32>` channels. Surfaces the bytes
    /// allocated-but-unused after the FST build — typically up to ~50 %
    /// of the live `postings_bytes` because of `Vec`'s geometric growth
    /// (~doubling) leaving the last realloc half-filled.
    pub fn postings_capacity_slack_bytes(&self) -> u64 {
        self.terms.postings_capacity_slack_bytes()
    }

    /// #17c memory accounting: taille on-heap du `PostingsBuilder` retenu.
    /// Lot 1.5 garde le builder live entre rebuilds incrémentaux, donc
    /// pour 1.36 M docs ça peut peser GROS et n'était pas compté ailleurs.
    /// Suspect #1 du gap heap ~4 GiB sur deces (cf docs/paper/scoreboard-2026-06-10-mesured.md).
    pub fn postings_builder_bytes(&self) -> u64 {
        self.postings_builder.memory_bytes()
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

    /// Serial merge of a pre-analyzed document (produced off-lock by the pure
    /// `analyze_document`) into the shared postings builder and side-tables.
    /// This is the ONLY part of bulk indexing that mutates shared state, so it
    /// stays serial; the CPU-heavy analysis runs in parallel beforehand. The
    /// merge is cheap (inserts of already-computed terms) and preserves the
    /// previous serial path's output exactly (documents merged in input order).
    fn merge_analyzed(&mut self, document: AnalyzedDocument) -> Result<()> {
        let doc_id = document.doc_id;
        for (path, stored) in document.subfield_values {
            self.subfield_values
                .entry(path)
                .or_default()
                .insert(doc_id, stored);
        }
        for (field, prefix) in document.prefixes {
            self.prefix_postings
                .entry(field)
                .or_default()
                .entry(prefix)
                .or_default()
                .insert(doc_id);
        }
        for (field, term, positions) in document.postings {
            self.postings_builder.add(field, term, doc_id, positions)?;
        }
        for (field, doc_len, norms_enabled) in document.field_lengths {
            self.field_stats.entry(field).or_default().record_doc_len(
                doc_id,
                doc_len,
                norms_enabled,
            );
        }
        self.live_docs.insert(doc_id);
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
/// Pure, off-lock analysis output for one document. Produced in parallel by
/// [`analyze_document`] and merged serially by `DocumentIndex::merge_analyzed`.
struct AnalyzedDocument {
    doc_id: u32,
    /// Main + sub-field postings: `(field path, term, positions)`.
    postings: Vec<(String, String, Vec<u32>)>,
    /// `index_prefixes` fan-out: `(field, prefix)`.
    prefixes: Vec<(String, String)>,
    /// Sub-field stored projections: `(qualified path, stored value)`.
    subfield_values: Vec<(String, String)>,
    /// Per-field length stats: `(field, doc_len, norms_enabled)`.
    field_lengths: Vec<(String, u64, bool)>,
}

/// Analyze one document into an [`AnalyzedDocument`] without touching any shared
/// index state — a pure function of `(doc, mapping)`, so it runs in parallel
/// across documents. Mirrors the previous serial `add_validated_document`
/// exactly (same tokens, prefix fan-out, sub-field projections); only the
/// execution is parallelized, the merged index is byte-identical.
fn analyze_document(
    doc_id: u32,
    fields: &[(String, String)],
    mapping: &IndexMapping,
) -> AnalyzedDocument {
    let mut field_lengths = BTreeMap::<String, (u64, bool)>::new();
    let mut postings: Vec<(String, String, Vec<u32>)> = Vec::new();
    let mut prefixes: Vec<(String, String)> = Vec::new();
    let mut subfield_values: Vec<(String, String)> = Vec::new();

    for (field, value) in fields {
        let field_mapping = mapping.field(field);
        let analyzer = field_mapping.map_or(AnalyzerName::default_for(FieldType::Text), |field| {
            field.analyzer()
        });
        let norms_enabled = field_mapping.is_none_or(|field| field.norms_enabled());
        let index_prefixes = field_mapping.and_then(|m| m.index_prefixes);

        let analyzed = analyzed_terms(analyzer, value);
        let token_count = analyzed
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

        // A6 phase 2: prefix fan-out (formerly `index_prefix_terms`).
        if let Some(pfx) = index_prefixes {
            for term in analyzed.keys() {
                let chars: Vec<char> = term.chars().collect();
                let token_len = chars.len();
                if token_len < pfx.min_chars {
                    continue;
                }
                let upper = token_len.min(pfx.max_chars);
                for k in pfx.min_chars..=upper {
                    prefixes.push((field.clone(), chars[..k].iter().collect()));
                }
            }
        }

        // A10 (Phase 4): multi-field sub-field fan-out (formerly `index_subfields`).
        if let Some(field_mapping) = field_mapping {
            if field_mapping.has_subfields() {
                for (sub_name, sub_mapping) in field_mapping.subfields() {
                    let path = format!("{field}.{sub_name}");
                    let analyzed_sub = subfield_terms(sub_mapping, value, mapping.analysis());
                    if let Some(stored) = lowest_position_term(&analyzed_sub) {
                        subfield_values.push((path.clone(), stored));
                    }
                    for (term, positions) in analyzed_sub {
                        postings.push((path.clone(), term, positions));
                    }
                }
            }
        }

        for (term, positions) in analyzed {
            postings.push((field.clone(), term, positions));
        }
    }

    let field_lengths = field_lengths
        .into_iter()
        .map(|(field, (doc_len, norms_enabled))| (field, doc_len, norms_enabled))
        .collect();

    AnalyzedDocument {
        doc_id,
        postings,
        prefixes,
        subfield_values,
        field_lengths,
    }
}

fn lowest_position_term(analyzed: &BTreeMap<String, Vec<u32>>) -> Option<String> {
    analyzed
        .iter()
        .filter_map(|(term, positions)| positions.iter().min().map(|pos| (*pos, term.clone())))
        .min_by_key(|(pos, _)| *pos)
        .map(|(_, term)| term)
}

/// Optimisation #2: the per-doc term map is keyed on the **term only** — the
/// field name is constant for the whole call and is attached once per unique
/// term by the caller when it emits postings. The previous
/// `entry((field.to_owned(), term))` cloned the field `String` on EVERY token
/// (O(tokens), even repeated tokens that collapse into one entry) and carried
/// the field bytes through every BTreeMap key comparison. Matches Lucene's
/// single `FieldInvertState` (field identity never re-materialised per token).
fn analyzed_terms(analyzer: AnalyzerName, value: &str) -> BTreeMap<String, Vec<u32>> {
    let tokenized = match analyzer {
        AnalyzerName::Standard => StandardAnalyzer.token_stream(value),
        AnalyzerName::Simple => SimpleAnalyzer.token_stream(value),
        AnalyzerName::Stop => StopAnalyzer.token_stream(value),
        AnalyzerName::Keyword => KeywordAnalyzer.token_stream(value),
        AnalyzerName::Whitespace => WhitespaceAnalyzer.token_stream(value),
        AnalyzerName::Norm => NormAnalyzer.token_stream(value),
    };

    let mut terms = BTreeMap::<String, Vec<u32>>::new();
    let mut position = 0_u32;

    for token in tokenized {
        position += token.position_increment;
        terms.entry(token.term).or_default().push(position - 1);
    }

    terms
}

/// A10 (Phase 4): analyze the parent `value` for a multi-field sub-field,
/// keyed by the qualified `field` path.
///
/// Routes the value through the sub-field's analysis chain with ES-faithful
/// semantics:
///
/// - `keyword` + `normalizer`: the WHOLE value as one token, lowercased +
///   asciifolded via [`Normalizer`]. A normalizer in ES is a char/token
///   filter chain applied to the single keyword token — it never tokenizes,
///   so `"Étienne DUPRÉ"` stores `"etienne dupre"`, not `["etienne",
///   "dupre"]`.
/// - `keyword` without normalizer: the whole value verbatim as one token
///   (identity, via [`KeywordAnalyzer`]).
/// - any other type (e.g. `text`): the declared / default analyzer, which
///   tokenizes (`analyzed_terms`).
fn subfield_terms(
    sub_mapping: &crate::mapping::FieldMapping,
    value: &str,
    analysis: &AnalysisSettings,
) -> BTreeMap<String, Vec<u32>> {
    if sub_mapping.field_type == FieldType::Keyword {
        let tokens = match sub_mapping.normalizer {
            Some(_) => Normalizer.token_stream(value),
            None => KeywordAnalyzer.token_stream(value),
        };
        let mut terms = BTreeMap::<String, Vec<u32>>::new();
        let mut position = 0_u32;
        for token in tokens {
            position += token.position_increment;
            terms.entry(token.term).or_default().push(position - 1);
        }
        return terms;
    }

    // A1/A13: a text sub-field may declare a custom analyzer (e.g.
    // `autocomplete_analyzer` = edge_ngram + lowercase/asciifolding). Resolve
    // it against the index analysis settings and fan out its emitted terms as
    // postings. Falls back to the builtin analyzer when the name is unknown
    // (resolution returns None) so a bad reference degrades gracefully.
    if let Some(name) = &sub_mapping.custom_analyzer {
        if let Some(resolved) = analysis.resolve_analyzer(name) {
            let mut terms = BTreeMap::<String, Vec<u32>>::new();
            for (position, term) in resolved.terms(value).into_iter().enumerate() {
                terms.entry(term).or_default().push(position as u32);
            }
            return terms;
        }
    }

    analyzed_terms(sub_mapping.analyzer(), value)
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

    // ---------------------------------------------------------------------
    // A10 (Phase 4): write-time fan-out of multi-field sub-fields
    // ---------------------------------------------------------------------

    /// matchID's `NOM: { type: text, analyzer: norm, fields: { raw: {
    /// type: keyword, normalizer: norm } } }` (intake §2.12).
    fn nom_multi_field_mapping() -> IndexMapping {
        let mut subfields = BTreeMap::new();
        subfields.insert(
            "raw".to_owned(),
            FieldMapping::new(FieldType::Keyword, None).with_normalizer(Some(AnalyzerName::Norm)),
        );
        let nom =
            FieldMapping::new(FieldType::Text, Some(AnalyzerName::Norm)).with_subfields(subfields);
        let mut mapping = IndexMapping::new();
        mapping.set_field_mapping("NOM", nom);
        mapping
    }

    #[test]
    fn subfield_fanned_out_stores_normalized_keyword_value() {
        // `NOM.raw` is a keyword sub-field with `normalizer: norm`. The
        // write-time fan-out must store the whole parent value lowercased
        // and asciifolded as a single token (parity with ES keyword +
        // normalizer), NOT the per-word `norm`-analyzer tokens of the
        // text parent.
        let mapping = nom_multi_field_mapping();
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("NOM", "Étienne DUPRÉ")], &mapping)
            .expect("doc 1");

        assert!(index.has_subfield_values("NOM.raw"));
        // Whole value, lowercased + asciifolded, single keyword token.
        assert_eq!(index.subfield_value("NOM.raw", 1), Some("etienne dupre"));
        // The parent field is untouched: no stored projection on "NOM".
        assert!(!index.has_subfield_values("NOM"));
        assert!(index.subfield_value("NOM", 1).is_none());
    }

    #[test]
    fn subfield_fanned_out_is_searchable_via_postings() {
        // The fanned-out keyword token is also indexed into the regular
        // postings under the qualified `NOM.raw` path, so a `term` lookup
        // on the sub-field resolves through the FST like any other field.
        let mapping = nom_multi_field_mapping();
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("NOM", "DUPONT")], &mapping)
            .expect("doc 1");
        index
            .add_document_with_mapping(2, [("NOM", "Dupré")], &mapping)
            .expect("doc 2");

        // `NOM.raw` holds the whole normalized value as one keyword token.
        let postings = index
            .postings("NOM.raw", "dupont")
            .expect("NOM.raw=dupont postings present");
        let doc_ids: Vec<u32> = postings.into_iter().map(|p| p.doc_id).collect();
        assert_eq!(doc_ids, vec![1]);

        let postings = index
            .postings("NOM.raw", "dupre")
            .expect("NOM.raw=dupre postings present (asciifolded)");
        let doc_ids: Vec<u32> = postings.into_iter().map(|p| p.doc_id).collect();
        assert_eq!(doc_ids, vec![2]);

        // The parent text field keeps its own `norm`-analyzed postings.
        assert!(index.postings("NOM", "dupont").is_some());
    }

    /// matchID-style autocomplete sub-field: `NOM: { type: text, fields: {
    /// autocomplete: { type: text, analyzer: autocomplete_analyzer } } }`
    /// with `settings.analysis` declaring the edge_ngram tokenizer +
    /// lowercase/asciifolding chain.
    fn nom_autocomplete_mapping() -> IndexMapping {
        let mut subfields = BTreeMap::new();
        subfields.insert(
            "autocomplete".to_owned(),
            FieldMapping::new(FieldType::Text, None)
                .with_custom_analyzer(Some("autocomplete_analyzer".to_owned())),
        );
        let nom =
            FieldMapping::new(FieldType::Text, Some(AnalyzerName::Norm)).with_subfields(subfields);
        let mut mapping = IndexMapping::new();
        mapping.set_field_mapping("NOM", nom);
        let analysis = IndexMapping::from_index_settings_value(&serde_json::json!({
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
        }))
        .expect("analysis settings parse");
        mapping.set_analysis(analysis);
        mapping
    }

    #[test]
    fn edge_ngram_subfield_fans_out_prefix_postings() {
        // A1/A13: a text sub-field with a custom `autocomplete_analyzer`
        // (edge_ngram min 2 / max 4 + lowercase + asciifolding) must index
        // every prefix of the parent value as postings under the qualified
        // `NOM.autocomplete` path.
        let mapping = nom_autocomplete_mapping();
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("NOM", "Dupont")], &mapping)
            .expect("doc 1");

        // Prefixes of "Dupont" length 2..=4, lowercased + asciifolded.
        for prefix in ["du", "dup", "dupo"] {
            let postings = index
                .postings("NOM.autocomplete", prefix)
                .unwrap_or_else(|| panic!("NOM.autocomplete={prefix} postings present"));
            let doc_ids: Vec<u32> = postings.into_iter().map(|p| p.doc_id).collect();
            assert_eq!(doc_ids, vec![1], "prefix {prefix}");
        }
        // max_gram is 4, so length-5/6 prefixes are NOT emitted.
        assert!(index.postings("NOM.autocomplete", "dupon").is_none());
        assert!(index.postings("NOM.autocomplete", "dupont").is_none());
        // The parent text field keeps its own `norm`-analyzed postings.
        assert!(index.postings("NOM", "dupont").is_some());
    }

    #[test]
    fn no_subfield_fan_out_when_field_has_no_subfields() {
        // A plain text field declares no `fields:` block => the side-table
        // stays empty and `term` lookups on a fabricated `.raw` path miss.
        let mut mapping = IndexMapping::new();
        mapping.set_field_mapping("NOM", FieldMapping::new(FieldType::Text, None));
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("NOM", "DUPONT")], &mapping)
            .expect("doc 1");

        assert!(!index.has_subfield_values("NOM.raw"));
        assert!(index.subfield_value("NOM.raw", 1).is_none());
        assert!(index.subfield_values_map().is_empty());
        assert!(index.postings("NOM.raw", "dupont").is_none());
    }

    #[test]
    fn keyword_subfield_without_normalizer_stores_verbatim_value() {
        // A keyword sub-field with no `normalizer` stores the untouched
        // value as a single token (keyword analyzer is identity), so case
        // and diacritics are preserved.
        let mut subfields = BTreeMap::new();
        subfields.insert(
            "exact".to_owned(),
            FieldMapping::new(FieldType::Keyword, None),
        );
        let nom = FieldMapping::new(FieldType::Text, None).with_subfields(subfields);
        let mut mapping = IndexMapping::new();
        mapping.set_field_mapping("NOM", nom);

        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("NOM", "Dupré Martin")], &mapping)
            .expect("doc 1");

        assert_eq!(index.subfield_value("NOM.exact", 1), Some("Dupré Martin"));
    }

    #[test]
    fn subfield_fan_out_cleared_on_index_clear() {
        // `clear()` must drop the sub-field side-table so a fresh
        // generation does not leak the previous projections.
        let mapping = nom_multi_field_mapping();
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("NOM", "DUPONT")], &mapping)
            .expect("doc 1");
        assert!(index.has_subfield_values("NOM.raw"));

        index.clear();
        assert!(!index.has_subfield_values("NOM.raw"));
        assert!(index.subfield_value("NOM.raw", 1).is_none());
        assert!(index.subfield_values_map().is_empty());
    }
}
