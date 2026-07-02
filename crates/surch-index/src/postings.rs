use std::collections::{BTreeMap, BTreeSet};

use fst::{IntoStreamer, Map, MapBuilder, Streamer};
use surch_codec::postings_block::{
    BlockSkipCursor, BlockSkipEntry, BlockSkipList, PostingsBlockError, FOR_BLOCK_SIZE,
};

use crate::roaring::RoaringDocSet;

/// A1: build a roaring/hybrid bitmap only for terms with more than this many
/// postings. Below it, the galloping leapfrog already wins and the bitmap RAM
/// is not worth it; above it (the common-name tail), the word-parallel AND is
/// the bool/full gap-closer. The container choice (dense bitmap vs sparse
/// array) is then made per 65 536-chunk inside [`RoaringDocSet`].
const TERM_ROARING_THRESHOLD: usize = 4_096;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, PostingsError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PostingsError {
    #[error("field name must not be empty")]
    EmptyField,
    #[error("term must not be empty")]
    EmptyTerm,
}

/// A single posting: one (doc_id, term_frequency) pair.
///
/// Track A (beat-ES optimisation #9): positions are NOT stored. They are
/// computed during analysis only to derive `freq` (and the field's doc_len),
/// then discarded — no production read path consumes index positions
/// (`match_phrase` re-tokenises `_source`; BM25 reads only `freq`; the
/// persisted codec never wrote positions). Dropping the per-posting
/// `Vec<u32>` shrinks each posting from ~32 B + a heap Vec to a `Copy` 8 B
/// struct, the dominant in-memory RSS term on multi-field large corpora —
/// the in-memory engine's scale juge-de-paix vs ES (which stores positions
/// only when `index_options >= positions`, Lucene-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Posting {
    pub doc_id: u32,
    pub freq: u32,
}

impl Posting {
    pub fn new(doc_id: u32, freq: u32) -> Self {
        Self { doc_id, freq }
    }

    /// Term frequency for a token list (matches the historical semantics: an
    /// empty position list — a single non-positional occurrence — counts as 1).
    pub fn freq_from_positions(positions: &[u32]) -> u32 {
        if positions.is_empty() {
            1
        } else {
            positions.len() as u32
        }
    }
}

/// Per-block statistics computed once at `PostingsBuilder::build()` time,
/// then read directly on the hot scoring path instead of being recomputed
/// at every query (the Block-Max WAND a.k.a. `BlockWAND` schema used by
/// Tantivy / Lucene block-max postings).
///
/// Each `BlockMeta` describes one chunk of up to [`BLOCK_SIZE`] consecutive
/// postings inside a term's `Vec<Posting>` (postings are kept sorted by
/// ascending `doc_id`, so `min_doc_id`/`max_doc_id` are simply the first
/// and last entry of the chunk).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BlockMeta {
    /// Number of postings described by this block.
    pub posting_count: usize,
    /// Greatest term_freq inside this block of up to [`BLOCK_SIZE`] postings.
    pub max_term_freq: u32,
    /// Smallest `doc_id` inside this block (so callers can range-skip).
    pub min_doc_id: u32,
    /// Largest `doc_id` inside this block.
    pub max_doc_id: u32,
}

/// Number of postings per BMW block. Must match the `BLOCK_SIZE` used by
/// `maxscore_match` in `surch-api` — block metas are aligned with the
/// `Vec<Posting>` chunks produced by `Vec::chunks(BLOCK_SIZE)`.
pub const BLOCK_SIZE: usize = FOR_BLOCK_SIZE;

