use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use fst::{IntoStreamer, Map, MapBuilder, Streamer};
use surch_codec::postings_block::{
    block_directory, decode_block, decode_postings_blocked, encode_postings_blocked, BlockDirEntry,
    BlockSkipCursor, BlockSkipEntry, BlockSkipList, PostingsBlockError, FOR_BLOCK_SIZE,
};

use crate::roaring::RoaringDocSet;
use postings_segment::PostingsSegment;

/// Lot C `C1a-batché` (`docs/paper/c1b-disk-backed-design-2026-07-02.md`)
/// SHADOW disk-backed FoR postings segment. Mirrors `DocumentIndex`'s
/// historical `subfield_store` module (itself mirroring surch-api's
/// `source_store`): an append-only tempfile under `TMPDIR`, written and
/// read with SAFE positional I/O (`FileExt::write_all_at`/`read_exact_at`
/// — no `mmap`, no `posix_fallocate`, both `unsafe`, so `surch-index`
/// keeps `#![forbid(unsafe_code)]`).
///
/// "SHADOW" means the segment is written and round-trip-validated
/// (`debug_assert!` in [`PostingsBuilder::build`]) but nothing reads from
/// it on any production path yet: RAM (`doc_ids_flat`/`freqs_flat`
/// inside [`FieldPostings`]) stays the sole source of truth consumed by
/// `lookup*`/[`PostingsList`]/scoring/the leapfrog. [`TermDictionary::decode_from_segment`]
/// exists only for validation and future cold-path use — it is not on
/// any hot path today.
///
/// Writes are batched PER FIELD, not per term: [`PostingsBuilder::build`]
/// accumulates one field's entire FoR-blocked payload into a single
/// `Vec<u8>` and calls [`PostingsSegment::append`] exactly once per
/// field. On a real corpus (a handful to a few dozen indexed fields)
/// that is on the order of 12-25 `write_all_at` calls TOTAL for the
/// whole build — the lesson from the Lot C C0 cliff (5.46 M syscalls,
/// one per term, cost -43 % indexation throughput) is the reason this
/// is a hard batching requirement, not an optimization.
///
/// Hardening: the whole segment path is BEST-EFFORT. A `_refresh` on the
/// real deces corpus (1.36 M docs) once paniced inside `build()` because
/// `encode_postings_blocked` legitimately rejects duplicate doc_ids
/// (`Value::Array` fields explode into several same-doc_id postings
/// upstream, and `PostingsBuilder::add` used to not deduplicate) and the
/// call site used to `.expect()` that away. Since a SHADOW feature must
/// never be able to crash indexation, every failure mode here — tempfile
/// creation, a term's encode, a field's batched write, a `pread` —
/// degrades to "no disk coverage" (a sentinel `(0, 0)` descriptor, or a
/// fully absent segment) instead of panicking. RAM
/// (`doc_ids_flat`/`freqs_flat`) is unaffected by any of it; see
/// [`TermDictionary::postings_segment_skipped_terms`] for the gauge that
/// makes per-term encode failures observable.
///
/// Correction fix: `PostingsBuilder::build()` now merges same-doc_id runs
/// (`dedup_merge_postings`, matching Lucene/ES multi-valued-field
/// semantics) BEFORE the sort/encode above, so the scenario that used to
/// panic no longer arises from ordinary indexing — the skipped-terms
/// gauge should read 0 in steady state. The best-effort fallback (and
/// this whole tolerance path) stays in place as defense-in-depth for any
/// other future bug that could produce non-monotonic doc_ids.
#[cfg(unix)]
mod postings_segment {
    use std::{
        fs::{File, OpenOptions},
        os::unix::fs::FileExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    /// Logical pre-extension chunk for the segment file, in bytes. Kept
    /// from the `subfield_store` pattern for parity — a safe `set_len`
    /// (ftruncate) pre-extension, NOT `posix_fallocate` (which is
    /// `unsafe`). C1a's writes are already batched per field (a few
    /// dozen `append()` calls total, not millions), so per-write ext4
    /// extension is not the cliff risk here; this is cheap insurance
    /// regardless.
    const EXTEND_CHUNK: u64 = 64 * 1024 * 1024;

    /// Unique tempfile name without pulling a `uuid` dependency into
    /// surch-index: pid + a process-global sequence + wall-clock nanos.
    fn next_temp_path() -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("surch-postings-{pid}-{seq}-{nanos}.dat"))
    }

    /// Append-only FoR-postings segment. `pread`-addressed via the
    /// `(offset, len)` descriptors [`crate::postings::PostingsBuilder::build`]
    /// records in `FieldPostings::segment_descriptors`, one descriptor
    /// per term.
    #[derive(Debug)]
    pub(crate) struct PostingsSegment {
        file: File,
        next_offset: u64,
        extended_len: u64,
        path: PathBuf,
    }

    impl PostingsSegment {
        /// Best-effort constructor: NEVER panics. A SHADOW feature must
        /// not be able to crash indexation (the C1a-batché lesson: the
        /// historical `Default::default()` here `.expect()`'d on tempfile
        /// creation, which paniced the whole `_refresh` on a real corpus
        /// when the underlying cause was actually an unrelated encode
        /// failure further down — see [`PostingsBuilder::build`]).
        /// Returns `None` on any I/O failure (unwritable `TMPDIR`,
        /// `ENOSPC`, permissions, ...): the caller then keeps building
        /// with no segment at all, every term gets a sentinel `(0, 0)`
        /// descriptor, and RAM (`doc_ids_flat`/`freqs_flat`) stays the
        /// sole source of truth — exactly as if the segment had never
        /// existed.
        pub(crate) fn try_new() -> Option<Self> {
            let path = next_temp_path();
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .ok()?;
            Some(Self {
                file,
                next_offset: 0,
                extended_len: 0,
                path,
            })
        }

        /// Pre-extend the file's logical length past `write_end` in
        /// `EXTEND_CHUNK` steps via the safe `File::set_len` (ftruncate),
        /// so the append below does not pay ext4's per-write size
        /// extension. Failure is non-fatal: the file keeps its current
        /// length and the next write falls back to the ordinary
        /// per-write extension (slower, still correct).
        fn ensure_capacity(&mut self, write_end: u64) {
            if write_end <= self.extended_len {
                return;
            }
            let mut new_len = self.extended_len;
            while new_len < write_end {
                new_len = new_len.saturating_add(EXTEND_CHUNK);
            }
            if self.file.set_len(new_len).is_ok() {
                self.extended_len = new_len;
            }
        }

        /// Append `bytes` as ONE positional write, returning the
        /// segment-absolute byte offset the payload now starts at, or
        /// `None` if the write failed (`ENOSPC`, a `tmpfs` gone away,
        /// ...). Callers batch a whole field's FoR payload into one
        /// `Vec<u8>` before calling this (see `PostingsBuilder::build`),
        /// so in steady state this is called ~once per field — never
        /// once per term. Best-effort: a `None` here must disable the
        /// segment for the caller, never panic — RAM
        /// (`doc_ids_flat`/`freqs_flat`) is always still correct.
        pub(crate) fn append(&mut self, bytes: &[u8]) -> Option<u64> {
            let offset = self.next_offset;
            self.ensure_capacity(offset.saturating_add(bytes.len() as u64));
            self.file.write_all_at(bytes, offset).ok()?;
            self.next_offset = offset.saturating_add(bytes.len() as u64);
            Some(offset)
        }

        /// Read `length` bytes at `offset` via `pread`. Returns `None` on
        /// any I/O failure instead of panicking — this is only ever
        /// called from the SHADOW round-trip validation and the
        /// (not-yet-production) cold-path decode, never from a path RAM
        /// depends on.
        pub(crate) fn read(&self, offset: u64, length: u32) -> Option<Vec<u8>> {
            let mut buf = vec![0u8; length as usize];
            self.file.read_exact_at(&mut buf, offset).ok()?;
            Some(buf)
        }

        /// Total bytes appended so far — feeds the
        /// `surch_index_disk_postings_bytes` gauge.
        pub(crate) fn bytes_written(&self) -> u64 {
            self.next_offset
        }
    }

    impl Drop for PostingsSegment {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Non-Unix fallback (Windows / unusual dev hosts): the same API backed
/// by an in-RAM buffer. The release target is Linux distroless and
/// `pread`/`pwrite` (`FileExt`) are Unix-only, so this keeps the crate
/// building elsewhere — SHADOW validation still exercises the codec
/// round-trip, just without a real file underneath.
#[cfg(not(unix))]
mod postings_segment {
    #[derive(Debug, Default)]
    pub(crate) struct PostingsSegment {
        buf: Vec<u8>,
    }

    impl PostingsSegment {
        /// Mirrors the Unix constructor's signature; the in-RAM buffer
        /// never fails to allocate in practice, so this is always `Some`.
        pub(crate) fn try_new() -> Option<Self> {
            Some(Self::default())
        }

        pub(crate) fn append(&mut self, bytes: &[u8]) -> Option<u64> {
            let offset = self.buf.len() as u64;
            self.buf.extend_from_slice(bytes);
            Some(offset)
        }

        pub(crate) fn read(&self, offset: u64, length: u32) -> Option<Vec<u8>> {
            let start = usize::try_from(offset).ok()?;
            let end = start.checked_add(length as usize)?;
            self.buf.get(start..end).map(<[u8]>::to_vec)
        }

        pub(crate) fn bytes_written(&self) -> u64 {
            self.buf.len() as u64
        }
    }
}

/// A1: build a roaring/hybrid bitmap only for terms with more than this many
/// postings. Below it, the galloping leapfrog already wins and the bitmap RAM
/// is not worth it; above it (the common-name tail), the word-parallel AND is
/// the bool/full gap-closer. The container choice (dense bitmap vs sparse
/// array) is then made per 65 536-chunk inside [`RoaringDocSet`].
const TERM_ROARING_THRESHOLD: usize = 4_096;
use thiserror::Error;

/// Lot C `C1b` sous-pas 2 (`docs/paper/c1b-disk-backed-design-2026-07-02.md`
/// §"Séquence + flag + revert"): process-wide default for the disk-backed
/// postings read path. `true` when `SURCH_POSTINGS_DISK` is `"1"` or
/// case-insensitively `"true"`; `false` (the 100 % in-RAM engine,
/// unconditionally safe) otherwise, including when the variable is unset.
///
/// Read ONCE per process via `OnceLock` — the value must stay stable for
/// the process's whole lifetime, since a `TermDictionary`'s RAM/disk
/// layout is fixed forever at the [`PostingsBuilder::build`] call that
/// produced it (flag ON empties `doc_ids_flat`/`freqs_flat`; flag OFF
/// never touches the disk read path). This is why the actual read-path
/// decision downstream is NOT a repeated call to this function: it is
/// [`TermDictionary::disk_backed`], a bit baked into the built
/// dictionary itself (see [`PostingsBuilder::build_with_disk_flag`]) —
/// that is both more correct (a dictionary's contents cannot silently
/// disagree with what its own read path assumes) and independently
/// solves a testability problem this `OnceLock` would otherwise create:
/// a single test process cannot flip a `OnceLock`-cached global mid-run,
/// so a same-process flag-ON == flag-OFF parity test
/// (`crates/surch-api/tests/postings_disk_parity.rs`) instead builds two
/// indices with an explicit per-index override
/// (`DocumentIndex::set_postings_disk_enabled`) that bypasses this
/// function entirely.
///
/// Revert story: turning the flag back off needs no rebuild of the disk
/// segment itself — `postings.dat` is written unconditionally regardless
/// of this flag (Lot C `C1a-batché`) — only the NEXT
/// [`PostingsBuilder::build`] (the next `_refresh`/materialize) picks up
/// the new value.
pub fn postings_disk_enabled() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| {
        std::env::var("SURCH_POSTINGS_DISK")
            .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

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

/// Per-block statistic computed once at `PostingsBuilder::build()` time,
/// then read directly on the hot scoring path instead of being recomputed
/// at every query (the Block-Max WAND a.k.a. `BlockWAND` schema used by
/// Tantivy / Lucene block-max postings).
///
/// Each `BlockMeta` describes one chunk of up to [`BLOCK_SIZE`] consecutive
/// postings inside a term's flattened `doc_ids_flat` slice (postings are
/// kept sorted by ascending `doc_id`, and the flat buffers are CSR-indexed
/// by `offsets`/`block_offsets` — see [`FieldPostings`]).
///
/// Lot C Phase 1 lever B: `BlockMeta` used to also carry `posting_count`,
/// `min_doc_id`, `max_doc_id` (16 B total). All three are DERIVABLE at
/// O(1) from data already resident: block `j` of term `i` is exactly
/// `doc_ids_flat[offsets[i] + j*BLOCK_SIZE .. offsets[i] +
/// min((j+1)*BLOCK_SIZE, df)]`, so its `posting_count` is that range's
/// length and `min_doc_id`/`max_doc_id` are its first/last entry
/// ([`PostingsList::doc_freq_from_block_metas`] and
/// [`PostingsList::block_skip_list`] read them straight off `doc_ids`
/// now). Only `max_term_freq` truly needs precomputing — finding it
/// requires an O(block) scan the hot path cannot repeat per query — so
/// `BlockMeta` shrank from 16 to 4 bytes: ~119 MiB to ~30 MiB on the
/// deces 1.36 M-doc corpus (one block per 128 postings, several fields).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BlockMeta {
    /// Greatest term_freq inside this block of up to [`BLOCK_SIZE`] postings.
    pub max_term_freq: u32,
}

/// Number of postings per BMW block. Must match the `BLOCK_SIZE` used by
/// `maxscore_match` in `surch-api` — block metas are aligned with the
/// `Vec<Posting>` chunks produced by `Vec::chunks(BLOCK_SIZE)`.
pub const BLOCK_SIZE: usize = FOR_BLOCK_SIZE;

/// Correction fix (post-C1a-batché hardening): merge every run of equal
/// `doc_id` in an already-sorted `postings` slice into ONE posting,
/// matching Lucene/ES semantics where a document appears at most once in
/// a term's posting list. `df` (`postings.len()` after this call) is the
/// number of distinct documents; `freq` on the merged posting is the SUM
/// of the run's freqs, i.e. `tf` = total occurrences of the term across
/// every value of the source field, not per value.
///
/// Runs exist because `PostingsBuilder::add` is called once per `(field,
/// value)` pair `indexed_fields_for_document` (surch-api) fans a source
/// `Value::Array` field out into: several pairs share the same
/// `field`/`doc_id` and, when the same term occurs in more than one
/// value, the same `(field, term, doc_id)` triple gets `add`-ed several
/// times for one document. `add` itself stays a plain push (append-only,
/// O(1), no per-insert scan of the term's Vec); this function is the one
/// and only place the merge happens, and it must run AFTER the
/// ascending-`doc_id` sort so every run is contiguous.
///
/// Positions are not affected: `Posting` does not carry them (see the
/// doc comment on `Posting`, positions are discarded right after `freq`
/// is derived from them at `add` time), so there is no position-list
/// concatenation to do here.
///
/// A mono-valued field never produces a run (`analyze_document` calls
/// `analyzed_terms` exactly once per field per document, so each
/// distinct term is `add`-ed exactly once): the common case is the
/// `postings.len() < 2` early return, and otherwise the loop below finds
/// no two adjacent entries with an equal `doc_id`, so it degrades to a
/// copy with no merge — mono-valued indexing is byte-identical to
/// before this fix.
fn dedup_merge_postings(postings: &mut Vec<Posting>) {
    if postings.len() < 2 {
        return;
    }
    let mut write = 0;
    for read in 1..postings.len() {
        if postings[read].doc_id == postings[write].doc_id {
            postings[write].freq = postings[write].freq.saturating_add(postings[read].freq);
        } else {
            write += 1;
            postings[write] = postings[read];
        }
    }
    postings.truncate(write + 1);
}

fn build_block_metas(postings: &[Posting]) -> Vec<BlockMeta> {
    postings
        .chunks(BLOCK_SIZE)
        .map(|chunk| {
            let max_term_freq = chunk.iter().map(|p| p.freq).max().unwrap_or(0);
            BlockMeta { max_term_freq }
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

    /// Build the [`TermDictionary`], reading the process-wide
    /// [`postings_disk_enabled`] flag for the RAM-vs-disk-backed decision.
    /// Every production call site (`surch-index::document_index`) uses
    /// this. Tests that need BOTH a flag-ON and a flag-OFF
    /// `TermDictionary` in the SAME process (the `OnceLock` behind
    /// [`postings_disk_enabled`] cannot flip mid-run — see its doc
    /// comment) call [`Self::build_with_disk_flag`] directly instead.
    pub fn build(self) -> TermDictionary {
        let disk_enabled = postings_disk_enabled();
        self.build_with_disk_flag(disk_enabled)
    }

    /// Lot C `C1b` sous-pas 2: [`Self::build`] with an explicit
    /// `disk_enabled` override instead of the process-wide
    /// [`postings_disk_enabled`] flag. `disk_enabled == false` reproduces
    /// [`Self::build`]'s historical behaviour byte-for-byte (the branch
    /// below is the SAME code [`Self::build`] always ran, now reached via
    /// `postings_disk_enabled() == false` instead of unconditionally).
    /// `disk_enabled == true` empties `doc_ids_flat`/`freqs_flat`/
    /// `block_metas_flat` (RAM freed — the disk segment, written
    /// unconditionally since Lot C `C1a-batché`, becomes the sole source
    /// of truth) and persists the per-block directory
    /// ([`FieldPostings::block_directory`]) so the disk read path never
    /// re-decodes a whole term just to skip blocks.
    pub fn build_with_disk_flag(mut self, disk_enabled: bool) -> TermDictionary {
        // Sort each posting list by ascending doc_id (the query engine
        // relies on this to do single-pass conjunctions and unions), then
        // merge same-doc_id runs into a single Lucene-shaped posting: see
        // `dedup_merge_postings` below for why a run can exist at all
        // (multi-valued source fields) and what "merge" means here.
        for terms in self.fields.values_mut() {
            for postings in terms.values_mut() {
                postings.sort_by_key(|posting| posting.doc_id);
                dedup_merge_postings(postings);
            }
        }

        // Lot C `C1a-batché`: ONE segment for the WHOLE build (not one per
        // field — the field is only the write-BATCH unit, see the module
        // doc on `postings_segment` above). Owned locally (not yet behind
        // an `Arc`) for the duration of the fields loop below so `append`
        // can take `&mut`; wrapped in `Arc` once, at the very end, for
        // read-only sharing via the returned `TermDictionary`.
        //
        // Hardening (post-C1a-batché crash on the real deces corpus):
        // best-effort from here on. `try_new()` returns `None` on any I/O
        // failure instead of panicking, and is downgraded to `None` again,
        // mid-build, the first time a field's batched `append()` fails
        // (see below) — the segment is a SHADOW validation write, RAM
        // (`doc_ids_flat`/`freqs_flat`) is and stays the sole source of
        // truth, so it must never be able to crash indexation.
        let mut postings_segment = PostingsSegment::try_new();
        let mut postings_skipped_terms: u64 = 0;

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
            // size the flat buffers EXACTLY: Σdf for `doc_ids_flat` /
            // `freqs_flat`, Σblocks for `block_metas_flat`, and the
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

            // Lot C `C1b` sous-pas 2: when `disk_enabled`, these three
            // channels are NEVER populated below (the per-term loop takes
            // the disk-mode branch instead) — sizing their capacity to 0
            // rather than `total_postings`/`total_blocks` avoids paying
            // for an allocation that would otherwise sit empty for the
            // dictionary's whole life. `disk_enabled == false` reserves
            // the EXACT SAME capacity as before this sous-pas.
            let mut freqs_flat: Vec<u32> =
                Vec::with_capacity(if disk_enabled { 0 } else { total_postings });
            let mut doc_ids_flat: Vec<u32> =
                Vec::with_capacity(if disk_enabled { 0 } else { total_postings });
            let mut block_metas_flat: Vec<BlockMeta> =
                Vec::with_capacity(if disk_enabled { 0 } else { total_blocks });
            let mut offsets: Vec<u32> = Vec::with_capacity(term_count + 1);
            // Plan segments S5b: `block_offsets` is DEAD in disk mode — its
            // only readers (`lookup_block_metas`/`lookup_with_block_metas`)
            // index `block_metas_flat`, which stays empty under
            // `disk_enabled`, and every `surch-api` call site branches on
            // `postings_disk_backed()` before reaching them. It is not
            // built at all in disk mode (neither resident nor spilled) —
            // the disk read path's block addressing lives in the
            // block-directory channel below instead. `disk_enabled ==
            // false` keeps the exact historical shape (`T + 1` entries).
            let mut block_offsets: Vec<u32> =
                Vec::with_capacity(if disk_enabled { 0 } else { term_count + 1 });
            // Sparse side table: only terms with df > TERM_ROARING_THRESHOLD
            // get an entry, so there is no useful exact size to precompute;
            // `shrink_to_fit()` right before insertion below removes the
            // geometric-growth slack instead.
            let mut roaring: Vec<(u32, RoaringDocSet)> = Vec::new();
            offsets.push(0);
            if !disk_enabled {
                block_offsets.push(0);
            }

            // Lot C `C1a-batché`: per-term `(offset, len)` descriptor into
            // the shared `postings_segment`, index-aligned with `offsets`
            // (`T` entries, one per term — NOT a CSR table, each entry
            // stands alone). `field_payload` accumulates every term's
            // `encode_postings_blocked` bytes for THIS field; it is
            // flushed to the segment with a SINGLE `append()` once the
            // field is done (see below the loop) — batched by field, not
            // by term, per the C0 lesson (5.46 M single-term writes cost
            // -43 % indexation throughput; a few dozen field-sized writes
            // do not).
            let mut segment_descriptors: Vec<(u64, u32)> = Vec::with_capacity(term_count);
            let mut field_payload: Vec<u8> = Vec::with_capacity(total_postings * 3);

            // Lot C `C1b` sous-pas 2: persisted per-block directory —
            // built ONLY when `disk_enabled` (see
            // `FieldPostings::block_directory`'s doc comment). Left as an
            // untouched, unallocated `Vec::new()` when the flag is off:
            // `into_boxed_slice()` on it allocates nothing.
            let mut block_directory_flat: Vec<BlockDirEntry> = Vec::new();
            let mut block_dir_offsets: Vec<u32> =
                Vec::with_capacity(if disk_enabled { term_count + 1 } else { 0 });
            if disk_enabled {
                block_dir_offsets.push(0);
            }

            // --- Pass 2: fill, draining `terms` term-by-term ------------
            // Transitory-peak mitigation (risk #1 of the flattening): a
            // naive "collect everything into Vec<Vec<_>>, then flatten"
            // would briefly hold BOTH the fully-populated builder map AND
            // the fully-populated flat buffers in RAM at once — doubling
            // the peak for the duration of the flatten. Instead,
            // `terms.into_iter()` DRAINS the per-field
            // `BTreeMap<String, Vec<Posting>>` one entry at a time: each
            // term's source `Vec<Posting>` is moved into `term_postings`
            // below and only ever BORROWED afterwards (into `doc_ids_flat`
            // then `freqs_flat`) — it is dropped at the end of the loop
            // body (the `for` pattern owns it), which frees that source
            // Vec's heap allocation as soon as its bytes have been copied
            // into the flat buffers. RAM therefore grows monotonically
            // toward `total_postings`, it never double-peaks.
            for (idx, (term, term_postings)) in terms.into_iter().enumerate() {
                builder
                    .insert(term.as_bytes(), idx as u64)
                    .expect("fst::MapBuilder accepts lex-sorted keys");

                if disk_enabled {
                    // Lot C `C1b` sous-pas 2: disk-backed path. Encode
                    // straight from `term_postings` (owned, transient —
                    // dropped at the end of this loop iteration) instead of
                    // slicing `doc_ids_flat`/`freqs_flat`, which stay EMPTY
                    // for the whole field: the disk segment + the persisted
                    // directory built below are the sole source of truth for
                    // this term's postings, RAM is freed. Mirrors the
                    // flag-off branch's CSR bookkeeping (`offsets`,
                    // `roaring`, `segment_descriptors`) exactly, just
                    // sourced differently — `block_offsets` is deliberately
                    // NOT maintained here (dead in disk mode, see its init
                    // above; plan segments S5b).
                    let term_doc_ids: Vec<u32> =
                        term_postings.iter().map(|posting| posting.doc_id).collect();
                    let term_freqs: Vec<u32> =
                        term_postings.iter().map(|posting| posting.freq).collect();

                    if term_postings.len() > TERM_ROARING_THRESHOLD {
                        roaring.push((idx as u32, RoaringDocSet::from_sorted(&term_doc_ids)));
                    }

                    offsets.push(offsets.last().copied().unwrap_or(0) + term_doc_ids.len() as u32);

                    match encode_postings_blocked(&term_doc_ids, &term_freqs) {
                        Ok(block_payload) => {
                            // SHADOW validation, disk-mode counterpart of the
                            // flag-off branch's post-loop
                            // `validate_field_segment_round_trip` (which
                            // cannot run here: `doc_ids_flat`/`freqs_flat`
                            // stay empty under disk mode, so there is nothing
                            // in RAM to compare the pread'd bytes against).
                            // Checked here instead, against the transient
                            // `term_doc_ids`/`term_freqs` this iteration
                            // owns, before they are dropped. Compiled out in
                            // release builds like every other `debug_assert!`
                            // in this file.
                            debug_assert!(
                            decode_postings_blocked(&block_payload, term_doc_ids.len())
                                .is_ok_and(|(decoded_ids, decoded_freqs)| {
                                    decoded_ids == term_doc_ids && decoded_freqs == term_freqs
                                }),
                            "disk-mode blocked encode round-trip mismatch for field {field:?} term {term:?}"
                        );
                            let payload_offset = field_payload.len() as u64;
                            // Lot C `C1b` sous-pas 2: persist the per-block
                            // directory computed once here (build time), so a
                            // query-time `disk_cursor` never re-preads +
                            // re-decodes the whole term payload just to find
                            // block boundaries (sous-pas 1's ad hoc fallback).
                            if let Ok(directory) =
                                block_directory(&block_payload, term_doc_ids.len())
                            {
                                block_directory_flat.extend(directory);
                            }
                            field_payload.extend_from_slice(&block_payload);
                            segment_descriptors.push((payload_offset, block_payload.len() as u32));
                        }
                        Err(_) => {
                            segment_descriptors.push((0, 0));
                            postings_skipped_terms += 1;
                        }
                    }
                    block_dir_offsets.push(block_directory_flat.len() as u32);
                } else {
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
                    // the `freqs_flat` slice pushed below).
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

                    // SoA freq channel (Lot C Phase 1 levier 5), index-aligned
                    // with `doc_ids_flat` via the same `offsets` CSR table —
                    // this is the last use of the source `Vec<Posting>` the
                    // drained `BTreeMap` entry owned (only borrowed here, via
                    // `.iter()`), so it drops and frees its heap allocation at
                    // the end of this loop iteration (see the transitory-peak
                    // mitigation note above). Eliminates the `doc_id`
                    // duplication the historical AoS `postings_flat:
                    // Box<[Posting]>` carried (`doc_id` was already stored in
                    // `doc_ids_flat` above).
                    let freqs_start = freqs_flat.len();
                    freqs_flat.extend(term_postings.iter().map(|posting| posting.freq));
                    debug_assert!(
                        freqs_flat.len() <= u32::MAX as usize,
                        "freqs_flat offset overflowed u32 — switch offsets to u64 \
                     (only relevant well past matchID's 1.36M-doc scale)"
                    );
                    offsets.push(freqs_flat.len() as u32);

                    // Lot C `C1a-batché`: encode this term's postings as
                    // self-contained FoR blocks and accumulate into the
                    // FIELD-wide payload buffer — NOT written to disk yet
                    // (see the single `postings_segment.append()` after this
                    // loop). `term_postings` is now GUARANTEED strictly
                    // ascending by `doc_id` at this point: the sort at the
                    // top of `build()` plus `dedup_merge_postings` right
                    // after it (Correction fix) sort AND merge every
                    // same-doc_id run into one posting, matching
                    // `encode_postings_blocked`'s strictly-increasing
                    // contract exactly. Same-doc_id runs used to come from a
                    // source `Value::Array` field exploded into several
                    // same-doc_id `(field, value)` pairs upstream
                    // (`indexed_fields_for_document` in surch-api), each
                    // `add`-ed separately — `dedup_merge_postings` is what
                    // now collapses them. The `NotMonotonic` branch below is
                    // kept as defense-in-depth for any other bug that could
                    // still produce non-monotonic doc_ids; best-effort here
                    // means such a term just gets NO disk coverage (sentinel
                    // descriptor) instead of crashing the whole `_refresh` —
                    // RAM (`doc_ids_flat`/`freqs_flat`) already holds every
                    // posting regardless of encode success, so nothing is
                    // lost, only the SHADOW segment's coverage of this one
                    // term.
                    match encode_postings_blocked(
                        &doc_ids_flat[doc_ids_start..],
                        &freqs_flat[freqs_start..],
                    ) {
                        Ok(block_payload) => {
                            let payload_offset = field_payload.len() as u64;
                            field_payload.extend_from_slice(&block_payload);
                            segment_descriptors.push((payload_offset, block_payload.len() as u32));
                        }
                        Err(_) => {
                            // Sentinel: no disk coverage for this term.
                            segment_descriptors.push((0, 0));
                            postings_skipped_terms += 1;
                        }
                    }
                }
            }
            roaring.shrink_to_fit();
            debug_assert_eq!(segment_descriptors.len(), term_count);

            // Lot C `C1a-batché`: ONE `append()` for the WHOLE field —
            // this is the batching contract (see the module doc on
            // `postings_segment`). `field_base_offset` is where this
            // field's payload starts in the SHARED segment; every
            // `segment_descriptors` entry recorded above was relative to
            // `field_payload` (starting at 0), so it is rebased to a
            // segment-absolute offset here.
            //
            // Best-effort: `field_payload` can be non-empty with no
            // segment to receive it (creation failed up front) or the
            // `append()` itself can fail (`ENOSPC`, tmpfs gone away,
            // ...). Either way every descriptor recorded above for THIS
            // field collapses to the sentinel `(0, 0)` — RAM is
            // unaffected, only disk coverage is lost. A write failure
            // additionally drops `postings_segment` to `None`, which
            // best-effort-disables the segment for the REST of the
            // build (an `ENOSPC`-class failure will not un-happen for
            // the next field either, so there is no point retrying).
            // These are NOT counted in `postings_skipped_terms` — that
            // gauge is reserved for genuine per-term encode failures
            // above, so the two failure modes stay distinguishable at
            // read time (`skipped_terms > 0` ⇒ non-monotonic doc_ids,
            // expected to never happen post Correction fix;
            // `skipped_terms == 0` but `postings_segment_bytes() == 0`
            // ⇒ I/O).
            let field_base_offset = if field_payload.is_empty() {
                None
            } else if let Some(segment) = postings_segment.as_mut() {
                let appended = segment.append(&field_payload);
                if appended.is_none() {
                    postings_segment = None;
                }
                appended
            } else {
                None
            };
            match field_base_offset {
                Some(base) => {
                    for descriptor in &mut segment_descriptors {
                        // A `(0, 0)` sentinel from the per-term encode
                        // failure above must stay `(0, 0)` — only
                        // genuinely encoded descriptors get rebased.
                        if descriptor.1 != 0 {
                            descriptor.0 += base;
                        }
                    }
                }
                None => {
                    segment_descriptors.fill((0, 0));
                }
            }
            drop(field_payload);

            // SHADOW validation (Lot C `C1a-batché`): re-`pread` every
            // term's just-flushed bytes from the ACTUAL segment file
            // (not the local buffer) and decode them, comparing
            // bit-for-bit against the RAM source of truth. This exercises
            // the codec AND the pwrite/pread path together. Costs nothing
            // in release builds — `debug_assert!`'s condition, including
            // every `pread` syscall inside `validate_field_segment_round_trip`,
            // is unreachable dead code once `cfg!(debug_assertions)` is
            // `false`. Skips sentinel `(0, 0)` descriptors — see that
            // function's doc comment.
            //
            // Lot C `C1b` sous-pas 2: skipped entirely when `disk_enabled`
            // — `doc_ids_flat`/`freqs_flat` are intentionally empty then
            // (RAM freed), so there is no RAM source of truth left to
            // compare the pread'd bytes against; the disk-mode per-term
            // round trip is instead checked inline, per term, right after
            // each `encode_postings_blocked` call above (see the
            // `disk_enabled` branch of the per-term loop).
            if !disk_enabled {
                debug_assert!(
                    validate_field_segment_round_trip(
                        postings_segment.as_ref(),
                        &segment_descriptors,
                        &offsets,
                        &doc_ids_flat,
                        &freqs_flat,
                    ),
                    "postings segment blocked round-trip mismatch for field {field:?}"
                );
            }

            let bytes = builder
                .into_inner()
                .expect("fst::MapBuilder memory writer never fails I/O");
            let fst = Map::new(bytes).expect("fst::Map from valid MapBuilder bytes");
            // Plan segments S5b: best-effort disk-back of this field's
            // WHOLE per-term directory (descriptors + CSR offsets + block
            // directory) — see `persist_or_keep_term_directory`'s doc
            // comment. No-op (returns every channel resident unchanged,
            // `term_entries_directory: None`) when `disk_enabled` is
            // `false`.
            let tables = persist_or_keep_term_directory(
                TermDirectoryChannels {
                    segment_descriptors,
                    offsets,
                    block_offsets,
                    block_directory: block_directory_flat,
                    block_dir_offsets,
                },
                disk_enabled,
                &mut postings_segment,
            );
            fields.insert(
                field,
                FieldPostings {
                    fst,
                    doc_ids_flat: doc_ids_flat.into_boxed_slice(),
                    freqs_flat: freqs_flat.into_boxed_slice(),
                    block_metas_flat: block_metas_flat.into_boxed_slice(),
                    offsets: tables.offsets,
                    block_offsets: tables.block_offsets,
                    roaring,
                    segment_descriptors: tables.segment_descriptors,
                    term_entries_directory: tables.term_entries_directory,
                    block_directory: tables.block_directory,
                    block_dir_offsets: tables.block_dir_offsets,
                },
            );
        }

        TermDictionary {
            fields,
            postings_segment: postings_segment.map(Arc::new),
            postings_skipped_terms,
            disk_enabled,
        }
    }
}

/// Plan segments S5b: per-field, per-term metadata channels a build
/// (`PostingsBuilder::build_with_disk_flag`) or a merge
/// (`FieldMergeAccumulator::finish`) accumulated, handed to
/// [`persist_or_keep_term_directory`] once the field's per-term loop is
/// done and every descriptor has been rebased to segment-absolute
/// offsets. One struct rather than five parameters: the spill decision
/// is all-or-nothing across the channels (a half-spilled field would
/// leave readers guessing which side is authoritative), and clippy's
/// `too_many_arguments` agrees.
struct TermDirectoryChannels {
    /// Per-term `(offset, len)` into the shared segment — `T` entries.
    segment_descriptors: Vec<(u64, u32)>,
    /// CSR postings offsets — `T + 1` entries (both modes).
    offsets: Vec<u32>,
    /// CSR block-meta offsets — `T + 1` entries in RAM mode, EMPTY in
    /// disk mode (dead there, never built — plan segments S5b).
    block_offsets: Vec<u32>,
    /// Every term's [`BlockDirEntry`] run concatenated (disk mode only).
    block_directory: Vec<BlockDirEntry>,
    /// CSR offsets into `block_directory` — `T + 1` entries in disk
    /// mode, empty in RAM mode.
    block_dir_offsets: Vec<u32>,
}

/// What [`persist_or_keep_term_directory`] resolved
/// [`TermDirectoryChannels`] into: either every channel boxed resident
/// (`term_entries_directory: None` — the flag-off default and every
/// best-effort fallback) or every channel EMPTY with
/// `term_entries_directory: Some((base, term_count))` pointing at the
/// spilled [`TermEntry`] table inside the shared segment.
struct TermDirectoryTables {
    segment_descriptors: Box<[(u64, u32)]>,
    offsets: Box<[u32]>,
    block_offsets: Box<[u32]>,
    block_directory: Box<[BlockDirEntry]>,
    block_dir_offsets: Box<[u32]>,
    term_entries_directory: Option<(u64, u32)>,
}

impl TermDirectoryChannels {
    /// Keep every channel resident (boxed as-is, no spill) — the
    /// flag-off default and the target of every best-effort fallback in
    /// [`persist_or_keep_term_directory`].
    fn into_resident(self) -> TermDirectoryTables {
        TermDirectoryTables {
            segment_descriptors: self.segment_descriptors.into_boxed_slice(),
            offsets: self.offsets.into_boxed_slice(),
            block_offsets: self.block_offsets.into_boxed_slice(),
            block_directory: self.block_directory.into_boxed_slice(),
            block_dir_offsets: self.block_dir_offsets.into_boxed_slice(),
            term_entries_directory: None,
        }
    }
}

/// Plan segments S5b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
/// §"Dimensionnement S5", après le S5a `segment_descriptors`): best-effort
/// disk-back of a field's WHOLE per-term directory — the four remaining
/// `T`-scaled resident arrays (`offsets`, `block_directory`,
/// `block_dir_offsets`, plus the S5a `segment_descriptors`) collapse into
/// ONE fixed-size [`TermEntry`] record per term (28 bytes, see
/// [`encode_term_entry`]) written to the shared `postings_segment` in TWO
/// extra `append()` calls per field (the packed block-directory region,
/// then the `TermEntry` table — same per-field batching contract as the
/// field's own FoR payload, never one write per term). Shared by both
/// producers ([`PostingsBuilder::build_with_disk_flag`] and
/// [`FieldMergeAccumulator::finish`]) so the on-disk layout stays a
/// single source of truth.
///
/// The unified record (rather than one spilled table per array) keeps
/// the query-time cost at ONE `pread` per resolved term for ALL the
/// scalar metadata (postings offset/len/df + block-dir locator), plus
/// one more `pread` for the term's whole block-directory run when a
/// cursor is opened ([`FieldPostings::block_directory_entries`] — read
/// in one grouped shot, never one `pread` per block).
///
/// Falls back to keeping every channel fully resident (returning
/// [`TermDirectoryChannels::into_resident`]) whenever the disk flag is
/// off (unchanged default), the field carries no terms, the channel
/// shapes are inconsistent, the segment is absent, or either metadata
/// `append()` fails (`ENOSPC`, …). On a metadata append failure the
/// segment itself is deliberately kept ALIVE — unlike a FoR-payload
/// append failure (which disables it): under `disk_enabled` the flat RAM
/// channels are empty, so the already-written payload bytes are the ONLY
/// readable copy of the postings, and the resident tables this fallback
/// preserves still address them correctly. Nulling the segment here
/// would silently empty every query on the segment — the exact opposite
/// of "best-effort".
fn persist_or_keep_term_directory(
    channels: TermDirectoryChannels,
    disk_enabled: bool,
    postings_segment: &mut Option<PostingsSegment>,
) -> TermDirectoryTables {
    if !disk_enabled || channels.segment_descriptors.is_empty() {
        return channels.into_resident();
    }
    let term_count = channels.segment_descriptors.len();
    // Producer invariant (both producers push exactly one entry per term
    // into each CSR channel in disk mode) — checked loudly in debug
    // builds, with a graceful resident fallback (never a panic) in
    // release if a future refactor ever breaks it.
    debug_assert_eq!(
        channels.offsets.len(),
        term_count + 1,
        "offsets CSR shape diverged from the descriptor count"
    );
    debug_assert_eq!(
        channels.block_dir_offsets.len(),
        term_count + 1,
        "block_dir_offsets CSR shape diverged from the descriptor count"
    );
    if channels.offsets.len() != term_count + 1
        || channels.block_dir_offsets.len() != term_count + 1
    {
        return channels.into_resident();
    }
    let Some(segment) = postings_segment.as_mut() else {
        return channels.into_resident();
    };

    // (1) The field's whole block directory, packed at
    // BLOCK_DIR_ENTRY_ENCODED_LEN (10 B, no Rust padding) per entry —
    // ONE append for the field.
    let mut encoded_directory = Vec::with_capacity(
        channels
            .block_directory
            .len()
            .saturating_mul(BLOCK_DIR_ENTRY_ENCODED_LEN as usize),
    );
    for entry in &channels.block_directory {
        encoded_directory.extend_from_slice(&encode_block_dir_entry(entry));
    }
    let Some(block_dir_base) = segment.append(&encoded_directory) else {
        return channels.into_resident();
    };

    // (2) The per-term TermEntry table, TERM_ENTRY_ENCODED_LEN (28 B)
    // per term — ONE append for the field. Built only now that
    // `block_dir_base` is known, so every record carries a FINAL
    // segment-absolute block-dir offset (never a to-be-rebased one).
    let mut encoded_entries =
        Vec::with_capacity(term_count.saturating_mul(TERM_ENTRY_ENCODED_LEN as usize));
    for idx in 0..term_count {
        let (postings_offset, postings_len) = channels.segment_descriptors[idx];
        let postings_count = channels.offsets[idx + 1].saturating_sub(channels.offsets[idx]);
        let dir_start = channels.block_dir_offsets[idx];
        let dir_end = channels.block_dir_offsets[idx + 1];
        let entry = TermEntry {
            postings_offset,
            postings_len,
            postings_count,
            block_dir_offset: block_dir_base + u64::from(dir_start) * BLOCK_DIR_ENTRY_ENCODED_LEN,
            block_dir_count: dir_end.saturating_sub(dir_start),
        };
        encoded_entries.extend_from_slice(&encode_term_entry(&entry));
    }
    let Some(entries_base) = segment.append(&encoded_entries) else {
        return channels.into_resident();
    };

    TermDirectoryTables {
        segment_descriptors: Box::default(),
        offsets: Box::default(),
        block_offsets: Box::default(),
        block_directory: Box::default(),
        block_dir_offsets: Box::default(),
        term_entries_directory: Some((entries_base, term_count as u32)),
    }
}

/// S3 (`docs/paper/design-segments-pic-borne-2026-07-05.md`, "merge tiered
/// STREAMING"): same per-block `max_term_freq` computation as
/// [`build_block_metas`], sourced from an already-merged `freqs` slice
/// (the segment merge concatenates `doc_ids`/`freqs` directly — it never
/// reconstructs a `Vec<Posting>`) instead of a `&[Posting]`. Byte-identical
/// output to `build_block_metas` for the same postings: both chunk by
/// [`BLOCK_SIZE`] and take the max `freq` per chunk.
fn build_block_metas_from_freqs(freqs: &[u32]) -> Vec<BlockMeta> {
    freqs
        .chunks(BLOCK_SIZE)
        .map(|chunk| BlockMeta {
            max_term_freq: chunk.iter().copied().max().unwrap_or(0),
        })
        .collect()
}

/// Unique tempfile path for [`MergeFstBuilder::new`], mirroring
/// `postings_segment::next_temp_path`'s pid+seq+nanos scheme (kept
/// independent: that helper is private to the `postings_segment` module,
/// and this one needs plain sequential `std::fs::File` I/O, not the
/// `FileExt` positional read/write `postings_segment` uses — no
/// `#[cfg(unix)]` split needed here).
fn merge_fst_temp_path() -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("surch-fst-merge-{pid}-{seq}-{nanos}.dat"))
}

