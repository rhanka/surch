use std::collections::BTreeMap;

use fst::{IntoStreamer, Map, MapBuilder, Streamer};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, PostingsError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PostingsError {
    #[error("field name must not be empty")]
    EmptyField,
    #[error("term must not be empty")]
    EmptyTerm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub doc_id: u32,
    pub freq: u32,
    pub positions: Vec<u32>,
}

impl Posting {
    pub fn new(doc_id: u32, positions: Vec<u32>) -> Self {
        let freq = if positions.is_empty() {
            1
        } else {
            positions.len() as u32
        };

        Self {
            doc_id,
            freq,
            positions,
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
    /// Greatest term_freq inside this block of up to 128 postings.
    pub max_term_freq: u32,
    /// Smallest `doc_id` inside this block (so callers can range-skip).
    pub min_doc_id: u32,
    /// Largest `doc_id` inside this block.
    pub max_doc_id: u32,
}

/// Number of postings per BMW block. Must match the `BLOCK_SIZE` used by
/// `maxscore_match` in `surch-api` — block metas are aligned with the
/// `Vec<Posting>` chunks produced by `Vec::chunks(BLOCK_SIZE)`.
pub const BLOCK_SIZE: usize = 128;

fn build_block_metas(postings: &[Posting]) -> Vec<BlockMeta> {
    postings
        .chunks(BLOCK_SIZE)
        .map(|chunk| {
            // `chunks` only yields non-empty slices, so first/last are safe.
            let min_doc_id = chunk.first().expect("chunk is non-empty").doc_id;
            let max_doc_id = chunk.last().expect("chunk is non-empty").doc_id;
            let max_term_freq = chunk.iter().map(|p| p.freq).max().unwrap_or(0);
            BlockMeta {
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

        self.fields
            .entry(field)
            .or_default()
            .entry(term)
            .or_default()
            .push(Posting::new(doc_id, positions));

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
            let mut postings: Vec<Vec<Posting>> = Vec::with_capacity(terms.len());
            let mut block_metas: Vec<Vec<BlockMeta>> = Vec::with_capacity(terms.len());
            for (idx, (term, term_postings)) in terms.into_iter().enumerate() {
                builder
                    .insert(term.as_bytes(), idx as u64)
                    .expect("fst::MapBuilder accepts lex-sorted keys");
                // Per-block stats are computed once here, against the
                // ascending-doc_id postings, then read back in O(1) by
                // `maxscore_match` instead of being recomputed at every
                // query.
                block_metas.push(build_block_metas(&term_postings));
                postings.push(term_postings);
            }
            let bytes = builder
                .into_inner()
                .expect("fst::MapBuilder memory writer never fails I/O");
            let fst = Map::new(bytes).expect("fst::Map from valid MapBuilder bytes");
            fields.insert(
                field,
                FieldPostings {
                    fst,
                    postings,
                    block_metas,
                },
            );
        }

        TermDictionary { fields }
    }
}

/// Per-field FST term dictionary plus a `Vec<Vec<Posting>>` indexed by
/// the FST output (a `u64` we narrow to `usize`). The FST shares
/// prefixes between terms (e.g. all the "rue de la X" or "DUPONT"
/// variants in a French civic address corpus) which is where the RAM
/// gain comes from compared to the previous `BTreeMap<String, …>`.
#[derive(Debug, Clone)]
pub struct FieldPostings {
    fst: Map<Vec<u8>>,
    postings: Vec<Vec<Posting>>,
    /// Per-term `Vec<BlockMeta>` aligned with `postings` (same index, same
    /// length: one inner Vec per term). Inside each term, the `BlockMeta`
    /// at position `i` describes `postings[term_idx][i*BLOCK_SIZE ..
    /// (i+1)*BLOCK_SIZE]`. Built once in `PostingsBuilder::build()` and
    /// never mutated afterwards.
    block_metas: Vec<Vec<BlockMeta>>,
}

impl FieldPostings {
    fn lookup(&self, term: &str) -> Option<&[Posting]> {
        let idx = self.fst.get(term.as_bytes())? as usize;
        self.postings.get(idx).map(Vec::as_slice)
    }

    fn lookup_block_metas(&self, term: &str) -> Option<&[BlockMeta]> {
        let idx = self.fst.get(term.as_bytes())? as usize;
        self.block_metas.get(idx).map(Vec::as_slice)
    }

    fn sorted_terms(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.postings.len());
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