fn build_block_metas(postings: &[Posting]) -> Vec<BlockMeta> {
    postings
        .chunks(BLOCK_SIZE)
        .map(|chunk| {
            // `chunks` only yields non-empty slices, so first/last are safe.
            let min_doc_id = chunk.first().expect("chunk is non-empty").doc_id;
            let max_doc_id = chunk.last().expect("chunk is non-empty").doc_id;
            let max_term_freq = chunk.iter().map(|p| p.freq).max().unwrap_or(0);
            BlockMeta {
                posting_count: chunk.len(),
                max_term_freq,
                min_doc_id,
                max_doc_id,
            }
        })
        .collect()
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PostingsBuilder {
    /// The outer layer stays a `BTreeMap<field_name, …>` because real
    /// indices only have a handful of fields. Inside each field we
    /// accumulate `term -> postings` in a `BTreeMap` (lexicographic
    /// order) so that `build()` can feed `fst::MapBuilder` directly
    /// without an extra sort pass.
    fields: BTreeMap<String, BTreeMap<String, Vec<Posting>>>,
}

impl PostingsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// #17c memory accounting : taille on-heap effective du builder retenu
    /// (Lot 1.5 le garde live entre rebuilds incrémentaux). Walk les
    /// BTreeMap imbriqués + somme `Vec<Posting>` capacities. Le but est
    /// de chiffrer enfin la part de RAM heap (#17b gap ~4 GiB) qui vient
    /// de la version "build snapshot" en attente d'`finalize_postings`.
    /// Read-only, O(field × terms × postings.capacity).
    pub fn memory_bytes(&self) -> u64 {
        use std::mem::size_of;
        let posting_size = size_of::<Posting>() as u64;
        let str_overhead = size_of::<String>() as u64;
        // BTreeMap node overhead approximé : 11 entrées par nœud côté Rust,
        // donc on majore l'estimate plate avec 1.5× pour le slack typique.
        let inner_kv = (str_overhead + size_of::<Vec<Posting>>() as u64) * 3 / 2;
        let outer_kv = (str_overhead + size_of::<BTreeMap<String, Vec<Posting>>>() as u64) * 3 / 2;
        let mut total: u64 = 0;
        for (field, terms) in &self.fields {
            total = total.saturating_add(outer_kv);
            total = total.saturating_add(field.len() as u64);
            for (term, postings) in terms {
                total = total.saturating_add(inner_kv);
                total = total.saturating_add(term.len() as u64);
                total =
                    total.saturating_add((postings.capacity() as u64).saturating_mul(posting_size));
            }
        }
        total
    }

    pub fn add(
        &mut self,
        field: impl Into<String>,
        term: impl Into<String>,
        doc_id: u32,
        positions: Vec<u32>,
    ) -> Result<()> {
        let field = field.into();
        if field.trim().is_empty() {
            return Err(PostingsError::EmptyField);
        }

        let term = term.into();
        if term.trim().is_empty() {
            return Err(PostingsError::EmptyTerm);
        }

        let freq = Posting::freq_from_positions(&positions);
        self.fields
            .entry(field)
            .or_default()
            .entry(term)
            .or_default()
            .push(Posting::new(doc_id, freq));

        Ok(())
    }

    pub fn build(mut self) -> TermDictionary {
        // Sort each posting list by ascending doc_id (the query engine
        // relies on this to do single-pass conjunctions and unions).
        for terms in self.fields.values_mut() {
            for postings in terms.values_mut() {
                postings.sort_by_key(|posting| posting.doc_id);
            }
        }

        let mut fields: BTreeMap<String, FieldPostings> = BTreeMap::new();
        for (field, terms) in self.fields {
            // `BTreeMap` already yields keys in lexicographic order, so
            // we can feed `MapBuilder` directly without an extra sort.
            //
            // The `expect` calls below cannot fire in practice: the
            // in-memory `MapBuilder` writes to a `Vec<u8>` so I/O can
            // never fail, and we feed it strictly increasing keys by
            // construction. If they ever do fire it means we passed
            // the input invariant, which is a programmer bug.
            let mut builder = MapBuilder::memory();

            // --- Pass 1 (Lot C Phase 1 — flatten `FieldPostings`) -------
            // Read-only pass over `terms` (nothing is moved out yet) to
            // size the flat buffers EXACTLY: Σdf for `postings_flat` /
            // `doc_ids_flat`, Σblocks for `block_metas_flat`, and the
            // term count for the CSR offset tables (`T + 1` entries).
            // Exact sizing means `Vec::with_capacity` never triggers a
            // geometric-growth reallocation below, so the final
            // `into_boxed_slice()` is a plain, slack-free conversion.
            let term_count = terms.len();
            let total_postings: usize = terms.values().map(Vec::len).sum();
            let total_blocks: usize = terms
                .values()
                .map(|postings| postings.len().div_ceil(BLOCK_SIZE))
                .sum();

            let mut postings_flat: Vec<Posting> = Vec::with_capacity(total_postings);
            let mut doc_ids_flat: Vec<u32> = Vec::with_capacity(total_postings);
            let mut block_metas_flat: Vec<BlockMeta> = Vec::with_capacity(total_blocks);
            let mut offsets: Vec<u32> = Vec::with_capacity(term_count + 1);
            let mut block_offsets: Vec<u32> = Vec::with_capacity(term_count + 1);
            // Sparse side table: only terms with df > TERM_ROARING_THRESHOLD
            // get an entry, so there is no useful exact size to precompute;
            // `shrink_to_fit()` right before insertion below removes the
            // geometric-growth slack instead.
            let mut roaring: Vec<(u32, RoaringDocSet)> = Vec::new();
            offsets.push(0);
            block_offsets.push(0);

            // --- Pass 2: fill, draining `terms` term-by-term ------------
            // Transitory-peak mitigation (risk #1 of the flattening): a
            // naive "collect everything into Vec<Vec<_>>, then flatten"
            // would briefly hold BOTH the fully-populated builder map AND
            // the fully-populated flat buffers in RAM at once — doubling
            // the peak for the duration of the flatten. Instead,
            // `terms.into_iter()` DRAINS the per-field
            // `BTreeMap<String, Vec<Posting>>` one entry at a time: each
            // term's source `Vec<Posting>` is moved into `term_postings`
            // below and consumed by `postings_flat.extend(term_postings)`
            // at the end of the loop body, which frees that source Vec's
            // heap allocation as soon as its bytes have been copied into
            // the flat buffer. RAM therefore grows monotonically toward
            // `total_postings`, it never double-peaks.
            for (idx, (term, term_postings)) in terms.into_iter().enumerate() {
                builder
                    .insert(term.as_bytes(), idx as u64)
                    .expect("fst::MapBuilder accepts lex-sorted keys");

                // Per-block stats are computed once here, against the
                // ascending-doc_id postings, then read back in O(1) by
                // `maxscore_match` instead of being recomputed at every
                // query.
                block_metas_flat.extend(build_block_metas(&term_postings));
                debug_assert!(
                    block_metas_flat.len() <= u32::MAX as usize,
                    "block_metas_flat offset overflowed u32 — switch block_offsets to u64 \
                     (only relevant well past matchID's 1.36M-doc scale)"
                );
                block_offsets.push(block_metas_flat.len() as u32);

                // Compact doc_id channel for the conjunction leapfrog (same
                // ascending order as `term_postings`, so index-aligned with
                // the `postings_flat` slice pushed below).
                let doc_ids_start = doc_ids_flat.len();
                doc_ids_flat.extend(term_postings.iter().map(|posting| posting.doc_id));

                // A1: precompute a roaring/hybrid bitmap for high-`df` terms so
                // the conjunction of two common terms ANDs word-parallel bitmaps
                // instead of walking O(df_rare) scalar-ly. Below the threshold
                // the galloping leapfrog already wins, so we skip the RAM. We
                // iterate `idx` in strictly increasing order (the drained
                // `BTreeMap` yields terms in FST order), so appending here
                // keeps `roaring` sorted by term idx for free — `lookup`
                // below can `binary_search` it directly.
                if term_postings.len() > TERM_ROARING_THRESHOLD {
                    roaring.push((
                        idx as u32,
                        RoaringDocSet::from_sorted(&doc_ids_flat[doc_ids_start..]),
                    ));
                }

                // AoS postings channel. `extend` consumes `term_postings` by
                // value — this is the last use of the source `Vec<Posting>`
                // the drained `BTreeMap` entry owned, so it is freed right
                // here (see the transitory-peak mitigation note above).
                postings_flat.extend(term_postings);
                debug_assert!(
                    postings_flat.len() <= u32::MAX as usize,
                    "postings_flat offset overflowed u32 — switch offsets to u64 \
                     (only relevant well past matchID's 1.36M-doc scale)"
                );
                offsets.push(postings_flat.len() as u32);
            }
            roaring.shrink_to_fit();

            let bytes = builder
                .into_inner()
                .expect("fst::MapBuilder memory writer never fails I/O");
            let fst = Map::new(bytes).expect("fst::Map from valid MapBuilder bytes");
            fields.insert(
                field,
                FieldPostings {
                    fst,
                    postings_flat: postings_flat.into_boxed_slice(),
                    doc_ids_flat: doc_ids_flat.into_boxed_slice(),
                    block_metas_flat: block_metas_flat.into_boxed_slice(),
                    offsets: offsets.into_boxed_slice(),
                    block_offsets: block_offsets.into_boxed_slice(),
                    roaring,
                },
            );
        }

        TermDictionary { fields }
    }
}