/// Best-effort tempfile open for [`MergeFstBuilder::new`] — `None` on any
/// I/O failure (unwritable `TMPDIR`, `ENOSPC`, ...), in which case the
/// caller falls back to `MapBuilder::memory()` (same contract as
/// [`postings_segment::PostingsSegment::try_new`]).
fn create_merge_fst_tempfile() -> Option<(std::fs::File, std::path::PathBuf)> {
    let path = merge_fst_temp_path();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    Some((file, path))
}

/// Plan segments — Q6 (design doc `docs/paper/design-segments-pic-borne-2026-07-05.md`,
/// challenge C1 diagnostic): backing store for
/// [`FieldMergeAccumulator`]'s `fst::MapBuilder`. A large tiered merge
/// (the design's "paliers ×10, fan-in ~4-10") can touch a term dictionary
/// of 10-15M distinct terms (edge_ngram/prefix fields inflate `T` well
/// past the doc count) — `MapBuilder::memory()`'s `Vec<u8>` sink then
/// holds the WHOLE compressed FST resident WHILE it grows (geometric
/// reallocation spikes toward ~2x the final size at the worst moment),
/// hundreds of MiB transient on top of everything else already live
/// during a merge. `Streamed` writes incrementally to a tempfile through
/// a bounded `BufWriter` (tens of KiB resident regardless of key count)
/// and the final bytes are read back ONCE, sized exactly (`fs::read`, no
/// growth doubling) — peak drops from ~2x-final-FST-size to ~1x. Falls
/// back to `Memory` when the tempfile cannot even be opened — same
/// best-effort contract as [`postings_segment::PostingsSegment::try_new`]
/// elsewhere in this module: never let an optimization crash indexation.
///
/// Ordinary per-segment seals (`PostingsBuilder::build_with_disk_flag`)
/// deliberately keep `MapBuilder::memory()` unchanged: a single
/// budget-bounded segment's own FST is already small (the design doc:
/// "les FST de seal de petits segments peuvent rester memory()") — only
/// the MERGE path, which can span a whole large tier, gets the streaming
/// treatment.
///
/// Known residual risk (flagged by independent review, design doc §C1
/// fix): the best-effort contract above only covers the tempfile OPEN.
/// Once opened, a LATE write/flush/read-back failure (`ENOSPC`/`EIO` on
/// `TMPDIR` mid-merge) still propagates as a panic via the caller's
/// `.expect()` (`FieldMergeAccumulator::push_term`/`finish`) — a
/// narrower failure mode than `MapBuilder::memory()` ever had (an
/// in-memory `Vec<u8>` write essentially cannot fail short of the
/// process already being OOM), so this is NOT a strict risk regression
/// in the impossible-in-practice sense, but it IS a new, real, rare
/// panic surface (a full disk under a big merge) this pass accepts
/// rather than solves: recovering would need buffering enough state to
/// replay the merge in memory, which re-introduces the very transient
/// this fix exists to avoid. Left as a documented follow-up.
enum MergeFstBuilder {
    Streamed {
        builder: MapBuilder<std::io::BufWriter<std::fs::File>>,
        path: std::path::PathBuf,
    },
    Memory(MapBuilder<Vec<u8>>),
}