/// Per-field FST term dictionary plus FLAT postings buffers indexed by
/// the FST output (a `u64` we narrow to `usize`) through CSR-style offset
/// tables. The FST shares prefixes between terms (e.g. all the "rue de la
/// X" or "DUPONT" variants in a French civic address corpus) which is
/// where the RAM gain comes from compared to the historical
/// `BTreeMap<String, …>`.
///
/// Lot C Phase 1: `postings`/`doc_ids`/`block_metas` used to be one
/// `Vec<Posting>` / `Vec<u32>` / `Vec<BlockMeta>` PER TERM (a `Vec<Vec<T>>`
/// indexed by FST idx). On the deces 1.36 M-doc corpus that is millions of
/// small heap allocations, each paying a ~24-56 B allocator header plus up
/// to ~50 % geometric-growth slack. They are now ONE allocation per field
/// per channel: every term's postings/doc_ids/block_metas are concatenated,
/// in FST idx order, into `postings_flat` / `doc_ids_flat` /
/// `block_metas_flat`, and `offsets` / `block_offsets` are CSR index
/// tables (`len == T + 1`) such that term `i`'s slice is
/// `buf[offsets[i]..offsets[i+1]]`. This recovers the per-term Vec header
/// overhead and slack while keeping the exact same bytes in the exact
/// same order — `lookup*` below hand back sub-slices of the shared
/// buffers, zero-copy, same lifetime as `&self`.
#[derive(Debug, Clone)]
pub struct FieldPostings {
    fst: Map<Vec<u8>>,
    /// All terms' postings concatenated, in FST idx order, ascending
    /// `doc_id` within each term (same order as the historical per-term
    /// `Vec<Posting>`). Exactly `Σdf` long — sized once in
    /// `PostingsBuilder::build()`'s pass 1, so `capacity == len` and this
    /// `Box<[Posting]>` carries no growth slack.
    postings_flat: Box<[Posting]>,
    /// All terms' `doc_id` channel concatenated the same way, index-aligned
    /// with `postings_flat` (same `offsets` table). The conjunction
    /// leapfrog scans ONLY doc_ids (the `freq` is read by value lookup at
    /// scoring time), so a dedicated `[u32]` channel touches half the bytes
    /// (4 vs 8/posting) of the `[Posting]` slice — fewer cache lines on the
    /// high-`df` conjunction tail. Kept as its own channel in this phase
    /// (NOT fused into `postings_flat`; that is a separate, bench-gated
    /// step).
    doc_ids_flat: Box<[u32]>,
    /// All terms' `BlockMeta` chunks concatenated, in FST idx order. Term
    /// `i`'s blocks are `block_metas_flat[block_offsets[i]..
    /// block_offsets[i+1]]`; inside that range, block `j` describes
    /// `postings_flat[offsets[i] + j*BLOCK_SIZE .. offsets[i] +
    /// min((j+1)*BLOCK_SIZE, df)]`. Built once in
    /// `PostingsBuilder::build()` and never mutated afterwards.
    block_metas_flat: Box<[BlockMeta]>,
    /// CSR offsets into `postings_flat` / `doc_ids_flat`, length `T + 1`
    /// (`T` = number of distinct terms in this field). Term `i`'s postings
    /// are `postings_flat[offsets[i]..offsets[i+1]]` (and likewise for
    /// `doc_ids_flat`, which shares the same offsets since both channels
    /// are index-aligned). `offsets[0] == 0` and `offsets[T] ==
    /// postings_flat.len()`. `u32` is enough for matchID's 1.36 M docs;
    /// `PostingsBuilder::build()` carries a `debug_assert` that the
    /// cumulative count fits, with a comment on switching to `u64` if a
    /// future corpus needs more than ~4.29 B total postings in one field.
    offsets: Box<[u32]>,
    /// CSR offsets into `block_metas_flat`, length `T + 1`, same shape as
    /// `offsets` but for the (coarser) block-meta channel.
    block_offsets: Box<[u32]>,
    /// SPARSE side table: only terms whose `df > TERM_ROARING_THRESHOLD`
    /// get an entry — `(term_idx, bitmap)`, sorted ascending by `term_idx`
    /// (built that way, see `PostingsBuilder::build()`) so `lookup` can
    /// `binary_search_by_key` it in O(log terms_over_threshold) instead of
    /// paying one `Vec` slot per LOW-df term the way the historical
    /// `Vec<Option<RoaringDocSet>>` (one slot per term, `None` almost
    /// everywhere) did.
    roaring: Vec<(u32, RoaringDocSet)>,
}