impl MergeFstBuilder {
    fn new() -> Self {
        if let Some((file, path)) = create_merge_fst_tempfile() {
            if let Ok(builder) = MapBuilder::new(std::io::BufWriter::new(file)) {
                return MergeFstBuilder::Streamed { builder, path };
            }
            let _ = std::fs::remove_file(&path);
        }
        MergeFstBuilder::Memory(MapBuilder::memory())
    }

    fn insert(&mut self, key: &[u8], value: u64) -> fst::Result<()> {
        match self {
            MergeFstBuilder::Streamed { builder, .. } => builder.insert(key, value),
            MergeFstBuilder::Memory(builder) => builder.insert(key, value),
        }
    }

    /// Finishes the FST and returns its encoded bytes, ready for
    /// `fst::Map::new`. `MapBuilder::into_inner` already flushes the
    /// underlying writer (see the `fst` crate's own doc comment on that
    /// method), so the streamed tempfile is fully written by the time we
    /// read it back with a single `fs::read` — then removed, the same
    /// tempfile-cleanup contract every other disk-backed segment in this
    /// crate follows.
    fn finish(self) -> Vec<u8> {
        match self {
            MergeFstBuilder::Streamed { builder, path } => {
                let bytes = builder
                    .into_inner()
                    .ok()
                    .and_then(|_flushed_writer| std::fs::read(&path).ok())
                    .expect(
                        "streamed fst tempfile should flush and be readable right after writing it",
                    );
                let _ = std::fs::remove_file(&path);
                bytes
            }
            MergeFstBuilder::Memory(builder) => builder
                .into_inner()
                .expect("fst::MapBuilder memory writer never fails I/O"),
        }
    }
}

/// S3 merge tiered inline: per-field accumulator for
/// [`merge_term_dictionaries`], bundling the same channels
/// [`PostingsBuilder::build_with_disk_flag`]'s per-field loop accumulates
/// (FST builder, CSR offsets, the sparse roaring side table, the disk
/// segment payload, the persisted block directory) behind one struct —
/// avoids a `too_many_arguments` free function, and is the single place a
/// future S3b fast path would change (see [`Self::push_term`]'s doc
/// comment).
struct FieldMergeAccumulator {
    disk_enabled: bool,
    builder: MergeFstBuilder,
    doc_ids_flat: Vec<u32>,
    freqs_flat: Vec<u32>,
    block_metas_flat: Vec<BlockMeta>,
    offsets: Vec<u32>,
    block_offsets: Vec<u32>,
    roaring: Vec<(u32, RoaringDocSet)>,
    segment_descriptors: Vec<(u64, u32)>,
    field_payload: Vec<u8>,
    block_directory_flat: Vec<BlockDirEntry>,
    block_dir_offsets: Vec<u32>,
    postings_skipped_terms: u64,
    next_term_idx: u32,
}

impl FieldMergeAccumulator {
    fn new(disk_enabled: bool) -> Self {
        let mut block_dir_offsets = Vec::new();
        if disk_enabled {
            block_dir_offsets.push(0);
        }
        // Plan segments S5b: `block_offsets` is dead in disk mode (same
        // rationale as `PostingsBuilder::build_with_disk_flag`'s init) —
        // only the RAM merge maintains it.
        let block_offsets = if disk_enabled { Vec::new() } else { vec![0] };
        Self {
            disk_enabled,
            builder: MergeFstBuilder::new(),
            doc_ids_flat: Vec::new(),
            freqs_flat: Vec::new(),
            block_metas_flat: Vec::new(),
            offsets: vec![0],
            block_offsets,
            roaring: Vec::new(),
            segment_descriptors: Vec::new(),
            field_payload: Vec::new(),
            block_directory_flat: Vec::new(),
            block_dir_offsets,
            postings_skipped_terms: 0,
            next_term_idx: 0,
        }
    }

    /// Streaming per-term merge step. `term` must be strictly greater
    /// (lexicographically) than every previously pushed term — guaranteed
    /// by [`merge_term_dictionaries`]'s `fst::map::OpBuilder::union`
    /// stream, which already walks every source field's FST in sorted
    /// lock-step. `doc_ids`/`freqs` are this term's postings, already
    /// concatenated across the merge run's segments in ascending
    /// `doc_base` order by the caller.
    ///
    /// Fable's arbitrage (design doc, doc_ids GLOBAUX STABLES) keeps
    /// doc_ids global and never renumbers them, and a run is only ever
    /// ADJACENT sealed segments, so the concatenation the caller built is
    /// already strictly ascending with NO overlap: unlike
    /// `PostingsBuilder::build`'s `dedup_merge_postings` (which exists for
    /// a same-DOCUMENT multi-valued field, a write-time concern), there is
    /// no run of equal doc_ids to merge here — only a debug-only sanity
    /// check that the invariant actually holds.
    ///
    /// v1 (this commit) is decode+concat+re-encode: the caller already
    /// materialized `doc_ids`/`freqs` by reading back each source
    /// segment's postings (a RAM slice, or one `pread`+decode when that
    /// source is disk-backed) and concatenating them; this method
    /// re-encodes them exactly like a fresh build. The planned S3b fast
    /// path (design doc: "copie verbatim des blobs FoR avec fixup du seul
    /// premier varint") only needs to replace the body of
    /// [`Self::encode_and_store`] below with a byte-copy of each source's
    /// already-FoR-encoded block payload (re-based varint-delta on the
    /// very first doc_id of the run only) instead of calling
    /// `encode_postings_blocked` on decoded `u32`s — every other channel
    /// here (FST insert, CSR offsets, roaring, directory) is already
    /// agnostic to how the payload bytes were produced, so this is the
    /// ONLY hook that path needs to change.
    fn push_term(&mut self, term: &[u8], doc_ids: &[u32], freqs: &[u32]) {
        debug_assert_eq!(doc_ids.len(), freqs.len());
        debug_assert!(
            doc_ids.windows(2).all(|pair| pair[0] < pair[1]),
            "merged term postings must be strictly ascending doc_id — adjacent \
             sealed segments must never overlap (S3 carries no tombstones yet)"
        );
        let term_idx = self.next_term_idx;
        self.next_term_idx += 1;
        self.builder.insert(term, u64::from(term_idx)).expect(
            "fst union stream must yield strictly increasing keys (a failure here is a \
             logic bug) OR, in MergeFstBuilder::Streamed mode, the tempfile write just \
             failed (ENOSPC/EIO on TMPDIR) — a narrower, accepted best-effort risk this \
             pass does not recover from gracefully, see MergeFstBuilder's doc comment",
        );

        if self.disk_enabled {
            if doc_ids.len() > TERM_ROARING_THRESHOLD {
                self.roaring
                    .push((term_idx, RoaringDocSet::from_sorted(doc_ids)));
            }
            self.offsets
                .push(self.offsets.last().copied().unwrap_or(0) + doc_ids.len() as u32);
            self.encode_and_store(doc_ids, freqs);
            self.block_dir_offsets
                .push(self.block_directory_flat.len() as u32);
        } else {
            self.block_metas_flat
                .extend(build_block_metas_from_freqs(freqs));
            self.block_offsets.push(self.block_metas_flat.len() as u32);
            let doc_ids_start = self.doc_ids_flat.len();
            self.doc_ids_flat.extend_from_slice(doc_ids);
            if doc_ids.len() > TERM_ROARING_THRESHOLD {
                self.roaring.push((
                    term_idx,
                    RoaringDocSet::from_sorted(&self.doc_ids_flat[doc_ids_start..]),
                ));
            }
            self.freqs_flat.extend_from_slice(freqs);
            self.offsets.push(self.freqs_flat.len() as u32);
            self.encode_and_store(doc_ids, freqs);
        }
    }

    /// FoR-encode `(doc_ids, freqs)` and accumulate into `field_payload`,
    /// mirroring [`PostingsBuilder::build_with_disk_flag`]'s per-term
    /// encode step (persisted block directory only when `disk_enabled`,
    /// same sentinel-on-failure contract).
    fn encode_and_store(&mut self, doc_ids: &[u32], freqs: &[u32]) {
        match encode_postings_blocked(doc_ids, freqs) {
            Ok(block_payload) => {
                let payload_offset = self.field_payload.len() as u64;
                if self.disk_enabled {
                    if let Ok(directory) = block_directory(&block_payload, doc_ids.len()) {
                        self.block_directory_flat.extend(directory);
                    }
                }
                self.field_payload.extend_from_slice(&block_payload);
                self.segment_descriptors
                    .push((payload_offset, block_payload.len() as u32));
            }
            Err(_) => {
                self.segment_descriptors.push((0, 0));
                self.postings_skipped_terms += 1;
            }
        }
    }

    /// Finish this field: flush `field_payload` into the SHARED
    /// `postings_segment` with ONE `append()` call (same per-field
    /// batching contract as [`PostingsBuilder::build_with_disk_flag`]),
    /// rebase `segment_descriptors`, and assemble the final
    /// [`FieldPostings`]. Returns the skipped-terms count for the caller
    /// to fold into [`TermDictionary::postings_skipped_terms`].
    fn finish(mut self, postings_segment: &mut Option<PostingsSegment>) -> (FieldPostings, u64) {
        self.roaring.shrink_to_fit();
        let field_base_offset = if self.field_payload.is_empty() {
            None
        } else if let Some(segment) = postings_segment.as_mut() {
            let appended = segment.append(&self.field_payload);
            if appended.is_none() {
                *postings_segment = None;
            }
            appended
        } else {
            None
        };
        match field_base_offset {
            Some(base) => {
                for descriptor in &mut self.segment_descriptors {
                    if descriptor.1 != 0 {
                        descriptor.0 += base;
                    }
                }
            }
            None => {
                self.segment_descriptors.fill((0, 0));
            }
        }

        // SHADOW validation, same contract (and same free-in-release
        // `debug_assert!`) as `PostingsBuilder::build_with_disk_flag`'s
        // post-loop round trip: re-`pread` every merged term's
        // just-flushed bytes from the actual segment file and compare
        // against the RAM channels. RAM mode only — under `disk_enabled`
        // the flat channels are intentionally empty (the per-term encode
        // was instead sanity-checked by `push_term`'s strict-ascending
        // `debug_assert!` on the exact slices that were encoded).
        if !self.disk_enabled {
            debug_assert!(
                validate_field_segment_round_trip(
                    postings_segment.as_ref(),
                    &self.segment_descriptors,
                    &self.offsets,
                    &self.doc_ids_flat,
                    &self.freqs_flat,
                ),
                "merged postings segment blocked round-trip mismatch"
            );
        }

        let bytes = self.builder.finish();
        let fst = Map::new(bytes).expect("fst::Map from valid MapBuilder bytes");
        // Plan segments S5b: same best-effort disk-back as
        // `PostingsBuilder::build_with_disk_flag` — see
        // `persist_or_keep_term_directory`'s doc comment.
        let tables = persist_or_keep_term_directory(
            TermDirectoryChannels {
                segment_descriptors: self.segment_descriptors,
                offsets: self.offsets,
                block_offsets: self.block_offsets,
                block_directory: self.block_directory_flat,
                block_dir_offsets: self.block_dir_offsets,
            },
            self.disk_enabled,
            postings_segment,
        );
        (
            FieldPostings {
                fst,
                doc_ids_flat: self.doc_ids_flat.into_boxed_slice(),
                freqs_flat: self.freqs_flat.into_boxed_slice(),
                block_metas_flat: self.block_metas_flat.into_boxed_slice(),
                offsets: tables.offsets,
                block_offsets: tables.block_offsets,
                roaring: self.roaring,
                segment_descriptors: tables.segment_descriptors,
                term_entries_directory: tables.term_entries_directory,
                block_directory: tables.block_directory,
                block_dir_offsets: tables.block_dir_offsets,
            },
            self.postings_skipped_terms,
        )
    }
}

/// S3 (`docs/paper/design-segments-pic-borne-2026-07-05.md`, ordre amendé
/// "S3: merge tiered inline sur runs adjacents"): merge a run of ADJACENT
/// sealed segments' [`TermDictionary`]s into ONE, streaming per term — the
/// merge never materializes a whole segment's postings in RAM at once,
/// only one term's merged postings at a time (v1 scope: "simple et correct
/// d'abord" — see [`FieldMergeAccumulator::push_term`]'s doc comment for
/// the planned S3b verbatim fast path).
///
/// `sources` MUST be the run's `TermDictionary`s in ascending `doc_base`
/// order — the only caller, `document_index::DocumentIndex`'s merge
/// policy, only ever builds runs of CONSECUTIVE `segments` entries, which
/// are always kept in ascending `doc_base` order by construction (segments
/// are only ever pushed, never reordered). Fable's arbitrage keeps
/// doc_ids global and never renumbers them, so a term's per-source
/// postings, concatenated in `sources` order, are already one strictly
/// ascending list — no re-sort, no dedup needed (S3 carries no tombstones
/// yet, see the design doc's §C5).
///
/// For each field present in at least one source, a k-way `fst::Map`
/// union (`fst::map::OpBuilder`, one stream per participating source)
/// walks every source's FST in sorted lock-step — this is what gives the
/// merged term order for free, with no separate sort pass, exactly like
/// `PostingsBuilder::build`'s `BTreeMap` does for a fresh build. For every
/// resulting term, the union's `IndexedValue`s hand back each
/// participating source's own FST output (the term idx) directly, so the
/// per-source postings are resolved with NO second FST lookup: a
/// RAM-backed source is read from its resident CSR `offsets` +
/// `doc_ids_flat`/`freqs_flat` slices, a disk-backed one through its
/// [`FieldPostings::term_entry`] record (resident-or-pread — plan
/// segments S5b — then one `pread` + [`decode_postings_blocked`] for the
/// payload, the same bytes [`TermDictionary::decode_from_segment`] would
/// read, minus that method's redundant FST resolution). A disk source
/// term carrying the sentinel `postings_len == 0` (no disk coverage — an
/// I/O failure at ITS build; those postings are then already unreadable
/// by every production query on that segment) contributes nothing,
/// matching `decode_from_segment`'s best-effort contract.
///
/// `disk_enabled` selects the MERGED dictionary's own RAM/disk layout. The
/// caller passes `DocumentIndex`'s resolved postings-disk flag, which
/// (being fixed for the process's/index's whole lifetime — see
/// [`postings_disk_enabled`]'s doc comment) necessarily already agrees
/// with every source's own `disk_backed()`.
pub(crate) fn merge_term_dictionaries(
    sources: &[&TermDictionary],
    disk_enabled: bool,
) -> TermDictionary {
    let mut field_names: BTreeSet<&str> = BTreeSet::new();
    for source in sources {
        field_names.extend(source.fields.keys().map(String::as_str));
    }

    let mut postings_segment = PostingsSegment::try_new();
    let mut postings_skipped_terms: u64 = 0;
    let mut fields: BTreeMap<String, FieldPostings> = BTreeMap::new();

    for field in field_names {
        let participants: Vec<(&TermDictionary, &FieldPostings)> = sources
            .iter()
            .copied()
            .filter_map(|source| source.fields.get(field).map(|fp| (source, fp)))
            .collect();
        if participants.is_empty() {
            continue;
        }

        let mut op = participants[0].1.fst.op();
        for (_, fp) in &participants[1..] {
            op = op.add(&fp.fst);
        }
        let mut union_stream = op.union();

        let mut acc = FieldMergeAccumulator::new(disk_enabled);
        let mut merged_doc_ids: Vec<u32> = Vec::new();
        let mut merged_freqs: Vec<u32> = Vec::new();
        while let Some((term_bytes, indexed_values)) = union_stream.next() {
            merged_doc_ids.clear();
            merged_freqs.clear();
            // Concatenate in ascending `doc_base` order == `participants`
            // order (== `sources` order): each participant's `IndexedValue`
            // is matched back by its stream index (a linear `find` over
            // <= fan-in entries) rather than trusting any internal
            // ordering of `indexed_values` itself.
            for (participant_idx, (source, fp)) in participants.iter().enumerate() {
                let Some(indexed) = indexed_values
                    .iter()
                    .find(|indexed| indexed.index == participant_idx)
                else {
                    continue;
                };
                let term_idx = indexed.value as usize;
                if source.disk_backed() {
                    // Plan segments S5b: a disk-backed source's CSR
                    // `offsets` may be EMPTY (spilled into its TermEntry
                    // table), so everything — descriptor AND df — comes
                    // from the unified accessor, resident-or-pread. The
                    // resident indexing below is RAM-branch-only.
                    let segment = source.postings_segment.as_deref();
                    let Some(entry) = fp.term_entry(term_idx, segment) else {
                        continue;
                    };
                    if entry.postings_len == 0 {
                        continue;
                    }
                    let Some(segment) = segment else {
                        continue;
                    };
                    let Some(bytes) = segment.read(entry.postings_offset, entry.postings_len)
                    else {
                        continue;
                    };
                    let Ok((ids, freqs)) =
                        decode_postings_blocked(&bytes, entry.postings_count as usize)
                    else {
                        continue;
                    };
                    merged_doc_ids.extend(ids);
                    merged_freqs.extend(freqs);
                } else {
                    let start = fp.offsets[term_idx] as usize;
                    let end = fp.offsets[term_idx + 1] as usize;
                    merged_doc_ids.extend_from_slice(&fp.doc_ids_flat[start..end]);
                    merged_freqs.extend_from_slice(&fp.freqs_flat[start..end]);
                }
            }
            acc.push_term(term_bytes, &merged_doc_ids, &merged_freqs);
        }

        let (field_postings, skipped) = acc.finish(&mut postings_segment);
        postings_skipped_terms += skipped;
        fields.insert(field.to_string(), field_postings);
    }

    TermDictionary {
        fields,
        postings_segment: postings_segment.map(Arc::new),
        postings_skipped_terms,
        disk_enabled,
    }
}