impl FieldPostings {
    /// Resolve `term` through the FST, then the term's `[start, end)` range
    /// in the CSR `offsets` table. Shared by every `lookup*` method below.
    fn term_range(&self, term: &str) -> Option<(usize, usize, usize)> {
        let idx = self.fst.get(term.as_bytes())? as usize;
        let start = *self.offsets.get(idx)? as usize;
        let end = *self.offsets.get(idx + 1)? as usize;
        Some((idx, start, end))
    }

    fn lookup(&self, term: &str) -> Option<&[Posting]> {
        let (_, start, end) = self.term_range(term)?;
        self.postings_flat.get(start..end)
    }

    fn lookup_block_metas(&self, term: &str) -> Option<&[BlockMeta]> {
        let idx = self.fst.get(term.as_bytes())? as usize;
        let start = *self.block_offsets.get(idx)? as usize;
        let end = *self.block_offsets.get(idx + 1)? as usize;
        self.block_metas_flat.get(start..end)
    }

    /// Sparse roaring lookup by FST term idx: `binary_search_by_key` over
    /// the `(term_idx, bitmap)` side table (ascending by construction).
    fn lookup_roaring(&self, term_idx: u32) -> Option<&RoaringDocSet> {
        self.roaring
            .binary_search_by_key(&term_idx, |(idx, _)| *idx)
            .ok()
            .map(|pos| &self.roaring[pos].1)
    }