/// SHADOW validation (Lot C `C1a-batché`, only ever called from inside a
/// `debug_assert!` in [`PostingsBuilder::build`]): re-`pread`s every
/// term's just-flushed bytes from `segment` and decodes them via
/// [`decode_postings_blocked`], comparing bit-for-bit against the RAM
/// source of truth (`doc_ids_flat`/`freqs_flat`, sliced by `offsets`).
/// Returns `false` on the first mismatch or decode error.
///
/// Best-effort hardening: `segment` is `None` when the segment could not
/// be created (or was disabled mid-build) — the whole field is then made
/// of sentinel descriptors, so there is nothing to validate and this
/// trivially returns `true`. A sentinel `(0, 0)` descriptor (per-term
/// encode failure, see [`PostingsBuilder::build`]) is likewise skipped
/// term-by-term: it was never encoded, so there is no round trip to
/// check for it.
fn validate_field_segment_round_trip(
    segment: Option<&PostingsSegment>,
    segment_descriptors: &[(u64, u32)],
    offsets: &[u32],
    doc_ids_flat: &[u32],
    freqs_flat: &[u32],
) -> bool {
    let Some(segment) = segment else {
        return true;
    };
    for (idx, &(offset, len)) in segment_descriptors.iter().enumerate() {
        if len == 0 {
            continue;
        }
        let start = offsets[idx] as usize;
        let end = offsets[idx + 1] as usize;
        let expected_doc_ids = &doc_ids_flat[start..end];
        let expected_freqs = &freqs_flat[start..end];

        let Some(bytes) = segment.read(offset, len) else {
            return false;
        };
        let Ok((decoded_doc_ids, decoded_freqs)) =
            decode_postings_blocked(&bytes, expected_doc_ids.len())
        else {
            return false;
        };
        if decoded_doc_ids != expected_doc_ids || decoded_freqs != expected_freqs {
            return false;
        }
    }
    true
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
///
/// Lot C Phase 1 levier 5: the original AoS `postings_flat: Box<[Posting]>`
/// channel duplicated `doc_id` a second time — once inside every
/// `Posting { doc_id, freq }`, once more in `doc_ids_flat` for the
/// leapfrog. It has been replaced by a SoA split: `doc_ids_flat` is kept
/// byte-for-byte as before (still the ONLY channel the conjunction
/// leapfrog / `PostingsBlockSkipIter` walks, so its cache footprint is
/// unchanged), and `freqs_flat` below carries just the `freq` values,
/// index-aligned with `doc_ids_flat` via the same `offsets` table.
///
/// `lookup*` hands back the two slices together instead of one
/// `&[Posting]`; scoring reads `doc_id` from `doc_ids_flat[i]` and `freq`
/// from `freqs_flat[i]`.
#[derive(Debug, Clone)]
pub struct FieldPostings {
    fst: Map<Vec<u8>>,
    /// All terms' `doc_id` channel concatenated, in FST idx order,
    /// ascending `doc_id` within each term (same order as the historical
    /// per-term `Vec<Posting>`). Exactly `Σdf` long — sized once in
    /// `PostingsBuilder::build()`'s pass 1, so `capacity == len` and this
    /// `Box<[u32]>` carries no growth slack. The conjunction leapfrog
    /// scans ONLY this channel (the `freq` is read by index lookup into
    /// `freqs_flat` at scoring time), so it touches half the bytes (4 vs
    /// 8/posting) a `[Posting]` slice would — fewer cache lines on the
    /// high-`df` conjunction tail.
    doc_ids_flat: Box<[u32]>,
    /// All terms' `freq` channel concatenated the same way, index-aligned
    /// with `doc_ids_flat` (same `offsets` table): posting `i` of a term
    /// is `(doc_ids_flat[start + i], freqs_flat[start + i])`. Read ONLY at
    /// scoring time — never by the leapfrog, which stays on `doc_ids_flat`
    /// above — so this channel adds no cache pressure to the doc_id-only
    /// hot path.
    freqs_flat: Box<[u32]>,
    /// All terms' `BlockMeta` chunks concatenated, in FST idx order. Term
    /// `i`'s blocks are `block_metas_flat[block_offsets[i]..
    /// block_offsets[i+1]]`; inside that range, block `j` describes
    /// `doc_ids_flat[offsets[i] + j*BLOCK_SIZE .. offsets[i] +
    /// min((j+1)*BLOCK_SIZE, df)]` (and the same range of `freqs_flat`).
    /// Built once in `PostingsBuilder::build()` and never mutated
    /// afterwards.
    block_metas_flat: Box<[BlockMeta]>,
    /// CSR offsets into `doc_ids_flat` / `freqs_flat`, length `T + 1`
    /// (`T` = number of distinct terms in this field). Term `i`'s doc_ids
    /// are `doc_ids_flat[offsets[i]..offsets[i+1]]` (and likewise for
    /// `freqs_flat`, which shares the same offsets since both channels
    /// are index-aligned). `offsets[0] == 0` and `offsets[T] ==
    /// doc_ids_flat.len()`. `u32` is enough for matchID's 1.36 M docs;
    /// `PostingsBuilder::build()` carries a `debug_assert` that the
    /// cumulative count fits, with a comment on switching to `u64` if a
    /// future corpus needs more than ~4.29 B total postings in one field.
    ///
    /// Plan segments S5b: `Box::default()` (empty) when
    /// [`Self::term_entries_directory`] is `Some(_)` — each term's `df`
    /// then comes from its spilled [`TermEntry::postings_count`] instead
    /// of an `offsets[i+1] - offsets[i]` difference.
    offsets: Box<[u32]>,
    /// CSR offsets into `block_metas_flat`, length `T + 1`, same shape as
    /// `offsets` but for the (coarser) block-meta channel.
    ///
    /// Plan segments S5b: RAM mode only — DEAD in disk mode (its only
    /// readers, [`Self::lookup_block_metas`] /
    /// [`Self::lookup_with_block_metas`], index `block_metas_flat`, which
    /// is empty under the disk flag, and every `surch-api` call site
    /// branches on `postings_disk_backed()` first), so disk-mode builds
    /// leave it empty from the start, neither resident nor spilled.
    block_offsets: Box<[u32]>,
    /// SPARSE side table: only terms whose `df > TERM_ROARING_THRESHOLD`
    /// get an entry — `(term_idx, bitmap)`, sorted ascending by `term_idx`
    /// (built that way, see `PostingsBuilder::build()`) so `lookup` can
    /// `binary_search_by_key` it in O(log terms_over_threshold) instead of
    /// paying one `Vec` slot per LOW-df term the way the historical
    /// `Vec<Option<RoaringDocSet>>` (one slot per term, `None` almost
    /// everywhere) did.
    roaring: Vec<(u32, RoaringDocSet)>,
    /// Lot C `C1a-batché` SHADOW: per-term `(offset, len)` descriptor into
    /// the shared segment (see [`TermDictionary::postings_segment`]),
    /// index-aligned with `offsets` by FST idx — `T` entries, ONE per
    /// term (not a CSR table: each descriptor already spans the term's
    /// whole FoR-blocked payload, so there is no cumulative-offset
    /// pattern to exploit here). Written once in `PostingsBuilder::build()`,
    /// read only by [`TermDictionary::decode_from_segment`] (validation /
    /// future cold paths) — never by `lookup*`/scoring/the leapfrog,
    /// which stay on `doc_ids_flat`/`freqs_flat` above.
    ///
    /// Plan segments S5 (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §"Dimensionnement S5"): this `T`-scaled array was identified as the
    /// single largest previously-ungauged RAM component (16 B/term after
    /// Rust struct padding — a field with high term cardinality, e.g. an
    /// `edge_ngram`/`autocomplete` analyzer or `index_prefixes`, can push
    /// `T` into the millions even on a modest doc count). `Box::default()`
    /// (empty) when [`Self::term_entries_directory`] is `Some(_)` —
    /// see that field's doc comment for the disk-backed replacement.
    segment_descriptors: Box<[(u64, u32)]>,
    /// Plan segments S5b (extends the S5a descriptor spill): when
    /// `Some((entries_base, term_count))`, EVERY `T`-scaled resident
    /// array of this struct (`segment_descriptors`, `offsets`,
    /// `block_offsets`, `block_directory`, `block_dir_offsets`) is EMPTY
    /// and each term's whole per-term metadata instead lives in the
    /// SHARED `postings_segment` file as ONE packed 28-byte [`TermEntry`]
    /// record (see [`encode_term_entry`]) at byte `entries_base +
    /// term_idx * TERM_ENTRY_ENCODED_LEN`, written ONCE by
    /// [`persist_or_keep_term_directory`] right after the field's own FoR
    /// payload and its packed block-directory region (same per-field
    /// batching contract, two extra `append()`s). Resolved by
    /// [`FieldPostings::term_entry`] with one `pread` for ALL the scalar
    /// per-term metadata (postings offset/len/df + block-dir locator) —
    /// the S5 trade-off already accepted for the FoR payload itself
    /// (C1b) — and the term's block-directory run is then fetched in one
    /// MORE grouped `pread` by [`FieldPostings::block_directory_entries`]
    /// when a cursor opens (never one `pread` per block).
    /// `None` when the postings-disk flag (`SURCH_POSTINGS_DISK`) was off
    /// at build time (the unchanged, 100 %-resident default) or when
    /// persisting the directory failed (best-effort: falls back to the
    /// resident arrays above, same philosophy as every other
    /// segment-write failure in this file).
    term_entries_directory: Option<(u64, u32)>,
    /// Lot C `C1b` sous-pas 2: per-block directory (`max_doc_id` per FoR
    /// block, [`BlockDirEntry`]), persisted ONCE at
    /// [`PostingsBuilder::build_with_disk_flag`] time — CSR-indexed by
    /// [`Self::block_dir_offsets`], concatenated across every term in FST
    /// idx order. Built ONLY when the disk flag is on for this build;
    /// `Box::default()` (empty, zero heap) otherwise, so the RAM-only
    /// engine pays nothing for it. Lets [`TermDictionary::disk_cursor`]
    /// open a block-addressed cursor in O(term's block count) — a
    /// directory LOOKUP — instead of `pread`-ing and decoding the term's
    /// WHOLE payload just to compute the directory (sous-pas 1's ad hoc
    /// fallback, still used when no directory is available for a term).
    ///
    /// Plan segments S5b: `Box::default()` (empty) when
    /// [`Self::term_entries_directory`] is `Some(_)` — the entries then
    /// live in the shared segment as packed 10-byte records
    /// ([`encode_block_dir_entry`]), addressed per term by
    /// [`TermEntry::block_dir_offset`]/[`TermEntry::block_dir_count`] and
    /// fetched in ONE grouped `pread` per cursor open
    /// ([`Self::block_directory_entries`]).
    block_directory: Box<[BlockDirEntry]>,
    /// CSR offsets into `block_directory`, length `T + 1` when the disk
    /// flag was on for this build, empty (`Box::default()`) otherwise —
    /// same shape/contract as `offsets`/`block_offsets`. Plan segments
    /// S5b: also empty once spilled (see
    /// [`Self::term_entries_directory`]).
    block_dir_offsets: Box<[u32]>,
}

/// Plan segments S5b: one term's COMPLETE per-term metadata, unified
/// across what used to be four separate resident arrays (S5a's
/// `segment_descriptors` + the CSR `offsets` df + the
/// `block_dir_offsets`-CSR-indexed `block_directory` locator). ONE
/// fixed-size record per term so the disk-backed read path resolves
/// everything scalar about a term in ONE `pread`
/// ([`FieldPostings::term_entry`]).
///
/// Sentinel contract (mirrors the historical `(0, 0)` descriptor):
/// `postings_len == 0` means NO disk coverage for the term (its FoR
/// encode failed at build time, or the payload append failed) — readers
/// must not interpret any other field then. `postings_offset == 0` with
/// `postings_len > 0` is a perfectly valid record (the first term of the
/// first field written to the segment). `block_dir_count == 0` with
/// `postings_len > 0` means the payload is readable but carries no
/// persisted block directory (its `block_directory()` computation failed
/// at build time) — [`TermDictionary::disk_cursor`] then falls back to
/// [`DiskPostingsCursor::open`]'s recompute instead of treating the term
/// as empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TermEntry {
    /// Segment-absolute byte offset of the term's FoR payload.
    postings_offset: u64,
    /// Byte length of the term's FoR payload (`0` = sentinel, see above).
    postings_len: u32,
    /// The term's `df` (posting count) — what `offsets[i+1] - offsets[i]`
    /// used to provide.
    postings_count: u32,
    /// Where the term's [`BlockDirEntry`] run starts. Directory spilled
    /// ([`FieldPostings::term_entries_directory`] `Some`): segment-absolute
    /// byte offset of the packed 10-byte records. Resident fallback:
    /// START INDEX into the resident `block_directory` array (an entry
    /// count, not bytes) — [`FieldPostings::block_directory_entries`] is
    /// the single reader and branches accordingly.
    block_dir_offset: u64,
    /// Number of [`BlockDirEntry`] records in the term's run (`0` = no
    /// persisted directory, cursor recomputes — see the sentinel
    /// contract above).
    block_dir_count: u32,
}