    fn lookup_with_block_metas(&self, term: &str) -> Option<PostingsList<'_>> {
        let (idx, start, end) = self.term_range(term)?;
        let block_start = *self.block_offsets.get(idx)? as usize;
        let block_end = *self.block_offsets.get(idx + 1)? as usize;
        Some(PostingsList {
            postings: self.postings_flat.get(start..end)?,
            doc_ids: self.doc_ids_flat.get(start..end)?,
            block_metas: self.block_metas_flat.get(block_start..block_end)?,
            roaring: self.lookup_roaring(idx as u32),
        })
    }

    fn sorted_terms(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.offsets.len().saturating_sub(1));
        let mut stream = self.fst.stream().into_stream();
        while let Some((bytes, _)) = stream.next() {
            // Terms entered the FST as UTF-8 (analyzed tokens are
            // always valid UTF-8), so this conversion is lossless.
            // We fall back to `from_utf8_lossy` defensively rather
            // than panicking if a future caller injects raw bytes.
            out.push(String::from_utf8_lossy(bytes).into_owned());
        }
        out
    }

    /// A6 phase 3: collect the union of doc ids across every term whose
    /// bytes start with `prefix`. Implemented as an FST range scan
    /// `[prefix, upper_bound(prefix))` so the cost is O(matching_terms +
    /// returned_postings) instead of O(distinct_terms). Returns an empty
    /// `BTreeSet` when no term matches.
    fn collect_prefix_doc_ids(&self, prefix: &[u8]) -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        let mut builder = self.fst.range().ge(prefix);
        // Upper bound: smallest byte string lexicographically greater than
        // every `prefix.<suffix>`. We increment the rightmost byte that can
        // be incremented; if every byte is 0xFF (degenerate, never happens
        // with UTF-8 analyzer output), we fall back to an unbounded scan.
        if let Some(upper) = prefix_upper_bound(prefix) {
            builder = builder.lt(upper);
        }
        let mut stream = builder.into_stream();
        while let Some((_, idx)) = stream.next() {
            let idx = idx as usize;
            let Some((&start, &end)) = self.offsets.get(idx).zip(self.offsets.get(idx + 1)) else {
                continue;
            };
            let Some(term_doc_ids) = self.doc_ids_flat.get(start as usize..end as usize) else {
                continue;
            };
            out.extend(term_doc_ids.iter().copied());
        }
        out
    }
}

/// Compute the smallest byte string greater than every byte string that
/// starts with `prefix` — i.e. the exclusive upper bound for an FST range
/// `prefix.*` scan. Returns `None` when `prefix` is empty or composed
/// entirely of `0xFF` bytes, in which case the caller must omit the upper
/// bound (the half-open range degenerates to `[prefix, +inf)`).
fn prefix_upper_bound(prefix: &[u8]) -> Option<Vec<u8>> {
    if prefix.is_empty() {
        return None;
    }
    let mut upper = prefix.to_vec();
    while let Some(last) = upper.last_mut() {
        if *last < 0xFF {
            *last += 1;
            return Some(upper);
        }
        upper.pop();
    }
    None
}

#[derive(Debug, Default, Clone)]
pub struct TermDictionary {
    fields: BTreeMap<String, FieldPostings>,
}

impl TermDictionary {
    /// Returns the terms of `field` in lexicographic order. Terms are
    /// materialized into owned `String`s because the FST stores them
    /// as compressed bytes; this is fine in practice as `terms()` is
    /// only used by tests and admin tooling, not on the hot search
    /// path.
    pub fn terms(&self, field: &str) -> TermsEnum {
        let terms = self
            .fields
            .get(field)
            .map(FieldPostings::sorted_terms)
            .unwrap_or_default();

        TermsEnum { terms, position: 0 }
    }

    pub fn postings(&self, field: &str, term: &str) -> Option<PostingsEnum<'_>> {
        self.fields
            .get(field)
            .and_then(|field_postings| field_postings.lookup(term))
            .map(|postings| PostingsEnum {
                postings,
                position: 0,
            })
    }

    /// Runtime view for a term's postings and its FoR-aligned block
    /// metadata. Search execution should prefer this over separate
    /// lookups so the postings payload and metadata stay tied together.
    pub fn postings_with_block_metas(&self, field: &str, term: &str) -> Option<PostingsList<'_>> {
        self.fields
            .get(field)
            .and_then(|field_postings| field_postings.lookup_with_block_metas(term))
    }

    /// Pre-computed per-block stats for the given `(field, term)` pair,
    /// aligned with `postings(field, term)`'s `Vec::chunks(BLOCK_SIZE)`.
    /// Returns `None` if the field or the term is unknown. The slice is
    /// empty iff the posting list itself is empty (which never happens
    /// today: a term only exists in the FST once it has at least one
    /// posting, but callers should still treat the empty case as "no
    /// blocks to inspect").
    pub fn block_metas(&self, field: &str, term: &str) -> Option<&[BlockMeta]> {
        self.fields
            .get(field)
            .and_then(|field_postings| field_postings.lookup_block_metas(term))
    }

    /// Returns the names of every field that has at least one term in
    /// the dictionary, in lexicographic order. Used by the memory
    /// accounting helper (`surch_index::memory`) to enumerate
    /// `(field, term)` pairs without exposing the internal `BTreeMap`.
    pub fn field_names(&self) -> Vec<String> {
        self.fields.keys().cloned().collect()
    }

    /// #17 memory accounting: total bytes held by every field's FST term
    /// dictionary (the on-disk byte representation, which is exactly the RAM
    /// footprint since the FST is held in memory as its serialized bytes).
    pub fn fst_bytes(&self) -> u64 {
        self.fields
            .values()
            .map(|fp| fp.fst.as_fst().as_bytes().len() as u64)
            .sum()
    }

    /// #17 memory accounting: total bytes held by the precomputed roaring
    /// bitmaps (high-`df` terms only). Lot C Phase 1: `roaring` is now the
    /// sparse `Vec<(u32, RoaringDocSet)>` side table (one entry per
    /// over-threshold term, no more `None` slots for the rest), so this is
    /// a flat sum over its entries instead of a `flat_map` + `filter_map`
    /// over a dense `Vec<Option<RoaringDocSet>>`.
    pub fn roaring_bytes(&self) -> u64 {
        self.fields
            .values()
            .flat_map(|fp| fp.roaring.iter())
            .map(|(_, set)| set.memory_bytes() as u64)
            .sum()
    }

    /// #17 memory accounting: total bytes held by the flat `block_metas_flat`
    /// buffer across every field (the BMW block-skip metadata stored
    /// alongside the postings). Lot C Phase 1: one `Box<[BlockMeta]>` per
    /// field replaces the historical per-term `Vec<BlockMeta>`, so this is
    /// now `field.block_metas_flat.len() * size_of::<BlockMeta>()` summed
    /// over fields instead of walking every term's inner Vec.
    ///
    /// This is the SINGLE source of truth for block-meta bytes — see
    /// [`MemoryUsage::term_stats_bytes`] in `surch-index::memory`, which
    /// used to recompute the exact same bytes a second time (double
    /// counting ~125 MiB on the deces 1.36 M corpus) and is now hard-wired
    /// to 0.
    pub fn block_metas_bytes(&self) -> u64 {
        let meta_size = std::mem::size_of::<BlockMeta>() as u64;
        self.fields
            .values()
            .map(|fp| fp.block_metas_flat.len() as u64 * meta_size)
            .sum()
    }

    /// Lot C Phase 1 memory accounting: real on-heap bytes of the flat
    /// postings buffers — term strings (read back from the FST) +
    /// `postings_flat` + `doc_ids_flat`, summed over fields. Replaces the
    /// old `surch-index::memory::accounting_from_postings` walker, which
    /// paid one FST point-lookup per term (`doc_index.postings(field,
    /// term)`) plus an O(df) count just to re-derive numbers that are now
    /// directly available as buffer lengths. Numerically identical to the
    /// old walker's `postings_bytes` total (same term-byte + posting +
    /// doc_id counts), just computed without the redundant per-term FST
    /// round-trips.
    pub fn postings_bytes(&self) -> u64 {
        let posting_size = std::mem::size_of::<Posting>() as u64;
        let doc_id_size = std::mem::size_of::<u32>() as u64;
        self.fields
            .values()
            .map(|fp| {
                let mut term_bytes = 0u64;
                let mut stream = fp.fst.stream().into_stream();
                while let Some((bytes, _)) = stream.next() {
                    term_bytes += bytes.len() as u64;
                }
                term_bytes
                    + (fp.postings_flat.len() as u64).saturating_mul(posting_size)
                    + (fp.doc_ids_flat.len() as u64).saturating_mul(doc_id_size)
            })
            .sum()
    }

    /// #17c memory accounting: capacity SLACK across the postings buffers.
    /// Lot C Phase 1 flattened `postings`/`doc_ids`/`block_metas`/`offsets`/
    /// `block_offsets` from per-term `Vec<T>` (millions of small
    /// allocations, each with up to ~50 % geometric-growth slack) into one
    /// `Box<[T]>` PER FIELD, sized EXACTLY by `PostingsBuilder::build()`'s
    /// two-pass sizing. A boxed slice has no spare capacity by
    /// construction, so those five channels now contribute exactly 0. The
    /// sparse `roaring` side table is the one remaining `Vec` (it grows by
    /// `push` while draining, since its final size is not worth a separate
    /// counting pass) — `shrink_to_fit()` is called on it at the end of
    /// `build()`, so in steady state this whole gauge is ~0. Kept as a
    /// real computation rather than a hardcoded 0 so it still catches a
    /// future regression that reintroduces slack. Read-only, O(fields).
    pub fn postings_capacity_slack_bytes(&self) -> u64 {
        let entry_size = std::mem::size_of::<(u32, RoaringDocSet)>() as u64;
        self.fields
            .values()
            .map(|fp| {
                (fp.roaring.capacity().saturating_sub(fp.roaring.len()) as u64)
                    .saturating_mul(entry_size)
            })
            .sum()
    }

    /// A6 phase 3: union of doc ids across every term of `field` whose
    /// bytes start with `prefix`. Used by the keyword-prefix iterator on
    /// fields that did not declare `index_prefixes` (e.g. matchID's
    /// `DATE_NAISSANCE` keyword). The FST range scan is O(matching_terms)
    /// instead of O(distinct_terms): on the deces 1k slice and the matchID
    /// `< 8 chars` autocomplete contract, `matching_terms` is bounded by
    /// the year cardinality (~365 dates per year).
    ///
    /// Returns an empty set when the field is absent or no term matches.
    pub fn prefix_doc_ids(&self, field: &str, prefix: &str) -> BTreeSet<u32> {
        self.fields
            .get(field)
            .map(|field_postings| field_postings.collect_prefix_doc_ids(prefix.as_bytes()))
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct TermsEnum {
    terms: Vec<String>,
    position: usize,
}

impl Iterator for TermsEnum {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.terms.len() {
            return None;
        }
        let term = std::mem::take(&mut self.terms[self.position]);
        self.position += 1;
        Some(term)
    }
}