/// Plan segments S5b: on-disk encoded length of one [`TermEntry`] — 28
/// bytes packed with NO padding (`u64 + u32 + u32 + u64 + u32`), only
/// ever read back through [`decode_term_entry`], never transmuted.
const TERM_ENTRY_ENCODED_LEN: u64 = 28;

/// Plan segments S5b: pack one [`TermEntry`] into the tight 28-byte
/// on-disk format ([`TERM_ENTRY_ENCODED_LEN`]), little-endian.
fn encode_term_entry(entry: &TermEntry) -> [u8; TERM_ENTRY_ENCODED_LEN as usize] {
    let mut buf = [0u8; TERM_ENTRY_ENCODED_LEN as usize];
    buf[0..8].copy_from_slice(&entry.postings_offset.to_le_bytes());
    buf[8..12].copy_from_slice(&entry.postings_len.to_le_bytes());
    buf[12..16].copy_from_slice(&entry.postings_count.to_le_bytes());
    buf[16..24].copy_from_slice(&entry.block_dir_offset.to_le_bytes());
    buf[24..28].copy_from_slice(&entry.block_dir_count.to_le_bytes());
    buf
}

/// Plan segments S5b: inverse of [`encode_term_entry`]. Returns `None`
/// if `bytes` is shorter than [`TERM_ENTRY_ENCODED_LEN`] (a truncated
/// `pread`, treated the same as every other best-effort I/O failure in
/// this file: the caller sees "no disk coverage" for that term).
fn decode_term_entry(bytes: &[u8]) -> Option<TermEntry> {
    Some(TermEntry {
        postings_offset: u64::from_le_bytes(bytes.get(0..8)?.try_into().ok()?),
        postings_len: u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?),
        postings_count: u32::from_le_bytes(bytes.get(12..16)?.try_into().ok()?),
        block_dir_offset: u64::from_le_bytes(bytes.get(16..24)?.try_into().ok()?),
        block_dir_count: u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?),
    })
}

/// Plan segments S5b: on-disk encoded length of one [`BlockDirEntry`] —
/// 10 bytes packed with NO padding (`u32 + u16 + u32`; the in-RAM struct
/// pads to 12 on `size_of`-driven gauges).
const BLOCK_DIR_ENTRY_ENCODED_LEN: u64 = 10;

/// Plan segments S5b: pack one [`BlockDirEntry`] into the tight 10-byte
/// on-disk format ([`BLOCK_DIR_ENTRY_ENCODED_LEN`]), little-endian.
fn encode_block_dir_entry(entry: &BlockDirEntry) -> [u8; BLOCK_DIR_ENTRY_ENCODED_LEN as usize] {
    let mut buf = [0u8; BLOCK_DIR_ENTRY_ENCODED_LEN as usize];
    buf[0..4].copy_from_slice(&entry.byte_offset_in_term_payload.to_le_bytes());
    buf[4..6].copy_from_slice(&entry.count.to_le_bytes());
    buf[6..10].copy_from_slice(&entry.max_doc_id.to_le_bytes());
    buf
}