#[derive(Debug, Clone)]
pub struct PostingsEnum<'a> {
    postings: &'a [Posting],
    position: usize,
}

impl<'a> Iterator for PostingsEnum<'a> {
    type Item = &'a Posting;

    fn next(&mut self) -> Option<Self::Item> {
        let posting = self.postings.get(self.position);
        self.position += usize::from(posting.is_some());
        posting
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PostingsList<'a> {
    postings: &'a [Posting],
    doc_ids: &'a [u32],
    block_metas: &'a [BlockMeta],
    roaring: Option<&'a RoaringDocSet>,
}

impl<'a> PostingsList<'a> {
    pub fn postings(&self) -> &'a [Posting] {
        self.postings
    }

    /// The compact ascending `doc_id` channel (index-aligned with
    /// [`Self::postings`]). The conjunction leapfrog walks this instead of the
    /// `[Posting]` slice to touch half the bytes per posting.
    pub fn doc_ids(&self) -> &'a [u32] {
        self.doc_ids
    }

    /// The precomputed roaring/hybrid bitmap for this term, present only for
    /// high-`df` terms (`df > TERM_ROARING_THRESHOLD`). `Some` on both sides of
    /// a conjunction ⇒ the intersection ANDs word-parallel bitmaps (A1).
    pub fn roaring(&self) -> Option<&'a RoaringDocSet> {
        self.roaring
    }

    pub fn block_metas(&self) -> &'a [BlockMeta] {
        self.block_metas
    }

    pub fn doc_freq_from_block_metas(&self) -> usize {
        self.block_metas.iter().map(|meta| meta.posting_count).sum()
    }

    /// Build a [`BlockSkipList`] (Lot 2: skip lists on the codec FoR
    /// path) from this term's per-block metadata. Returns `None` when
    /// the posting list is empty (so callers can short-circuit without
    /// constructing an empty skip list). Returns `Err(_)` only on a
    /// programmer-bug-level invariant violation (postings not strictly
    /// increasing across blocks); these errors should never fire in
    /// practice because `PostingsBuilder::build()` sorts postings by
    /// ascending `doc_id` before `build_block_metas` runs.
    pub fn block_skip_list(
        &self,
    ) -> std::result::Result<Option<BlockSkipList>, PostingsBlockError> {
        if self.block_metas.is_empty() {
            return Ok(None);
        }
        let entries = self
            .block_metas
            .iter()
            .enumerate()
            .map(|(idx, meta)| BlockSkipEntry {
                block_index: idx,
                min_doc_id: meta.min_doc_id,
                max_doc_id: meta.max_doc_id,
            });
        let skip_list = BlockSkipList::from_block_ranges(entries)?;
        Ok(Some(skip_list))
    }

    /// Build a leapfrog iterator over this term's postings, driven by
    /// the codec-level [`BlockSkipList`]. The caller advances through
    /// the posting list by calling `advance_to(target)`; the iterator
    /// uses the skip list to jump past whole 128-block chunks whose
    /// `max_doc_id < target` (Lot 2).
    ///
    /// Returns `None` if the posting list is empty (so callers can
    /// short-circuit without holding a skip list around). The iterator
    /// is monotonic — subsequent `advance_to` calls must use a
    /// non-decreasing `target`.
    pub fn skip_iter(
        &self,
    ) -> std::result::Result<Option<PostingsBlockSkipIter<'a>>, PostingsBlockError> {
        let Some(skip_list) = self.block_skip_list()? else {
            return Ok(None);
        };
        Ok(Some(PostingsBlockSkipIter {
            doc_ids: self.doc_ids,
            skip_list,
            cursor_position: 0,
            position: 0,
            blocks_skipped: 0,
        }))
    }
}

/// Iterator over a term's doc_ids that leapfrogs whole 128-block
/// chunks whenever the caller advances past a block's `max_doc_id`.
/// Built by [`PostingsList::skip_iter`]. Walks the compact `[u32]` doc_id
/// channel (not `[Posting]`): the conjunction only needs doc_ids, and halving
/// the per-entry footprint cuts the cache lines touched on the high-`df` tail.
#[derive(Debug)]
pub struct PostingsBlockSkipIter<'a> {
    doc_ids: &'a [u32],
    skip_list: BlockSkipList,
    /// Current bottom-layer cursor position, kept in sync with
    /// the internal `BlockSkipCursor`. We re-derive a fresh
    /// `BlockSkipCursor` on every `advance_to` call to keep the
    /// iterator `'a`-lifetime-free.
    cursor_position: usize,
    position: usize,
    blocks_skipped: usize,
}

impl<'a> PostingsBlockSkipIter<'a> {
    /// Number of 128-block chunks the iterator skipped over (relative
    /// to a naive full walk). Used by tests to assert that the skip
    /// list is actually doing work.
    pub fn blocks_skipped(&self) -> usize {
        self.blocks_skipped
    }

    /// Bottom-layer cursor position: the number of doc_ids consumed so far.
    /// After `advance_to(target)` returns `Some(target)`, the matched entry is
    /// at index `position() - 1` in the underlying channel — and, since the
    /// `doc_id` channel is index-aligned with the term's `[Posting]` slice, at
    /// the same index in `PostingsList::postings()`. Lets the conjunction
    /// capture the matched `freq` in O(1) instead of a `binary_search` per hit.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Advance to the first `doc_id >= target`. The `target` must be
    /// non-decreasing across calls. Returns the matching `doc_id` (or `None`
    /// if the iterator is exhausted). Calling `advance_to` repeatedly produces
    /// doc_ids in strictly ascending order, just like `Iterator::next`, but
    /// with block-level skipping.
    pub fn advance_to(&mut self, target: u32) -> Option<u32> {
        if self.position >= self.doc_ids.len() {
            return None;
        }

        // Slow path: use the skip list to leapfrog past whole blocks
        // whose max_doc_id < target. We only enter the skip-list
        // codepath when the *current* block cannot contain target
        // (its max_doc_id < target). For repeated calls inside the
        // same block, the bottom-layer cursor stays put and we only
        // pay one comparison per loop iteration.
        let current_block_idx = self.position / BLOCK_SIZE;
        let current_block_max = self
            .skip_list
            .entries()
            .get(current_block_idx)
            .map(|entry| entry.max_doc_id);
        if current_block_max.is_some_and(|max| max < target) {
            let mut cursor = BlockSkipCursor::resume(&self.skip_list, self.cursor_position);
            let landed = cursor.advance_to(target)?;
            let block_start = landed.block_index * BLOCK_SIZE;
            if block_start > self.position {
                self.blocks_skipped += (block_start - self.position) / BLOCK_SIZE;
                self.position = block_start;
            }
            self.cursor_position = cursor.position();
        }

        // Find the first doc_id >= target from `position`. The skip list above
        // already bounded the distance to ~BLOCK_SIZE, but a per-element linear
        // scan branch-mispredicts on irregular gaps — the per-posting constant
        // factor that dominates the conjunction tail. Replace it with galloping
        // (exponential bound, O(log δ)) + `partition_point` (branchless cmov on
        // the ascending slice). Result is the exact "first >= target", so it is
        // BIT-IDENTICAL to the linear scan (parity-trivial, deterministic).
        let rest = &self.doc_ids[self.position..];
        let mut hi = 1usize;
        while hi < rest.len() && rest[hi] < target {
            hi <<= 1;
        }
        let lo = hi >> 1;
        let hi = hi.min(rest.len());
        let offset = lo + rest[lo..hi].partition_point(|&doc_id| doc_id < target);
        if offset >= rest.len() {
            self.position = self.doc_ids.len();
            return None;
        }
        self.position += offset + 1;
        Some(rest[offset])
    }
}

impl Iterator for PostingsBlockSkipIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let doc_id = *self.doc_ids.get(self.position)?;
        self.position += 1;
        Some(doc_id)
    }
}