/// Plan segments S5b: decode a term's whole [`BlockDirEntry`] run back
/// from `count` packed 10-byte records ([`encode_block_dir_entry`]).
/// Returns `None` on a truncated buffer — the caller
/// ([`FieldPostings::block_directory_entries`]) then reports no
/// directory and [`TermDictionary::disk_cursor`] falls back to the
/// recompute path, matching every other best-effort I/O failure here.
fn decode_block_dir_entries(bytes: &[u8], count: usize) -> Option<Vec<BlockDirEntry>> {
    let mut entries = Vec::with_capacity(count);
    for idx in 0..count {
        let base = idx.checked_mul(BLOCK_DIR_ENTRY_ENCODED_LEN as usize)?;
        entries.push(BlockDirEntry {
            byte_offset_in_term_payload: u32::from_le_bytes(
                bytes.get(base..base + 4)?.try_into().ok()?,
            ),
            count: u16::from_le_bytes(bytes.get(base + 4..base + 6)?.try_into().ok()?),
            max_doc_id: u32::from_le_bytes(bytes.get(base + 6..base + 10)?.try_into().ok()?),
        });
    }
    Some(entries)
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

    /// Returns the term's `(doc_ids, freqs)` slice pair, index-aligned.
    fn lookup(&self, term: &str) -> Option<(&[u32], &[u32])> {
        let (_, start, end) = self.term_range(term)?;
        Some((
            self.doc_ids_flat.get(start..end)?,
            self.freqs_flat.get(start..end)?,
        ))
    }

    /// Lot C `C1a-batché` SHADOW: this term's `(offset, len)` descriptor
    /// into the shared segment, plus its posting count (`df`, needed by
    /// [`decode_postings_blocked`] to know where the last block ends).
    /// `segment` is only consulted when [`Self::term_entries_directory`]
    /// is `Some(_)` (plan segments S5b, spilled per-term directory) —
    /// pass `None` when no segment is available, which then makes this
    /// method return `None` for such a term, matching every other
    /// best-effort I/O failure in this file.
    fn segment_slice(
        &self,
        term: &str,
        segment: Option<&PostingsSegment>,
    ) -> Option<((u64, u32), usize)> {
        let idx = self.fst.get(term.as_bytes())? as usize;
        let entry = self.term_entry(idx, segment)?;
        Some((
            (entry.postings_offset, entry.postings_len),
            entry.postings_count as usize,
        ))
    }

    /// Plan segments S5b: resolve term idx `idx`'s whole [`TermEntry`],
    /// either assembled from the resident arrays (postings-disk flag was
    /// off at build time, or persisting the directory failed — see
    /// [`Self::term_entries_directory`]'s doc comment) or via ONE `pread`
    /// into the spilled per-term table (flag on). The single accessor
    /// every consumer goes through: [`Self::segment_slice`] (thus
    /// [`TermDictionary::decode_from_segment`]),
    /// [`TermDictionary::disk_cursor`], and [`merge_term_dictionaries`]
    /// (which needs a SOURCE dictionary's own metadata while merging, not
    /// just the merged dictionary's).
    fn term_entry(&self, idx: usize, segment: Option<&PostingsSegment>) -> Option<TermEntry> {
        match self.term_entries_directory {
            Some((entries_base, term_count)) => {
                if idx as u64 >= u64::from(term_count) {
                    return None;
                }
                let byte_offset = entries_base + (idx as u64) * TERM_ENTRY_ENCODED_LEN;
                let bytes = segment?.read(byte_offset, TERM_ENTRY_ENCODED_LEN as u32)?;
                decode_term_entry(&bytes)
            }
            None => {
                let &(postings_offset, postings_len) = self.segment_descriptors.get(idx)?;
                let start = *self.offsets.get(idx)? as usize;
                let end = *self.offsets.get(idx + 1)? as usize;
                // Resident block-dir CSR: empty when the flag was off at
                // build time (RAM mode never builds a directory) — the
                // entry then reports `block_dir_count == 0`, which
                // downstream means "no persisted directory, recompute".
                let (block_dir_offset, block_dir_count) = match (
                    self.block_dir_offsets.get(idx),
                    self.block_dir_offsets.get(idx + 1),
                ) {
                    (Some(&dir_start), Some(&dir_end)) => {
                        (u64::from(dir_start), dir_end.saturating_sub(dir_start))
                    }
                    _ => (0, 0),
                };
                Some(TermEntry {
                    postings_offset,
                    postings_len,
                    postings_count: end.saturating_sub(start) as u32,
                    block_dir_offset,
                    block_dir_count,
                })
            }
        }
    }

    /// Plan segments S5b: fetch `entry`'s whole persisted per-block
    /// directory run in ONE grouped read — a slice copy of the resident
    /// [`Self::block_directory`] (spill fell back or never happened), or
    /// a single `pread` of `block_dir_count` packed 10-byte records
    /// (spilled — never one `pread` per block). Returns `None` when the
    /// term carries no persisted directory (`block_dir_count == 0`,
    /// including every RAM-mode build) or on any I/O/decode failure —
    /// [`TermDictionary::disk_cursor`] then falls back to
    /// [`DiskPostingsCursor::open`]'s payload-recompute path.
    fn block_directory_entries(
        &self,
        entry: TermEntry,
        segment: Option<&PostingsSegment>,
    ) -> Option<Vec<BlockDirEntry>> {
        if entry.block_dir_count == 0 {
            return None;
        }
        match self.term_entries_directory {
            Some(_) => {
                let byte_len = entry
                    .block_dir_count
                    .checked_mul(BLOCK_DIR_ENTRY_ENCODED_LEN as u32)?;
                let bytes = segment?.read(entry.block_dir_offset, byte_len)?;
                decode_block_dir_entries(&bytes, entry.block_dir_count as usize)
            }
            None => {
                let start = usize::try_from(entry.block_dir_offset).ok()?;
                let end = start.checked_add(entry.block_dir_count as usize)?;
                self.block_directory
                    .get(start..end)
                    .map(<[BlockDirEntry]>::to_vec)
            }
        }
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
            doc_ids: self.doc_ids_flat.get(start..end)?,
            freqs: self.freqs_flat.get(start..end)?,
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
    /// Lot C `C1a-batché` SHADOW disk-backed FoR postings segment (see the
    /// module doc on `postings_segment` at the top of this file). `Arc`
    /// because `TermDictionary` derives `Clone` (transitively required by
    /// `DocumentIndex: Clone`) and any clone must keep sharing the SAME
    /// underlying tempfile — cloning the `Arc` is a refcount bump, not a
    /// second segment. The segment is written exactly once, at
    /// [`PostingsBuilder::build`] time, and is read-only for the rest of
    /// its life (by [`Self::decode_from_segment`] only — no production
    /// read path touches it), so sharing via `Arc` needs no interior
    /// mutability. Dropped (and its tempfile removed) once every clone of
    /// the `TermDictionary` that reached this generation is gone.
    ///
    /// Hardening: `None` when the segment could not be created at all
    /// (tempfile I/O failure) or was disabled mid-build after a failed
    /// `append()` (best-effort — see [`PostingsBuilder::build`]). Every
    /// term's `segment_descriptors` entry is then the sentinel `(0, 0)`
    /// and [`Self::decode_from_segment`] returns `None` for all of them;
    /// RAM (`doc_ids_flat`/`freqs_flat`) is unaffected either way.
    postings_segment: Option<Arc<PostingsSegment>>,
    /// Lot C `C1a-batché` hardening gauge: total number of terms across
    /// every field whose FoR encode failed (`encode_postings_blocked`
    /// returned `Err`, non-strictly-increasing doc_ids — see the module
    /// doc at the top of this file) and therefore carry NO disk coverage
    /// (sentinel `(0, 0)` descriptor). Feeds
    /// `surch_index_disk_postings_skipped_terms`; deliberately does NOT
    /// count terms that lost disk coverage purely because the segment
    /// itself was absent/disabled (I/O failure) — see
    /// [`Self::postings_segment_skipped_terms`].
    ///
    /// Correction fix: since `PostingsBuilder::build()` now merges every
    /// same-doc_id run before this encode step (`dedup_merge_postings`),
    /// non-strictly-increasing doc_ids from a multi-valued source field
    /// can no longer occur — this gauge is expected to read 0 in steady
    /// state and now only guards against an unrelated future regression.
    postings_skipped_terms: u64,
    /// Lot C `C1b` sous-pas 2: whether THIS `TermDictionary` was built
    /// with the disk-backed postings read path
    /// ([`PostingsBuilder::build_with_disk_flag`]). `false` by default
    /// (`derive(Default)`), matching the 100 % in-RAM engine. Baked in
    /// once at build time rather than re-read from the process-wide
    /// [`postings_disk_enabled`] flag on every query — see that
    /// function's doc comment for why: a dictionary's own RAM/disk
    /// layout must never disagree with what its read path assumes, and
    /// re-reading a `OnceLock`-cached global here would also make a
    /// same-process flag-ON/flag-OFF parity test impossible.
    disk_enabled: bool,
}

impl TermDictionary {
    /// Lot C `C1b` sous-pas 2: whether this dictionary's postings are
    /// disk-backed (`doc_ids_flat`/`freqs_flat` empty, the segment +
    /// persisted block directory are the sole source of truth) or
    /// resident in RAM. Read-path callers (`surch-api`) branch on this
    /// instead of the process-wide [`postings_disk_enabled`] flag.
    pub fn disk_backed(&self) -> bool {
        self.disk_enabled
    }
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
            .map(|(doc_ids, freqs)| PostingsEnum {
                doc_ids,
                freqs,
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

    /// Lot C `C1a-batché` SHADOW: decode `(field, term)`'s postings back
    /// from the disk segment (`pread` + [`decode_postings_blocked`])
    /// instead of reading `doc_ids_flat`/`freqs_flat` in RAM. Returns
    /// `None` when the field or the term is unknown, `None` on a
    /// malformed payload (never observed in practice — `build()` already
    /// `debug_assert!`s this round-trip for every term at build time).
    ///
    /// NOT on any production read path — `lookup*`/[`PostingsList`]/
    /// scoring/the leapfrog all stay on the RAM buffers. This method
    /// exists for the shadow round-trip test and for a future cold path
    /// (C1b) to build on.
    ///
    /// Returns `None` (in addition to the unknown-field/-term cases
    /// above) when the term's descriptor is the sentinel `(0, 0)` — no
    /// disk coverage, either because its encode failed at build time or
    /// because the segment itself was absent/disabled (best-effort — see
    /// [`PostingsBuilder::build`]) — or when the underlying `pread`
    /// itself fails.
    pub fn decode_from_segment(&self, field: &str, term: &str) -> Option<(Vec<u32>, Vec<u32>)> {
        let field_postings = self.fields.get(field)?;
        let segment = self.postings_segment.as_deref();
        let ((offset, len), count) = field_postings.segment_slice(term, segment)?;
        if len == 0 {
            return None;
        }
        let bytes = segment?.read(offset, len)?;
        decode_postings_blocked(&bytes, count).ok()
    }

    /// Lot C `C1b` sous-pas 1: build a [`DiskPostingsCursor`] over
    /// `(field, term)`'s payload inside the disk segment. Originally
    /// PURELY ADDITIVE (no production read path called this) — sous-pas
    /// 2 (`docs/paper/c1b-disk-backed-design-2026-07-02.md`) turns it
    /// into the production entry point for the conjunction/leapfrog
    /// read path (`surch-api::state`'s `conjunction_hits_internal`,
    /// `fused_conjunction_scores`, `conjunction_of_matches`) when the
    /// disk flag is on. `disk_cursor_matches_ram_leapfrog`
    /// (`crates/surch-index/tests/postings.rs`) still drives it directly
    /// too.
    ///
    /// Fast path (sous-pas 2, re-plumbed by plan segments S5b): when
    /// `(field, term)` has a PERSISTED per-block directory, the term's
    /// [`TermEntry`] is resolved first
    /// ([`FieldPostings::term_entry`] — resident, or ONE `pread`) and the
    /// directory run is fetched in ONE grouped read
    /// ([`FieldPostings::block_directory_entries`] — a resident slice
    /// copy, or one `pread` of the whole run; never one `pread` per
    /// block), then the cursor opens from it directly
    /// ([`DiskPostingsCursor::open_with_directory`]) — no `pread` of the
    /// term's payload just to compute the directory. Falls back to
    /// sous-pas 1's ad hoc [`DiskPostingsCursor::open`] (one
    /// whole-payload `pread` + directory recompute) when no persisted
    /// directory exists — e.g. a `TermDictionary` built with the flag
    /// off, or a term whose directory computation failed at build time
    /// (`block_dir_count == 0` with a readable payload — see
    /// [`TermEntry`]'s sentinel contract; such a term is NOT empty).
    ///
    /// Returns `None` for the same reasons [`Self::decode_from_segment`]
    /// does: unknown field/term, the sentinel `postings_len == 0` (no
    /// disk coverage), an absent segment, or a directory-computation
    /// failure inside [`DiskPostingsCursor::open`].
    pub fn disk_cursor(&self, field: &str, term: &str) -> Option<DiskPostingsCursor<'_>> {
        let field_postings = self.fields.get(field)?;
        let segment = self.postings_segment.as_deref();
        let idx = field_postings.fst.get(term.as_bytes())? as usize;
        let entry = field_postings.term_entry(idx, segment)?;
        if entry.postings_len == 0 {
            return None;
        }
        let segment = segment?;
        if let Some(directory) = field_postings.block_directory_entries(entry, Some(segment)) {
            return Some(DiskPostingsCursor::open_with_directory(
                segment,
                entry.postings_offset,
                entry.postings_len,
                directory,
                entry.postings_count as usize,
            ));
        }
        DiskPostingsCursor::open(
            segment,
            entry.postings_offset,
            entry.postings_len,
            entry.postings_count as usize,
        )
    }

    /// Lot C `C1a-batché` gauge: total bytes physically written to the
    /// disk segment so far (`surch_index_disk_postings_bytes`). Mirrors
    /// `surch_index_disk_segment_bytes` (the `source.dat`/`_source`
    /// segment in surch-api): a raw disk-footprint measurement, kept
    /// OUTSIDE [`crate::memory::MemoryUsage`]/`total_bytes()` on purpose
    /// — it measures bytes on disk (page-cache backed), not RAM the
    /// process holds, so summing it into the RAM total would double-count
    /// against `postings_bytes` (SHADOW: the same postings are, today,
    /// ALSO fully resident in `doc_ids_flat`/`freqs_flat`). `0` when the
    /// segment is absent (best-effort hardening — see
    /// [`PostingsBuilder::build`]).
    pub fn postings_segment_bytes(&self) -> u64 {
        self.postings_segment
            .as_ref()
            .map(|segment| segment.bytes_written())
            .unwrap_or(0)
    }

    /// Lot C `C1a-batché` hardening gauge: total number of terms with NO
    /// disk coverage because their FoR encode failed at build time
    /// (`surch_index_disk_postings_skipped_terms`) — see the
    /// `postings_skipped_terms` field doc above. Diagnostic pairing with
    /// [`Self::postings_segment_bytes`]: `skipped_terms > 0` points at a
    /// non-monotonic-doc_ids regression (should not happen post
    /// Correction fix, see the field doc above); `skipped_terms == 0`
    /// while `postings_segment_bytes() == 0` instead points at an I/O
    /// failure (segment never created, or a batched `append()` failed
    /// and disabled it for the rest of the build).
    pub fn postings_segment_skipped_terms(&self) -> u64 {
        self.postings_skipped_terms
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

    /// Plan segments S5 (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §"Dimensionnement S5"): total bytes held by the per-term CSR/directory
    /// metadata NO OTHER gauge counted before this — `offsets`,
    /// `block_offsets`, `segment_descriptors`, `block_directory`, and
    /// `block_dir_offsets`, RESIDENT bytes only. Summed over every field.
    /// Plan segments S5b spills ALL five arrays into the shared segment
    /// once `SURCH_POSTINGS_DISK` is on (see
    /// [`FieldPostings::term_entries_directory`]'s doc comment), so this
    /// gauge reads ~0 under the flag — it keeps measuring what actually
    /// sits on the heap (including any best-effort resident fallback),
    /// while the spilled bytes show up in
    /// [`Self::postings_segment_bytes`] (disk footprint, page-cache
    /// backed) instead.
    ///
    /// Scales with the DISTINCT TERM COUNT (`T`), not the doc count — an
    /// `edge_ngram`/`autocomplete` analyzer or an `index_prefixes` field
    /// can push `T` into the millions even on a modest doc count. This was
    /// the strongest identified candidate for the ~295 MiB gap between
    /// jemalloc `allocated` and the sum of every other
    /// `surch_index_*`/`surch_state_*` gauge measured on the 1,36 M
    /// matchID corpus (design doc's dimensioning table): at ~28-36 B/term
    /// (16 B `segment_descriptors` + 4 B×3 CSR/directory offset arrays)
    /// against `fst_bytes`'s few-B/term COMPRESSED footprint, a `T` in
    /// the low tens of millions accounts for the observed order of
    /// magnitude.
    pub fn postings_directory_bytes(&self) -> u64 {
        let u32_size = std::mem::size_of::<u32>() as u64;
        let descriptor_size = std::mem::size_of::<(u64, u32)>() as u64;
        let block_dir_entry_size = std::mem::size_of::<BlockDirEntry>() as u64;
        self.fields
            .values()
            .map(|fp| {
                (fp.offsets.len() as u64).saturating_mul(u32_size)
                    + (fp.block_offsets.len() as u64).saturating_mul(u32_size)
                    + (fp.segment_descriptors.len() as u64).saturating_mul(descriptor_size)
                    + (fp.block_directory.len() as u64).saturating_mul(block_dir_entry_size)
                    + (fp.block_dir_offsets.len() as u64).saturating_mul(u32_size)
            })
            .sum()
    }

    /// Lot C Phase 1 memory accounting: real on-heap bytes of the flat
    /// postings buffers — term strings (read back from the FST) +
    /// `doc_ids_flat` + `freqs_flat`, summed over fields. Replaces the
    /// old `surch-index::memory::accounting_from_postings` walker, which
    /// paid one FST point-lookup per term (`doc_index.postings(field,
    /// term)`) plus an O(df) count just to re-derive numbers that are now
    /// directly available as buffer lengths.
    ///
    /// Lot C Phase 1 levier 5: the historical AoS `postings_flat:
    /// Box<[Posting]>` (8 B/posting: a duplicated `doc_id` + `freq`) was
    /// replaced by the SoA `freqs_flat: Box<[u32]>` (4 B/posting) kept
    /// alongside the already-present `doc_ids_flat`. Total per-posting
    /// cost drops from 12 B (`postings_flat` 8 B + `doc_ids_flat` 4 B) to
    /// 8 B (`doc_ids_flat` 4 B + `freqs_flat` 4 B) — the duplicated
    /// `doc_id` is gone.
    pub fn postings_bytes(&self) -> u64 {
        let doc_id_size = std::mem::size_of::<u32>() as u64;
        let freq_size = std::mem::size_of::<u32>() as u64;
        self.fields
            .values()
            .map(|fp| {
                let mut term_bytes = 0u64;
                let mut stream = fp.fst.stream().into_stream();
                while let Some((bytes, _)) = stream.next() {
                    term_bytes += bytes.len() as u64;
                }
                term_bytes
                    + (fp.doc_ids_flat.len() as u64).saturating_mul(doc_id_size)
                    + (fp.freqs_flat.len() as u64).saturating_mul(freq_size)
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
    doc_ids: &'a [u32],
    freqs: &'a [u32],
    position: usize,
}

impl Iterator for PostingsEnum<'_> {
    type Item = Posting;

    fn next(&mut self) -> Option<Self::Item> {
        let doc_id = *self.doc_ids.get(self.position)?;
        let freq = self.freqs[self.position];
        self.position += 1;
        Some(Posting::new(doc_id, freq))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PostingsList<'a> {
    doc_ids: &'a [u32],
    freqs: &'a [u32],
    block_metas: &'a [BlockMeta],
    roaring: Option<&'a RoaringDocSet>,
}

impl<'a> PostingsList<'a> {
    /// The compact ascending `doc_id` channel. The conjunction leapfrog /
    /// `PostingsBlockSkipIter` walks ONLY this slice — never
    /// [`Self::freqs`] — so it touches half the bytes per posting a
    /// `[Posting]` slice would.
    pub fn doc_ids(&self) -> &'a [u32] {
        self.doc_ids
    }

    /// The `freq` channel, index-aligned with [`Self::doc_ids`] (posting
    /// `i` is `(doc_ids()[i], freqs()[i])`). Read only at scoring time —
    /// the leapfrog never touches this slice.
    pub fn freqs(&self) -> &'a [u32] {
        self.freqs
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

    /// Total posting count for this term. Lot C Phase 1 lever B dropped
    /// `BlockMeta::posting_count` (derivable at O(1): the CSR-flattened
    /// `doc_ids` slice is already sized exactly to the term's `df`, so
    /// summing per-block counts and reading the buffer length are the
    /// same number), so this reads `doc_ids.len()` directly instead of
    /// summing over `block_metas`.
    pub fn doc_freq_from_block_metas(&self) -> usize {
        self.doc_ids.len()
    }

    /// Build a [`BlockSkipList`] (Lot 2: skip lists on the codec FoR
    /// path) over this term's postings. Returns `None` when the posting
    /// list is empty (so callers can short-circuit without constructing
    /// an empty skip list). Returns `Err(_)` only on a programmer-bug-level
    /// invariant violation (postings not strictly increasing across
    /// blocks); these errors should never fire in practice because
    /// `PostingsBuilder::build()` sorts postings by ascending `doc_id`.
    ///
    /// Lot C Phase 1 lever B: `min_doc_id`/`max_doc_id` are no longer
    /// stored in `BlockMeta` — derived here at O(1) from `doc_ids`
    /// directly (postings sorted ascending, so a block's range is simply
    /// its first/last entry in `doc_ids.chunks(BLOCK_SIZE)`, the exact
    /// chunking `build_block_metas` computed these fields from before).
    /// Bit-identical values, read from the buffer instead of the struct.
    pub fn block_skip_list(
        &self,
    ) -> std::result::Result<Option<BlockSkipList>, PostingsBlockError> {
        if self.doc_ids.is_empty() {
            return Ok(None);
        }
        let entries = self
            .doc_ids
            .chunks(BLOCK_SIZE)
            .enumerate()
            .map(|(idx, chunk)| BlockSkipEntry {
                block_index: idx,
                min_doc_id: chunk[0],
                max_doc_id: chunk[chunk.len() - 1],
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
    /// `doc_id` channel is index-aligned with the term's `freqs` channel, at
    /// the same index in `PostingsList::freqs()`. Lets the conjunction
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

/// Lot C `C1b` sous-pas 1 — pread-addressed leapfrog cursor prototype
/// (`docs/paper/c1b-disk-backed-design-2026-07-02.md`). PURELY ADDITIVE:
/// no production read path constructs or consumes this type as of this
/// commit — `lookup*`/[`PostingsList`]/[`PostingsBlockSkipIter`]/scoring/
/// the conjunction leapfrog all keep reading the RAM `doc_ids_flat`/
/// `freqs_flat` buffers exactly as before. `DiskPostingsCursor` exists to
/// validate, ahead of the read-path bascule (a later C1b sous-pas), that
/// a block-addressed `pread` + [`decode_block`] walk driven by a
/// directory of `(byte_offset, count, max_doc_id)` entries
/// ([`BlockDirEntry`], via [`block_directory`]) reproduces
/// [`PostingsBlockSkipIter::advance_to`]'s RAM leapfrog semantics
/// bit-for-bit — see `disk_cursor_matches_ram_leapfrog` in
/// `crates/surch-index/tests/postings.rs`.
///
/// Every block is `pread`'d and [`decode_block`]-decoded AT MOST ONCE:
/// the cursor caches the current block's decoded `(doc_ids, freqs)` and
/// only replaces that cache once `advance_to` walks past the block.
/// Blocks whose `max_doc_id < target` are skipped purely through the
/// directory (a `u32` comparison), with NO `pread` and NO decode at all
/// — exactly the property C1b's disk-backed read path needs on the
/// common-term conjunction tail.
#[derive(Debug)]
pub struct DiskPostingsCursor<'seg> {
    segment: &'seg PostingsSegment,
    /// Segment-absolute byte offset of this term's payload start (the
    /// `offset` half of the term's `FieldPostings::segment_descriptors`
    /// entry).
    term_base_offset: u64,
    /// Total byte length of this term's payload in the segment (the
    /// `len` half of the same descriptor) — the exclusive upper bound
    /// used to size the pread of the LAST block.
    term_payload_len: u32,
    /// Per-block directory, computed once at [`Self::open`] time (sous-pas
    /// 1: ad hoc, not persisted — see [`block_directory`]'s doc comment).
    directory: Vec<BlockDirEntry>,
    /// Index into `directory` of the block currently under consideration.
    /// Monotonic — only ever increases, mirroring [`BlockSkipCursor`]'s
    /// `position`.
    block_cursor: usize,
    /// The decoded `(block_index, doc_ids, freqs)` currently cached, if
    /// any. `block_index` matches `block_cursor` while that block is
    /// still being consumed; moving past it replaces the cache with the
    /// next block's decode — never re-decodes a block already advanced
    /// past.
    loaded: Option<(usize, Vec<u32>, Vec<u32>)>,
    /// Index into `loaded`'s `doc_ids`/`freqs` to resume scanning from on
    /// the NEXT `advance_to` call — one past the last returned entry.
    local_pos: usize,
    /// `freq` of the doc_id most recently returned by [`Self::advance_to`].
    last_freq: u32,
    /// Plan segments S5b: the term's `df`, carried by the caller (the
    /// [`TermEntry::postings_count`] the cursor was opened from) so
    /// [`Self::doc_freq`] is O(1) instead of an O(blocks) directory sum —
    /// `fused_conjunction_scores_disk` (surch-api) reads it per term per
    /// query BEFORE any `advance_to`, for idf and driver ordering.
    total_count: usize,
}

impl<'seg> DiskPostingsCursor<'seg> {
    /// Build a cursor over one term's payload inside `segment`. Computes
    /// the block directory (sous-pas 1: ad hoc, NOT persisted — see
    /// [`block_directory`]) via a single `pread` of the whole term
    /// payload; every SUBSEQUENT block is `pread`'d and decoded
    /// individually, on demand, by [`Self::advance_to`].
    ///
    /// `term_base_offset`/`term_payload_len` are the term's
    /// `FieldPostings::segment_descriptors` entry; `total_count` is the
    /// term's `df` (`offsets[i + 1] - offsets[i]`) — exactly the
    /// arguments [`TermDictionary::disk_cursor`] passes through from
    /// [`FieldPostings::segment_slice`].
    ///
    /// Returns `None` if the initial `pread` or the directory computation
    /// fails (a malformed payload — should never happen for a term the
    /// build-time segment round-trip already validated).
    ///
    /// `pub(crate)`, not `pub`: [`PostingsSegment`] itself is a
    /// crate-private type (the `postings_segment` submodule is not
    /// `pub`), so a fully `pub fn` naming it as a parameter would leak a
    /// more-private type through a public signature (the
    /// `private_interfaces` lint). External callers reach a cursor
    /// exclusively through [`TermDictionary::disk_cursor`], which never
    /// names `PostingsSegment` in ITS signature.
    pub(crate) fn open(
        segment: &'seg PostingsSegment,
        term_base_offset: u64,
        term_payload_len: u32,
        total_count: usize,
    ) -> Option<Self> {
        let payload = segment.read(term_base_offset, term_payload_len)?;
        let directory = block_directory(&payload, total_count).ok()?;
        Some(Self {
            segment,
            term_base_offset,
            term_payload_len,
            directory,
            block_cursor: 0,
            loaded: None,
            local_pos: 0,
            last_freq: 0,
            total_count,
        })
    }

    /// Lot C `C1b` sous-pas 2: build a cursor from an ALREADY-COMPUTED
    /// directory (the term's persisted [`BlockDirEntry`] run, resident or
    /// `pread` in one grouped shot — see
    /// [`FieldPostings::block_directory_entries`], plan segments S5b)
    /// instead of `pread`-ing the whole term payload to compute it fresh
    /// — the production fast path [`TermDictionary::disk_cursor`] takes
    /// when a persisted directory is available. `total_count` is the
    /// term's `df` ([`TermEntry::postings_count`]) — carried so
    /// [`Self::doc_freq`] is O(1). Infallible: the directory was already
    /// validated once, at build time (the per-term `debug_assert!` round
    /// trip in `PostingsBuilder::build_with_disk_flag`'s disk-mode
    /// branch).
    pub(crate) fn open_with_directory(
        segment: &'seg PostingsSegment,
        term_base_offset: u64,
        term_payload_len: u32,
        directory: Vec<BlockDirEntry>,
        total_count: usize,
    ) -> Self {
        Self {
            segment,
            term_base_offset,
            term_payload_len,
            directory,
            block_cursor: 0,
            loaded: None,
            local_pos: 0,
            last_freq: 0,
            total_count,
        }
    }

    /// Advance to the first `doc_id >= target`, `pread`+decoding at most
    /// one NEW block to get there. `target` must be non-decreasing across
    /// calls — same contract, same return-and-consume semantics, as
    /// [`PostingsBlockSkipIter::advance_to`]. Returns `None` once the
    /// term's postings are exhausted.
    pub fn advance_to(&mut self, target: u32) -> Option<u32> {
        loop {
            let entry = *self.directory.get(self.block_cursor)?;
            if entry.max_doc_id < target {
                // Skip this whole block: directory-only, no pread, no
                // decode.
                self.block_cursor += 1;
                continue;
            }

            if self.loaded.as_ref().map(|(idx, ..)| *idx) != Some(self.block_cursor) {
                let block_end = self
                    .directory
                    .get(self.block_cursor + 1)
                    .map(|next| next.byte_offset_in_term_payload)
                    .unwrap_or(self.term_payload_len);
                let block_len = block_end.checked_sub(entry.byte_offset_in_term_payload)?;
                let block_bytes = self.segment.read(
                    self.term_base_offset + u64::from(entry.byte_offset_in_term_payload),
                    block_len,
                )?;
                let (doc_ids, freqs) = decode_block(&block_bytes, entry.count as usize).ok()?;
                self.loaded = Some((self.block_cursor, doc_ids, freqs));
                self.local_pos = 0;
            }

            // `.unwrap()` is safe: the branch above just ensured `loaded`
            // holds `block_cursor`'s decode.
            let (_, doc_ids, freqs) = self.loaded.as_ref().unwrap();
            if self.local_pos >= doc_ids.len() {
                // This block is fully consumed even though its
                // `max_doc_id >= target` (target caught up with an
                // already-returned entry) — move on to the next block.
                self.block_cursor += 1;
                continue;
            }

            // Bounded exponential gallop + `partition_point`, same shape
            // as `PostingsBlockSkipIter::advance_to`'s scan — bounded
            // here to the decoded block (<= `FOR_BLOCK_SIZE` entries)
            // rather than the whole term, per the design's "galope dans
            // les <=128 entrées décodées" contract. Same result either
            // way: the landed block's `max_doc_id >= target` guarantees a
            // match exists at or before the block's last entry, so this
            // gallop always finds one — no "exhausted" branch needed.
            let rest = &doc_ids[self.local_pos..];
            let mut hi = 1usize;
            while hi < rest.len() && rest[hi] < target {
                hi <<= 1;
            }
            let lo = hi >> 1;
            let hi = hi.min(rest.len());
            let offset = lo + rest[lo..hi].partition_point(|&doc_id| doc_id < target);
            let found_index = self.local_pos + offset;
            self.local_pos = found_index + 1;
            self.last_freq = freqs[found_index];
            return Some(doc_ids[found_index]);
        }
    }

    /// `freq` of the doc_id most recently returned by [`Self::advance_to`].
    /// Meaningless before the first successful `advance_to` call (reads
    /// `0`) — same implicit precondition as the RAM side's
    /// `freqs()[position() - 1]`.
    pub fn freq(&self) -> u32 {
        self.last_freq
    }

    /// Lot C `C1b` sous-pas 2: total posting count (`df`) for this term,
    /// available without advancing the cursor at all. Used by the
    /// disk-mode conjunction/scoring read path (`surch-api::state`) to
    /// pick the rarest driver term and to compute `idf`, mirroring what
    /// `PostingsList::doc_freq_from_block_metas` gives the RAM path.
    /// Plan segments S5b: O(1) — reads the count both constructors carry
    /// ([`TermEntry::postings_count`], or [`Self::open`]'s `total_count`
    /// argument) instead of summing the directory's per-block `count`s
    /// (whose sum it equals by construction: both derive from the same
    /// build-time postings).
    pub fn doc_freq(&self) -> usize {
        self.total_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plan segments S5b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §"Dimensionnement S5"): white-box counterpart to
    /// `crates/surch-index/tests/postings.rs`'s read-parity tests — those
    /// can only observe PUBLIC behaviour; this one has access to the
    /// private `FieldPostings` fields and directly asserts the RAM-shrink
    /// claim the S5 disk-back is FOR: ALL FIVE per-term arrays
    /// (`segment_descriptors`, `offsets`, `block_offsets`,
    /// `block_directory`, `block_dir_offsets`) drop to empty
    /// (`Box::default()`, zero heap — the `postings_directory_bytes`
    /// gauge reads exactly 0) and `term_entries_directory` becomes
    /// `Some(_)` once the postings-disk flag is on, for a field that
    /// carries terms, INCLUDING a multi-block one (so the spilled block
    /// directory is non-trivial). Flag off stays BYTE-IDENTICAL to the
    /// historical shape: `T`-sized descriptors, `T + 1` CSR tables,
    /// `term_entries_directory == None`.
    #[test]
    fn per_term_directory_moves_off_heap_when_disk_flag_is_on() {
        fn populate(builder: &mut PostingsBuilder) {
            for i in 0..50u32 {
                builder
                    .add("body", format!("term{i}"), i, vec![0])
                    .expect("distinct term");
            }
            // One multi-block term (3 full blocks + a partial one) so the
            // block-directory channel genuinely carries several entries
            // per its term.
            for doc_id in 0..(BLOCK_SIZE as u32 * 3 + 17) {
                builder
                    .add("body", "wide", doc_id, vec![0])
                    .expect("wide posting");
            }
        }
        const TERM_COUNT: usize = 51;

        let mut builder_off = PostingsBuilder::new();
        populate(&mut builder_off);
        let dict_off = builder_off.build_with_disk_flag(false);
        let field_off = dict_off.fields.get("body").expect("body field (flag off)");
        assert_eq!(
            field_off.segment_descriptors.len(),
            TERM_COUNT,
            "flag off must keep every term's descriptor resident (unchanged default)"
        );
        assert_eq!(field_off.offsets.len(), TERM_COUNT + 1);
        assert_eq!(field_off.block_offsets.len(), TERM_COUNT + 1);
        assert!(
            field_off.block_directory.is_empty() && field_off.block_dir_offsets.is_empty(),
            "flag off never builds a persisted block directory (pre-existing C1b contract)"
        );
        assert!(
            field_off.term_entries_directory.is_none(),
            "flag off must never spill a per-term directory"
        );
        assert!(dict_off.postings_directory_bytes() > 0);

        let mut builder_on = PostingsBuilder::new();
        populate(&mut builder_on);
        let dict_on = builder_on.build_with_disk_flag(true);
        let field_on = dict_on.fields.get("body").expect("body field (flag on)");
        assert!(
            field_on.segment_descriptors.is_empty()
                && field_on.offsets.is_empty()
                && field_on.block_offsets.is_empty()
                && field_on.block_directory.is_empty()
                && field_on.block_dir_offsets.is_empty(),
            "flag on should shed every resident per-term array entirely"
        );
        assert!(
            field_on.term_entries_directory.is_some(),
            "flag on should spill the unified per-term table instead"
        );
        assert_eq!(
            dict_on.postings_directory_bytes(),
            0,
            "the resident-directory gauge must read exactly 0 once spilled"
        );

        // Read parity on the spilled side, through the unified accessor:
        // same postings for a mono-block and the multi-block term.
        for term in ["term7", "wide"] {
            assert_eq!(
                dict_on.decode_from_segment("body", term),
                dict_off.decode_from_segment("body", term),
                "decode_from_segment diverged for {term}"
            );
        }
        // The cursor opens from the SPILLED directory (one grouped pread)
        // and reports the O(1) doc_freq carried by the TermEntry.
        let cursor = dict_on
            .disk_cursor("body", "wide")
            .expect("wide disk cursor (flag on)");
        assert_eq!(cursor.doc_freq(), BLOCK_SIZE * 3 + 17);
    }

    /// C1 fix (design doc `docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §C1, Q6): `merge_term_dictionaries`'s FST builder now streams to a
    /// tempfile (`MergeFstBuilder::Streamed`) instead of growing an
    /// in-memory `Vec<u8>` — this must not change the merged
    /// dictionary's CONTENT at all. Exercises enough distinct terms
    /// (1,501 : 1 shared across every source plus 500 unique per
    /// segment, over 3 segments) to be a meaningful stand-in for a "big
    /// tier" merge, checks every term's postings survive byte-for-byte,
    /// AND that the FST is COMPLETE (no truncation from the tempfile
    /// round trip) — in BOTH RAM and disk-backed mode.
    #[test]
    fn merge_streams_fst_without_altering_merged_term_dictionary_contents() {
        const TERMS_PER_SEGMENT: u32 = 500;
        const SEGMENTS: u32 = 3;

        fn build_segment(segment_idx: u32, disk_enabled: bool) -> TermDictionary {
            let mut builder = PostingsBuilder::new();
            let doc_base = segment_idx * TERMS_PER_SEGMENT;
            for i in 0..TERMS_PER_SEGMENT {
                let doc_id = doc_base + i;
                // A term shared by EVERY segment (exercises the FST
                // union's multi-source branch) plus a segment-unique
                // term (exercises the "only this source has it" branch).
                builder
                    .add("body", "shared", doc_id, vec![0])
                    .expect("shared term");
                builder
                    .add("body", format!("seg{segment_idx}-term{i}"), doc_id, vec![0])
                    .expect("segment-unique term");
            }
            builder.build_with_disk_flag(disk_enabled)
        }

        for disk_enabled in [false, true] {
            let segments: Vec<TermDictionary> = (0..SEGMENTS)
                .map(|s| build_segment(s, disk_enabled))
                .collect();
            let refs: Vec<&TermDictionary> = segments.iter().collect();
            let merged = merge_term_dictionaries(&refs, disk_enabled);

            // Every segment-unique term resolves to EXACTLY its own
            // doc_id — proves the merge-joined FST correctly threads
            // each source's own term index back to the right postings.
            for segment_idx in 0..SEGMENTS {
                let doc_base = segment_idx * TERMS_PER_SEGMENT;
                for i in 0..TERMS_PER_SEGMENT {
                    let doc_id = doc_base + i;
                    let term = format!("seg{segment_idx}-term{i}");
                    let (ids, _freqs) = merged
                        .decode_from_segment("body", &term)
                        .unwrap_or_else(|| panic!("term {term} must survive the merge"));
                    assert_eq!(ids, vec![doc_id], "term {term} doc_id mismatch after merge");
                }
            }
            // The shared term must carry every doc_id across all 3
            // segments, ascending, un-renumbered (Fable's arbitrage:
            // adjacent runs concatenate verbatim, never re-sorted).
            let (shared_ids, shared_freqs) = merged
                .decode_from_segment("body", "shared")
                .expect("shared term must survive the merge");
            let expected_shared: Vec<u32> = (0..SEGMENTS * TERMS_PER_SEGMENT).collect();
            assert_eq!(
                shared_ids, expected_shared,
                "shared term must carry every doc_id across all 3 segments, in \
                 order (disk_enabled={disk_enabled})"
            );
            assert_eq!(shared_freqs.len(), expected_shared.len());

            // Term-count sanity: 1 shared + `TERMS_PER_SEGMENT` unique
            // per segment — proves the streamed FST is COMPLETE, not
            // silently truncated by the tempfile round trip.
            let all_terms: Vec<String> = merged.terms("body").collect();
            assert_eq!(
                all_terms.len(),
                1 + (SEGMENTS * TERMS_PER_SEGMENT) as usize,
                "merged FST term count diverged from the expected union (disk_enabled={disk_enabled})"
            );
        }
    }

    /// Plan segments S5b: [`encode_term_entry`]/[`decode_term_entry`]
    /// round-trip, including the boundary values a real segment can
    /// produce (offset `0`, a large `u64` offset past 4 GiB,
    /// `postings_len == 0` — the sentinel "no disk coverage" record —
    /// and `block_dir_count == 0` — the "recompute the directory"
    /// marker).
    #[test]
    fn term_entry_encoding_round_trips() {
        let entries = [
            TermEntry {
                postings_offset: 0,
                postings_len: 0,
                postings_count: 0,
                block_dir_offset: 0,
                block_dir_count: 0,
            },
            TermEntry {
                postings_offset: 12,
                postings_len: 512,
                postings_count: 129,
                block_dir_offset: 4096,
                block_dir_count: 2,
            },
            TermEntry {
                postings_offset: u32::MAX as u64 + 1,
                postings_len: u32::MAX,
                postings_count: u32::MAX,
                block_dir_offset: u64::MAX,
                block_dir_count: u32::MAX,
            },
        ];
        for entry in entries {
            let encoded = encode_term_entry(&entry);
            assert_eq!(encoded.len(), TERM_ENTRY_ENCODED_LEN as usize);
            let decoded = decode_term_entry(&encoded).expect("full-length buffer decodes");
            assert_eq!(decoded, entry);
        }
        assert_eq!(
            decode_term_entry(&[0u8; 12]),
            None,
            "truncated buffer must not decode"
        );
    }

    /// Plan segments S5b: [`encode_block_dir_entry`]/
    /// [`decode_block_dir_entries`] round-trip over a multi-entry run
    /// (the grouped per-term pread shape), plus the truncated-buffer
    /// guard.
    #[test]
    fn block_dir_entry_encoding_round_trips() {
        let run = vec![
            BlockDirEntry {
                byte_offset_in_term_payload: 0,
                count: 128,
                max_doc_id: 1_000,
            },
            BlockDirEntry {
                byte_offset_in_term_payload: 300,
                count: 128,
                max_doc_id: 250_000,
            },
            BlockDirEntry {
                byte_offset_in_term_payload: 641,
                count: 17,
                max_doc_id: u32::MAX,
            },
        ];
        let mut bytes = Vec::new();
        for entry in &run {
            bytes.extend_from_slice(&encode_block_dir_entry(entry));
        }
        assert_eq!(
            bytes.len(),
            run.len() * BLOCK_DIR_ENTRY_ENCODED_LEN as usize
        );
        let decoded = decode_block_dir_entries(&bytes, run.len()).expect("full run decodes");
        assert_eq!(decoded, run);
        assert_eq!(
            decode_block_dir_entries(&bytes[..bytes.len() - 1], run.len()),
            None,
            "truncated run must not decode"
        );
    }
}
