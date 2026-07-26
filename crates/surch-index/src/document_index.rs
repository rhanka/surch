use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    merge_term_dictionaries, postings_disk_enabled, BlockMeta, CheckedPostings,
    DiskPostingsAdvance, DiskPostingsCursor, PostingsBuilder, PostingsEnum, PostingsError,
    PostingsList, PostingsReadError, TermDictionary, TermsEnum,
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

/// Lot C Phase 1 lever A: dense bit-vector backing `Segment::live_docs`.
///
/// `doc_id`s are dense (`0..next_doc_id`), so `bits[doc_id / 64]` bit
/// `doc_id % 64` records presence — one bit per doc_id instead of the
/// ~32 B/entry a `BTreeSet<u32>` node costs. `bits` grows by
/// resize-on-write to the highest doc_id seen (never shrinks on its own,
/// same convention as `FieldLengthStats::doc_len_dense`), and `count`
/// tracks the number of set bits so `live_doc_count()` stays O(1) instead
/// of a full bitmap scan.
#[derive(Debug, Default, Clone)]
struct LiveDocsBitset {
    bits: Vec<u64>,
    count: usize,
}

impl LiveDocsBitset {
    /// Marks `doc_id` live. Idempotent: inserting an already-live doc_id
    /// does not double-count it (mirrors `BTreeSet::insert`).
    fn insert(&mut self, doc_id: u32) {
        let idx = doc_id as usize;
        let word = idx / 64;
        if word >= self.bits.len() {
            self.bits.resize(word + 1, 0);
        }
        let mask = 1u64 << (idx % 64);
        if self.bits[word] & mask == 0 {
            self.bits[word] |= mask;
            self.count += 1;
        }
    }

    /// Whether `doc_id` is currently live. `doc_id`s past the highest one
    /// ever inserted are simply absent (never resized), so this is a safe
    /// bounds-checked lookup rather than a panic.
    fn contains(&self, doc_id: u32) -> bool {
        let idx = doc_id as usize;
        let word = idx / 64;
        self.bits
            .get(word)
            .is_some_and(|w| w & (1u64 << (idx % 64)) != 0)
    }

    /// Number of live doc_ids (population count), O(1).
    fn count(&self) -> usize {
        self.count
    }

    fn clear(&mut self) {
        self.bits.clear();
        self.count = 0;
    }

    /// Ascending doc_id iteration — same contract as the previous
    /// `BTreeSet<u32>::iter()`. Uses the same trailing-zeros bit-clearing
    /// trick as [`crate::roaring`]'s container intersection.
    fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.bits.iter().enumerate().flat_map(|(word_idx, &word)| {
            let base = (word_idx as u32) * 64;
            let mut remaining = word;
            std::iter::from_fn(move || {
                if remaining == 0 {
                    None
                } else {
                    let bit = remaining.trailing_zeros();
                    remaining &= remaining - 1;
                    Some(base + bit)
                }
            })
        })
    }

    /// Approximate heap bytes held by the bitmap: `bits` capacity only
    /// (`count` is stack-resident). Used by
    /// [`DocumentIndex::live_docs_bytes`].
    fn memory_bytes(&self) -> u64 {
        (self.bits.capacity() * std::mem::size_of::<u64>()) as u64
    }
}

/// Plan segments **S5c** (`docs/paper/design-segments-pic-borne-2026-07-05.md`
/// §S5c, following the S5a/S5b template — see `crate::postings`'s
/// `postings_segment` module doc for the shared rationale): append-only,
/// `pread`-addressed tempfile backing every disk-spilled [`SubfieldColumn`]
/// of ONE sealed [`Segment`]. A single file is shared by every sub-field
/// path that segment declares (a handful of `.raw`/`.norm` columns per
/// index in practice), never one file per column.
///
/// SAFE positional I/O only (`FileExt::write_all_at`/`read_exact_at` — no
/// `mmap`, no `posix_fallocate`), keeping `#![forbid(unsafe_code)]` intact.
/// Writes are BATCHED PER COLUMN ([`SubfieldColumn::spill`], ONE `append()`
/// per path — a handful of calls per segment seal, never per document):
/// the historical `C0` attempt (commit `af94e52`, reverted the same day)
/// instead called `append` once per VALUE from `merge_analyzed` — 5.46 M+
/// syscalls on the deces 1.36 M corpus, -43 % indexation throughput. That
/// regression is structurally impossible here: nothing calls `append` from
/// the write path (`merge_analyzed`/`SubfieldColumn::set`) — only
/// [`Segment::seal_subfield_columns`], once a column is fully built and
/// about to become read-only.
#[cfg(unix)]
mod subfield_segment {
    use std::{
        fs::{File, OpenOptions},
        os::unix::fs::FileExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    /// Logical pre-extension chunk for the segment file, in bytes — same
    /// safe `set_len` (ftruncate) pre-extension idiom as
    /// `postings::postings_segment`, NOT `posix_fallocate` (`unsafe`).
    const EXTEND_CHUNK: u64 = 64 * 1024 * 1024;

    /// Unique tempfile name, same scheme as `postings::postings_segment`
    /// (pid + a process-global sequence + wall-clock nanos), distinct
    /// filename prefix so the two segment kinds never collide on disk.
    fn next_temp_path() -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("surch-subfields-{pid}-{seq}-{nanos}.dat"))
    }

    /// Append-only, `pread`-addressed sub-field segment — see the module
    /// doc above for the on-disk layout and the batching contract.
    #[derive(Debug)]
    pub(crate) struct SubfieldSegment {
        file: File,
        next_offset: u64,
        extended_len: u64,
        path: PathBuf,
    }

    impl SubfieldSegment {
        /// Best-effort constructor: NEVER panics — a SHADOW/best-effort
        /// feature must not be able to crash indexation. Returns `None` on
        /// any I/O failure (unwritable `TMPDIR`, `ENOSPC`, permissions…):
        /// the caller then keeps every column fully resident, exactly as
        /// if this feature did not exist.
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
        /// `EXTEND_CHUNK` steps via the safe `File::set_len`. Failure is
        /// non-fatal: the next write falls back to ordinary per-write
        /// extension (slower, still correct).
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
        /// `None` if the write failed. Callers batch a whole column's
        /// `dict`+`codes` payload into one `Vec<u8>` before calling this
        /// (see [`SubfieldColumn::spill`]) — never one write per value.
        pub(crate) fn append(&mut self, bytes: &[u8]) -> Option<u64> {
            let offset = self.next_offset;
            self.ensure_capacity(offset.saturating_add(bytes.len() as u64));
            self.file.write_all_at(bytes, offset).ok()?;
            self.next_offset = offset.saturating_add(bytes.len() as u64);
            Some(offset)
        }

        /// Read `length` bytes at `offset` via `pread`. Returns `None` on
        /// any I/O failure instead of panicking (best-effort read, see
        /// [`SubfieldColumn::materialized`]).
        pub(crate) fn read(&self, offset: u64, length: u32) -> Option<Vec<u8>> {
            let mut buf = vec![0u8; length as usize];
            self.file.read_exact_at(&mut buf, offset).ok()?;
            Some(buf)
        }

        /// Total bytes appended so far — feeds the disk-footprint gauge
        /// (`surch_index_disk_subfield_values_bytes`, mirrors
        /// `postings::TermDictionary::postings_segment_bytes`).
        pub(crate) fn bytes_written(&self) -> u64 {
            self.next_offset
        }
    }

    impl Drop for SubfieldSegment {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Non-Unix fallback (Windows / unusual dev hosts): the same API backed by
/// an in-RAM buffer — mirrors `postings::postings_segment`'s fallback so
/// the crate keeps building on non-Linux dev hosts.
#[cfg(not(unix))]
mod subfield_segment {
    #[derive(Debug, Default)]
    pub(crate) struct SubfieldSegment {
        buf: Vec<u8>,
    }

    impl SubfieldSegment {
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

use subfield_segment::SubfieldSegment;

#[derive(Debug, Clone)]
pub struct DocumentIndex {
    /// Sealed-or-active, per-generation index state — see [`Segment`] for
    /// what it bundles and why.
    ///
    /// Plan segments (`docs/paper/design-segments-pic-borne-2026-07-05.md`):
    /// **S1** kept `Vec<Arc<Segment>>` at length EXACTLY 1 (the pure
    /// structural refactor step, bit-identical to the pre-segment
    /// layout). **S2** (budget-triggered flush, [`Self::maybe_flush_by_budget`],
    /// and `_refresh`, [`Self::materialize_terms_and_finalize_postings`])
    /// can genuinely append more — `segments.len()` stays `1` forever
    /// only while `SURCH_FLUSH_BUDGET_BYTES` is unset (the S1
    /// reversibility flag).
    ///
    /// Every eager write (`merge_analyzed`) mutates the ACTIVE segment
    /// (`segments.last()`) in place via `Arc::make_mut` (never clones:
    /// nothing else ever holds a second strong reference to a live
    /// segment), so this costs nothing extra over a direct-field layout.
    /// Every read goes through [`Self::segment`] (`segments[0]`
    /// passthrough — valid only when the caller has established
    /// `segment_count() == 1`, see that method's doc) or, where the
    /// aggregation is meaningful for every segment count (BM25
    /// doc_count/avg_doc_len, byte accounting, `live_doc_count`,
    /// `postings_disk_backed`/`decode_from_segment`'s owned merge), a
    /// real `Σ`/merge over `segments.iter()`.
    segments: Vec<Arc<Segment>>,
    postings_builder: PostingsBuilder,
    /// A6 phase 2: per-field write-time prefix expansion. Populated only for
    /// fields whose `FieldMapping::index_prefixes` is `Some(_)`. The inner map
    /// is keyed by the normalized prefix (length in `[min_chars..=max_chars]`)
    /// and the value is the set of doc ids that contain at least one token
    /// starting with that prefix. Kept separate from the regular postings so
    /// the BM25 hot path (`doc_freq`, `term_freq`, norms) is unaffected.
    ///
    /// Deliberately NOT part of [`Segment`] for S1: the design's sealed
    /// bundle scopes to term dictionary / length stats / sub-fields /
    /// live-docs — prefix postings stay a plain `DocumentIndex` field,
    /// same as before this refactor (a documented, deliberately narrow
    /// scope decision — see the S1 write-up).
    prefix_postings: BTreeMap<String, BTreeMap<String, BTreeSet<u32>>>,
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
    /// Lot C `C1b` sous-pas 2: per-index override for the disk-backed
    /// postings read path, bypassing the process-wide
    /// [`crate::postings::postings_disk_enabled`] flag. `None`
    /// (`derive(Default)`) means "use the process-wide flag" — the
    /// production default, unchanged from before this override existed.
    /// `Some(_)` is set via [`Self::set_postings_disk_enabled`], primarily
    /// by tests that need a flag-ON and a flag-OFF index side by side in
    /// the SAME process (the global `OnceLock` cannot flip mid-run — see
    /// `postings_disk_enabled`'s doc comment). Consulted by
    /// [`Self::resolved_postings_disk_enabled`] at every
    /// `PostingsBuilder::build_with_disk_flag` call site.
    postings_disk_enabled_override: Option<bool>,
    /// Plan segments S2: one past the highest GLOBAL doc_id ever merged
    /// into the currently-active segment (i.e. the doc_id the NEXT write
    /// will use, assuming the caller's monotonic-non-reused contract —
    /// see `surch-api::InMemoryIndex::next_doc_id`). Updated at the end
    /// of every [`Self::merge_analyzed`] call. Used to (a) know the
    /// active segment's `doc_base` to hand to a freshly-pushed successor
    /// at seal time, and (b) detect whether the active segment is empty
    /// (`next_doc_id_hint == active.doc_base`) so sealing it twice in a
    /// row (e.g. two `_refresh` calls with no write in between) does not
    /// push a useless empty segment. Reset to `0` by [`Self::clear`].
    next_doc_id_hint: u32,
    /// Plan segments S2: per-index override for the flush-by-budget
    /// threshold, bypassing the process-wide `SURCH_FLUSH_BUDGET_BYTES`
    /// env var — same rationale as `postings_disk_enabled_override` (a
    /// single test binary cannot flip an `OnceLock`-cached env read
    /// mid-run). See [`FlushBudgetOverride`].
    flush_budget_override: FlushBudgetOverride,
    /// Plan segments S3: per-index override for the merge fan-in, same
    /// rationale/pattern as `flush_budget_override` — see
    /// [`MergeFaninOverride`].
    merge_fanin_override: MergeFaninOverride,
    /// Plan segments S3b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §S3b, "cap de taille de tier"): per-index override for the
    /// tiered-merge doc-count cap, same rationale/pattern as
    /// `merge_fanin_override` — see [`MergeMaxDocsOverride`].
    merge_max_docs_override: MergeMaxDocsOverride,
}

/// Avance commune aux curseurs RAM et disque d'un segment. Le chemin P2 doit
/// distinguer une fin normale d'une lecture disque défaillante pour ne jamais
/// publier les hits déjà accumulés avant l'erreur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentPostingsAdvance {
    Found(u32),
    Exhausted,
    Error,
}

/// Curseur d'un terme dans un seul segment. Les variantes restent internes au
/// constructeur : les consommateurs ne voient que les opérations communes
/// d'avance, de fréquence et de `df` local.
#[derive(Debug)]
pub enum SegmentPostingsCursor<'a> {
    Disk(DiskPostingsCursor<'a>),
    Ram {
        iter: PostingsBlockSkipIter<'a>,
        freqs: &'a [u32],
        local_df: u64,
    },
}

impl SegmentPostingsCursor<'_> {
    /// `df` du terme dans ce seul segment, utilisé pour choisir localement le
    /// driver le plus rare. Le scoring conserve séparément le `df` global.
    pub fn local_df(&self) -> u64 {
        match self {
            Self::Disk(cursor) => cursor.doc_freq() as u64,
            Self::Ram { local_df, .. } => *local_df,
        }
    }

    /// Avance monotone qui préserve la provenance d'une erreur disque.
    pub fn advance_to_with_status(&mut self, target: u32) -> SegmentPostingsAdvance {
        match self {
            Self::Disk(cursor) => match cursor.advance_to_with_status(target) {
                DiskPostingsAdvance::Found(doc_id) => SegmentPostingsAdvance::Found(doc_id),
                DiskPostingsAdvance::Exhausted => SegmentPostingsAdvance::Exhausted,
                DiskPostingsAdvance::Error => SegmentPostingsAdvance::Error,
            },
            Self::Ram { iter, .. } => match iter.advance_to(target) {
                Some(doc_id) => SegmentPostingsAdvance::Found(doc_id),
                None => SegmentPostingsAdvance::Exhausted,
            },
        }
    }

    /// Fréquence du document renvoyé par la dernière avance réussie.
    pub fn freq(&self) -> u32 {
        match self {
            Self::Disk(cursor) => cursor.freq(),
            Self::Ram { iter, freqs, .. } => freqs
                .get(iter.position().saturating_sub(1))
                .copied()
                .unwrap_or_default(),
        }
    }

    /// Compteurs de blocs pour un curseur disque ; un curseur RAM ne charge
    /// aucun bloc FoR depuis le segment persistant.
    pub fn disk_block_counts(&self) -> Option<(u64, u64)> {
        match self {
            Self::Disk(cursor) => Some((cursor.blocks_read() as u64, cursor.blocks_total() as u64)),
            Self::Ram { .. } => None,
        }
    }
}

/// Vue checked d'un terme alignée sur tous les segments de l'index. Une case
/// `None` désigne une absence réelle dans ce segment, jamais une erreur de
/// lecture ; `global_df` additionne les `df` locaux avant tout scoring BM25.
#[derive(Debug)]
pub struct SegmentedPostings<'a> {
    per_segment: Vec<Option<SegmentPostingsCursor<'a>>>,
    global_df: u64,
}

impl<'a> SegmentedPostings<'a> {
    /// Somme des `df` locaux sur tous les segments qui portent ce terme.
    pub fn global_df(&self) -> u64 {
        self.global_df
    }

    /// Curseur du terme dans le segment demandé, ou `None` si le terme y est
    /// réellement absent.
    pub fn cursor_at_mut(&mut self, segment_idx: usize) -> Option<&mut SegmentPostingsCursor<'a>> {
        self.per_segment.get_mut(segment_idx)?.as_mut()
    }

    /// Compteurs cumulés des curseurs disque de cette vue. La collecte se
    /// fait après la tentative P2 afin d'inclure aussi les termes ouverts
    /// avant une erreur de construction ou un segment localement incomplet.
    pub fn disk_block_counts(&self) -> (u64, u64) {
        self.per_segment
            .iter()
            .flatten()
            .filter_map(SegmentPostingsCursor::disk_block_counts)
            .fold((0, 0), |(read, total), (cursor_read, cursor_total)| {
                (
                    read.saturating_add(cursor_read),
                    total.saturating_add(cursor_total),
                )
            })
    }
}

/// Plan segments S2: resolution mode for [`DocumentIndex::maybe_flush_by_budget`]
/// and the `_refresh`-time seal. `UseEnv` (the `Default`) reads
/// [`flush_budget_bytes`] (the process-wide `SURCH_FLUSH_BUDGET_BYTES`
/// `OnceLock`) — production default. `Forced(_)` is set via
/// [`DocumentIndex::set_flush_budget_bytes_override`] so a test can pin an
/// exact budget (or force "no budget") on ONE index instance,
/// independently of the env var and of any other test in the same
/// process.
#[derive(Debug, Clone, Copy, Default)]
enum FlushBudgetOverride {
    #[default]
    UseEnv,
    Forced(Option<u64>),
}

/// Plan segments S3 (`docs/paper/design-segments-pic-borne-2026-07-05.md`,
/// merge tiered inline): resolution mode for
/// [`DocumentIndex::maybe_merge_after_seal`]. `UseEnv` (the `Default`)
/// reads [`merge_fanin_bytes`] (the process-wide `SURCH_MERGE_FANIN`
/// `OnceLock`) — production default. `Forced(_)` is set via
/// [`DocumentIndex::set_merge_fanin_override`] so a test can pin an exact
/// fan-in (or `0`, the merge-disabled reversibility flag) on ONE index
/// instance, independently of the env var and of any other test in the
/// same process — same rationale as [`FlushBudgetOverride`].
#[derive(Debug, Clone, Copy, Default)]
enum MergeFaninOverride {
    #[default]
    UseEnv,
    Forced(usize),
}

/// Plan segments S3: env-configured tiered-merge fan-in threshold, read
/// ONCE per process (mirrors [`flush_budget_bytes`]'s established
/// `OnceLock` pattern). Defaults to `8` when unset or unparseable — a
/// LogMergePolicy-style fan-in that keeps a 28M-doc corpus at a budget of
/// a few hundred MiB from accumulating 100+ segments (the design doc's
/// "sans merge, le fan-out FST × S tue la latence"). `0` disables merging
/// entirely — the S3 reversibility flag: [`DocumentIndex::maybe_merge_after_seal`]
/// is then always a no-op and `segments` only ever grows, bit-identical to
/// S2 without this feature. A `1` is treated the same as `0` (a "run" of
/// one segment is not a merge).
fn merge_fanin_bytes() -> usize {
    static FANIN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FANIN.get_or_init(|| {
        std::env::var("SURCH_MERGE_FANIN")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(8)
    })
}

/// Plan segments S3b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
/// §S3b, "cap de taille de tier"): resolution mode for
/// [`DocumentIndex::find_eligible_merge_run`]'s doc-count cap. `UseEnv`
/// (the `Default`) reads [`merge_max_docs`] (the process-wide
/// `SURCH_MERGE_MAX_DOCS` `OnceLock`) — production default. `Forced(_)`
/// is set via [`DocumentIndex::set_merge_max_docs_override`] so a test can
/// pin an exact cap (or `None`, "no cap") on ONE index instance,
/// independently of the env var and of any other test in the same
/// process — same rationale as [`FlushBudgetOverride`]/[`MergeFaninOverride`].
#[derive(Debug, Clone, Copy, Default)]
enum MergeMaxDocsOverride {
    #[default]
    UseEnv,
    Forced(Option<u64>),
}

/// Plan segments S3b: env-configured cap, in TOTAL DOCUMENTS, on a
/// tiered-merge run's `Σ doc_count` — read ONCE per process (mirrors
/// [`merge_fanin_bytes`]'s established `OnceLock` pattern). `None` when
/// unset, empty, `"0"`, or unparseable — the reversibility flag: with no
/// cap configured, [`DocumentIndex::find_eligible_merge_run`] never
/// rejects a run on size, bit-identical to before this feature existed.
///
/// The measured 28M@4g wall (`docs/paper/design-segments-pic-borne-2026-07-05.md`,
/// "PLANCHER 28M FINAL"): the transient `DocumentIndex::merge_segments_run`
/// builds (merged term dictionary payload, per-field `doc_len_dense`
/// concat, merged sub-field columns) before spilling scales with the
/// MERGED run's `Σ doc_count` — at the corpus's LAST tier that is the
/// whole corpus (~28M docs) by construction (the tiered policy keeps
/// merging comparably-sized runs upward with no ceiling). `Some(cap)`
/// makes [`DocumentIndex::find_eligible_merge_run`] refuse to extend a
/// candidate run past `cap` total docs, bounding that transient to
/// `O(cap)` instead of `O(corpus)` — at the cost of more, smaller final
/// segments (the read path already fans out over `Vec<Arc<Segment>>`, see
/// the design doc's "read-path multi-segment"). A segment that already
/// reached (or exceeds) the cap on its own simply stops being eligible
/// for any further run (`sizes[start] > cap` alone already fails the very
/// first `candidate_sum` check), which is the intended terminal state —
/// not a bug: a ~28M segment already discharged its whole transient once
/// and should not be re-merged into something bigger under a cap that
/// exists specifically to prevent that.
fn merge_max_docs() -> Option<u64> {
    static MAX_DOCS: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *MAX_DOCS.get_or_init(|| {
        std::env::var("SURCH_MERGE_MAX_DOCS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|&docs| docs > 0)
    })
}

impl Default for DocumentIndex {
    fn default() -> Self {
        Self {
            segments: vec![Arc::new(Segment::default())],
            postings_builder: PostingsBuilder::new(),
            prefix_postings: BTreeMap::new(),
            terms_dirty: false,
            terms_build_count: Arc::new(AtomicU64::new(0)),
            postings_disk_enabled_override: None,
            next_doc_id_hint: 0,
            flush_budget_override: FlushBudgetOverride::UseEnv,
            merge_fanin_override: MergeFaninOverride::UseEnv,
            merge_max_docs_override: MergeMaxDocsOverride::UseEnv,
        }
    }
}

/// Plan segments S2: env-configured flush-by-budget threshold in bytes,
/// read ONCE per process (mirrors [`crate::postings::postings_disk_enabled`]'s
/// established `OnceLock` pattern). `None` when unset, empty, `"0"`, or
/// unparseable — the S1 reversibility flag: with no budget configured,
/// [`DocumentIndex::maybe_flush_by_budget`] is always a no-op and
/// `_refresh` never seals a new segment either, so `DocumentIndex` stays
/// forever mono-segment, bit-identical to before this feature existed.
fn flush_budget_bytes() -> Option<u64> {
    static BUDGET: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("SURCH_FLUSH_BUDGET_BYTES")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|&bytes| bytes > 0)
    })
}

/// Plan segments **S1** (`docs/paper/design-segments-pic-borne-2026-07-05.md`):
/// an immutable-per-generation bundle of the index state that is safe to
/// seal and share. Groups exactly the four pieces of state the design
/// calls "sealed at refresh": the term dictionary (with its disk-backed
/// postings segment), the per-field BM25 length stats, the sub-field
/// projection columns, and the live-doc presence bitset.
///
/// S1 scope: `DocumentIndex` always holds `Vec<Arc<Segment>>` of length
/// EXACTLY 1 — the pure structural refactor step, bit-identical to the
/// pre-segment layout by construction (a single segment IS the whole
/// index).
///
/// Plan segments **S2**: budget-triggered flush (env
/// `SURCH_FLUSH_BUDGET_BYTES`, see [`flush_budget_bytes`]) and `_refresh`
/// can now seal the currently-active segment and APPEND a fresh one, so
/// `segments.len()` can genuinely exceed 1. Segments are always kept in
/// ascending [`Segment::doc_base`] order (only ever pushed, never
/// reordered), and each one covers a CONTIGUOUS range of GLOBAL doc_ids
/// `[doc_base, doc_base + doc_count)`. Per the design's divergence note
/// (resolved for S2): **postings keep GLOBAL doc_ids** in their FoR lists
/// (no remap needed — a segment's own postings only ever reference
/// doc_ids inside its own range, which is already true by construction),
/// while the EAGER per-doc columns (`field_stats.doc_len_dense`,
/// `subfield_values`' `SubfieldColumn.codes`, `live_docs`) are indexed
/// LOCALLY (`doc_id - doc_base`) so a late segment's dense arrays stay
/// sized to ITS OWN doc count instead of the whole corpus (the "maladie
/// B" this avoids — see the design doc's diagnostic).
#[derive(Debug, Default, Clone)]
struct Segment {
    /// Term dictionary: FST + postings (RAM or disk-backed), sealed by
    /// the last `materialize_terms` / `materialize_terms_and_finalize_postings`.
    /// Postings inside store GLOBAL doc_ids (see the struct doc above).
    terms: TermDictionary,
    /// Per-field BM25 length stats, indexed LOCALLY (`doc_id - doc_base`
    /// — see the struct doc). Unlike `terms`, updated eagerly by every
    /// `merge_analyzed` call (never deferred behind `terms_dirty`) —
    /// moving it into `Segment` does not change when it becomes visible
    /// to readers, only where it physically lives.
    field_stats: BTreeMap<String, FieldLengthStats>,
    /// A10 (Phase 4) write-time sub-field projections, indexed LOCALLY
    /// (see [`SubfieldColumn`]'s doc comment for the full per-doc
    /// contract, and the struct doc above for the local-indexing
    /// rationale) — unchanged behaviour otherwise, only relocated from
    /// `DocumentIndex`.
    subfield_values: BTreeMap<String, SubfieldColumn>,
    /// Live document ids in this generation (presence bitset), indexed
    /// LOCALLY — see [`LiveDocsBitset`]'s doc comment for the encoding.
    /// Updated eagerly by `merge_analyzed`, same as `field_stats`.
    live_docs: LiveDocsBitset,
    /// Plan segments S2: the smallest GLOBAL doc_id this segment covers.
    /// Fixed once, at the moment the segment is created (either the
    /// first-ever segment, always `0`, or a freshly-pushed segment after
    /// a budget flush / `_refresh` seal, set to `DocumentIndex`'s
    /// `next_doc_id_hint` at that instant) — never mutated afterwards.
    /// Every eager column above is indexed by `global_doc_id - doc_base`.
    doc_base: u32,
    /// Plan segments S2: number of doc_ids this SEALED segment covers
    /// (`next_doc_id_hint - doc_base` at the moment it was sealed).
    /// Meaningless (`0`) for the currently-active (not yet sealed)
    /// segment — nothing reads it until sealing sets it.
    doc_count: u32,
    /// Plan segments S5c: shared, `pread`-addressed file backing every
    /// disk-spilled column of `subfield_values` (`None` while the column
    /// is being written, while `SURCH_POSTINGS_DISK` is off, or if no
    /// column of this segment was actually spilled — see
    /// [`Self::seal_subfield_columns`]). Exactly one instance per sealed
    /// segment, `Arc`-shared the same way `TermDictionary::postings_segment`
    /// is, dropped (and its tempfile removed) once no `Segment` still
    /// references it.
    subfield_segment: Option<Arc<SubfieldSegment>>,
}

impl Segment {
    /// Plan segments S5c (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §S5c): finalize (clear write-time interning, matches every other
    /// sealed segment's contract) and, best-effort, disk-back every column
    /// of `self.subfield_values` into ONE shared per-segment file.
    ///
    /// Plan segments S3b: the S3 merge path (`DocumentIndex::merge_segments_run`)
    /// no longer calls this whole method — it streams each column's
    /// merge+spill directly (`merge_subfield_values`/
    /// `merge_subfield_column_streamed`) instead of building a
    /// fully-resident merged column and spilling it here afterwards; only
    /// the per-column `SubfieldColumn::spill` call this method also uses is
    /// reused there, as a best-effort catch-up for any ONE path that failed
    /// to stream (see `merge_subfield_values`'s doc comment). This method
    /// itself is now solely the ordinary seal path's
    /// (`DocumentIndex::materialize_active_segment_terms`) producer.
    ///
    /// Idempotent: calling this twice on an already-sealed segment is a
    /// strict no-op — every column's `intern_index` is already empty, and
    /// the `needs_spill` gate below sees nothing left to spill, so no file
    /// is even created (a `_refresh` with no write in between must not
    /// churn a tempfile per call).
    ///
    /// A segment that ALREADY holds a `subfield_segment` file never spills
    /// again either: its spilled columns' descriptors point into THAT
    /// file, and replacing it with a fresh one would silently corrupt
    /// their offsets. (Unreachable anyway — a sealed segment never gains
    /// new column data, see [`SubfieldColumn::set`]'s guard — but the gate
    /// makes the corruption structurally impossible rather than merely
    /// unreached.)
    ///
    /// `disk_enabled == false` (the flag-off default) makes this exactly
    /// the historical `column.finalize()` loop: no file is even created,
    /// every column stays fully resident, bit-identical to before this
    /// feature existed.
    fn seal_subfield_columns(&mut self, disk_enabled: bool) {
        let needs_spill = disk_enabled
            && self.subfield_segment.is_none()
            && self
                .subfield_values
                .values()
                .any(|column| column.disk.is_none() && !column.dict.is_empty());
        let mut store = if needs_spill {
            SubfieldSegment::try_new()
        } else {
            None
        };
        let mut any_spilled = false;
        for column in self.subfield_values.values_mut() {
            column.finalize();
            if let Some(store) = store.as_mut() {
                if column.spill(store) {
                    any_spilled = true;
                }
            }
        }
        if any_spilled {
            self.subfield_segment = store.map(Arc::new);
        }
    }
}

/// Lot C Phase 1 lever 2: dense, dict-interned column backing
/// `Segment::subfield_values` for ONE qualified sub-field path
/// (`"NOM.raw"`).
///
/// Replaces `BTreeMap<u32, String>` (one B-tree node + one heap `String`
/// per doc) with:
///
/// - `dict`: the distinct values actually seen, deduplicated. French
///   surnames/prefixes are extremely repetitive, so `dict.len()` is
///   orders of magnitude smaller than the doc count.
/// - `codes`: one `u32` per `doc_id`, `u32::MAX` ([`SubfieldColumn::ABSENT`])
///   meaning "this doc has no value for this path". Grows by
///   resize-on-write exactly like `FieldLengthStats::doc_len_dense` — the
///   array is only as long as the highest `doc_id` written so far, NOT
///   bounded by the live doc count (deletes leave holes, `doc_id`s are
///   never reused, see `surch-api::InMemoryIndex::next_doc_id`).
/// - `intern_index`: write-time-only `value -> code` lookup so repeated
///   `add_documents_with_mapping*` batches within the same index
///   generation reuse the same code for a value seen before, instead of
///   re-scanning `dict` (O(1) amortized insert instead of O(dict.len())).
///   Its keys duplicate the SAME distinct strings already held by `dict`
///   (one extra small allocation per DISTINCT value, not per doc), which
///   is intentionally simple: on this workload `dict.len()` is tiny next
///   to `codes.len()`, so the duplication is noise compared to the
///   millions of eliminated per-doc `String`/B-tree-node allocations.
///
/// A doc absent from a path (parent field missing/empty for that doc)
/// MUST stay `ABSENT`, never an interned empty string — sort/agg parity
/// with the ES oracle depends on distinguishing "no value" from "empty
/// string value" (flagged in the Lot C Phase 1 lever 2 triple consensus).
///
/// Plan segments **S5c** (`docs/paper/design-segments-pic-borne-2026-07-05.md`
/// §S5c): once a segment is sealed, `dict`+`codes` are BOTH
/// `O(corpus)`-adjacent RESIDENT arrays — `codes` is genuinely `O(docs ×
/// columns)` (a dense, doc-indexed array; the "maladie B" driver this
/// whole S5 lift targets), while `dict` is `O(distinct values)`, bounded
/// by the real-world vocabulary (surnames/communes/dates saturate long
/// before the corpus does) rather than by doc count. Every real consumer
/// (`DocumentIndex::subfield_projection`'s sort/agg reader, the S3 merge)
/// reads the WHOLE column in one sequential pass, never a single random
/// `doc_id` — see the design doc's access-pattern study — so `dict` and
/// `codes` are spilled TOGETHER, in one batched `append()` per column
/// ([`Self::spill`]), rather than splitting them across two files/regions:
/// they are always consumed together, and `dict` is small enough that
/// separating it would only add bookkeeping for no query-time benefit.
/// `disk` records the resulting `pread` descriptor; `dict`/`codes` are
/// then emptied (`Vec::new()`, 0 bytes) and every read goes through
/// [`Self::materialized`] instead.
#[derive(Debug, Default, Clone)]
pub struct SubfieldColumn {
    dict: Vec<Box<str>>,
    codes: Vec<u32>,
    intern_index: HashMap<Box<str>, u32>,
    /// `Some(_)` once this column's `dict`+`codes` have been spilled (see
    /// [`Self::spill`]) — `None` is the flag-off default AND the state of
    /// every column still being written to (a column is only ever spilled
    /// once, at seal time, by [`Segment::seal_subfield_columns`]).
    disk: Option<SubfieldColumnDisk>,
}

/// Plan segments S5c: `pread` descriptor for one disk-spilled
/// [`SubfieldColumn`]. All fields are `u32`/`u64` SCALARS — O(1) per
/// column (a handful of `.raw`/`.norm` paths per index), never O(docs) or
/// O(distinct values): this descriptor is the ENTIRE resident cost of a
/// spilled column.
#[derive(Debug, Clone, Copy, Default)]
struct SubfieldColumnDisk {
    /// Segment-absolute byte offset of the `(dict_len + 1)`-entry `u32`
    /// LE CSR offsets table (`dict_offsets[i]..dict_offsets[i+1]` spans
    /// dict entry `i`'s bytes inside `dict_bytes`).
    dict_offsets_base: u64,
    /// Number of distinct values (`dict.len()` at spill time).
    dict_len: u32,
    /// Segment-absolute byte offset of the concatenated UTF-8 dict bytes.
    dict_bytes_base: u64,
    /// Total byte length of the concatenated dict bytes.
    dict_bytes_len: u32,
    /// Segment-absolute byte offset of the `codes` table (`codes_len` LE
    /// `u32` entries, same values `SubfieldColumn::codes` held —
    /// `ABSENT` sentinel included).
    codes_base: u64,
    /// Number of `u32` entries in the codes table (`codes.len()` at spill
    /// time).
    codes_len: u32,
}

impl SubfieldColumn {
    /// Sentinel `codes` entry meaning "no value for this doc_id".
    pub const ABSENT: u32 = u32::MAX;

    /// Record `value` for `doc_id`, interning it into `dict` (reusing the
    /// code of an already-seen equal value). Resizes `codes` with the
    /// `ABSENT` sentinel exactly like `FieldLengthStats::record_doc_len`
    /// resizes `doc_len_dense` with `0` — every position between the
    /// previous length and `doc_id` that has no explicit `set()` call
    /// stays `ABSENT`.
    fn set(&mut self, doc_id: u32, value: String) {
        // Plan segments S5c: writing into an already-spilled column would
        // be silently INVISIBLE to reads (`materialized` ignores the
        // resident `dict`/`codes` once `disk` is `Some`). Unreachable by
        // the same guarantee that makes `finalize()`'s intern-index drop
        // safe (the first write after ANY seal targets either a FRESH
        // active segment's brand-new column, or follows a full
        // `DocumentIndex::clear()` — see `finalize()`'s doc comment), so
        // this is a loud debug-build guard, not a release branch.
        debug_assert!(
            self.disk.is_none(),
            "SubfieldColumn::set on a disk-spilled (sealed) column — writes must \
             target the fresh active segment's own column"
        );
        let code = self.intern(value);
        let idx = doc_id as usize;
        if idx >= self.codes.len() {
            self.codes.resize(idx + 1, Self::ABSENT);
        }
        self.codes[idx] = code;
    }

    /// Returns the existing code for `value` if already interned,
    /// otherwise allocates the next code and stores `value` in `dict`
    /// (and, duplicated, as the `intern_index` key — see the struct docs).
    fn intern(&mut self, value: String) -> u32 {
        if let Some(&code) = self.intern_index.get(value.as_str()) {
            return code;
        }
        debug_assert!(
            self.dict.len() < Self::ABSENT as usize,
            "sub-field dictionary exceeded u32::MAX - 1 distinct values"
        );
        let code = self.dict.len() as u32;
        let boxed: Box<str> = value.into_boxed_str();
        self.intern_index.insert(boxed.clone(), code);
        self.dict.push(boxed);
        code
    }

    /// Stored value for `doc_id`, zero-copy borrowed from `dict`. `None`
    /// when `doc_id` is out of range or its code is the `ABSENT` sentinel.
    pub fn get(&self, doc_id: u32) -> Option<&str> {
        let code = *self.codes.get(doc_id as usize)?;
        if code == Self::ABSENT {
            None
        } else {
            Some(self.dict[code as usize].as_ref())
        }
    }

    /// `(doc_id, value)` pairs in ascending `doc_id` order, `ABSENT`
    /// entries omitted — the exact iteration contract the previous
    /// `BTreeMap<u32, String>::iter()` provided.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.codes
            .iter()
            .enumerate()
            .filter(|&(_, &code)| code != Self::ABSENT)
            .map(|(idx, &code)| (idx as u32, self.dict[code as usize].as_ref()))
    }

    /// Lot C `C2` : libere `intern_index`. Provably write-only after this
    /// point (grep audit: the only reader is `intern()` itself, called
    /// exclusively from the write path `set()`/`add_documents_with_mapping*`
    /// — `get()`/`iter()` above read `dict`/`codes`, never `intern_index`).
    /// `dict`/`codes` are left untouched (still required for reads).
    ///
    /// Called once per `_refresh` (mirrors `DocumentIndex::finalize_postings`
    /// dropping `postings_builder`). Safe because the next `set()`/`intern()`
    /// call — if any — is always preceded by a full `DocumentIndex::clear()`
    /// (see `surch-api::InMemoryIndex::append_to_index`'s `terms_finalized`
    /// guard: the first write after ANY refresh always routes through
    /// `rebuild_index()`, which clears and fully repopulates
    /// `subfield_values` from every currently-live doc). So clearing
    /// `intern_index` here never causes a `dict` value to be re-interned
    /// with a duplicate code across write batches — it only frees memory
    /// slightly earlier than that guaranteed next `clear()` would anyway.
    ///
    /// Plan segments S2: also called when a budget flush seals the active
    /// segment (`DocumentIndex::maybe_flush_by_budget`). Safe for the
    /// same write-only reason, with an even simpler successor guarantee:
    /// the very next `set()` after a flush targets the FRESH active
    /// segment's own brand-new column (`entry(path).or_default()` on an
    /// empty map), never this sealed one — a sealed segment's columns are
    /// immutable for the rest of their life.
    ///
    /// Plan segments S5c: called from [`Segment::seal_subfield_columns`]
    /// (the single place that now seals a segment's whole
    /// `subfield_values` map), immediately before that same call
    /// best-effort attempts [`Self::spill`] — `intern_index` must already
    /// be empty by the time `spill` runs so its byte accounting
    /// (`memory_bytes`) is exact.
    fn finalize(&mut self) {
        self.intern_index = HashMap::new();
    }

    /// Plan segments S5c (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §S5c, following the S5a/S5b template): best-effort disk-back the
    /// WHOLE column (`dict` + `codes`) into `store` with exactly ONE
    /// `append()` call. The C0 lesson (commit `af94e52`, reverted the same
    /// day: one `pwrite` PER VALUE cost -43 % indexation throughput) is
    /// why this is called ONCE PER COLUMN, only from
    /// [`Segment::seal_subfield_columns`] at seal time — never from the
    /// write path (`merge_analyzed`/[`Self::set`]).
    ///
    /// Layout: `[dict_offsets: (dict.len()+1) × u32 LE CSR] [dict_bytes:
    /// UTF-8, concatenated] [codes: codes.len() × u32 LE]`. The three
    /// region boundaries are cheap scalars kept resident in
    /// [`SubfieldColumnDisk`] — O(columns), not O(corpus).
    ///
    /// No-op (`false`, stays fully resident) when already spilled or
    /// empty (nothing to gain — an empty column costs 0 bytes already).
    /// On an `append` failure the column ALSO stays resident: best-effort,
    /// matches `postings::persist_or_keep_term_directory`'s contract — a
    /// disk-back feature must never lose data or panic.
    fn spill(&mut self, store: &mut subfield_segment::SubfieldSegment) -> bool {
        if self.disk.is_some() || self.dict.is_empty() {
            return false;
        }
        // The descriptor's length fields — and [`SubfieldSegment::read`]'s
        // `length` parameter — are `u32`: a column whose dict bytes,
        // offsets-table bytes (`(dict.len()+1) × 4`) or codes-table bytes
        // (`codes.len() × 4`) could not round-trip through a `u32` must
        // REFUSE to spill (stay resident, still correct) rather than
        // saturate the offsets below into a blob [`Self::read_spilled`]
        // could never read back whole. Unreachable on real corpora (a
        // segment's codes are bounded by its own doc_count, and gigabytes
        // of DISTINCT value bytes in one column dwarf every observed
        // dict), so this is a guard, not a branch that ever costs
        // anything.
        let max_u32 = u32::MAX as usize;
        let dict_total_bytes: usize = self.dict.iter().map(|value| value.len()).sum();
        if dict_total_bytes > max_u32
            || self.dict.len() >= max_u32 / 4
            || self.codes.len() > max_u32 / 4
        {
            return false;
        }
        let mut dict_offsets: Vec<u32> = Vec::with_capacity(self.dict.len() + 1);
        let mut dict_bytes: Vec<u8> = Vec::new();
        let mut offset = 0u32;
        dict_offsets.push(0);
        for value in &self.dict {
            offset = offset.saturating_add(value.len() as u32);
            dict_offsets.push(offset);
            dict_bytes.extend_from_slice(value.as_bytes());
        }
        let codes_len = self.codes.len();
        let mut blob = Vec::with_capacity(
            dict_offsets
                .len()
                .saturating_mul(4)
                .saturating_add(dict_bytes.len())
                .saturating_add(codes_len.saturating_mul(4)),
        );
        for entry in &dict_offsets {
            blob.extend_from_slice(&entry.to_le_bytes());
        }
        blob.extend_from_slice(&dict_bytes);
        for &code in &self.codes {
            blob.extend_from_slice(&code.to_le_bytes());
        }
        let Some(base) = store.append(&blob) else {
            return false;
        };
        let dict_offsets_len = (dict_offsets.len() as u64).saturating_mul(4);
        let dict_bytes_base = base.saturating_add(dict_offsets_len);
        let codes_base = dict_bytes_base.saturating_add(dict_bytes.len() as u64);
        self.disk = Some(SubfieldColumnDisk {
            dict_offsets_base: base,
            dict_len: self.dict.len() as u32,
            dict_bytes_base,
            dict_bytes_len: dict_bytes.len() as u32,
            codes_base,
            codes_len: codes_len as u32,
        });
        self.dict = Vec::new();
        self.codes = Vec::new();
        true
    }

    /// Plan segments S5c: resident-or-`pread` accessor — mirrors
    /// `postings::FieldPostings::term_entry`'s single-accessor pattern.
    /// Returns a zero-copy borrow when this column was never spilled
    /// (`disk.is_none()`: the flag-off default, or a column too small to
    /// have been spilled), otherwise reads the WHOLE `dict`+`codes` blob
    /// back with exactly THREE grouped `pread`s (offsets, dict bytes,
    /// codes — never one `pread` per value): matches the full-column-scan
    /// access pattern of every real consumer
    /// (`DocumentIndex::subfield_projection`'s sort/agg reader and the S3
    /// merge, see the design doc's §S5c access-pattern study). A `pread`
    /// failure (file gone, truncated…) degrades to an EMPTY column
    /// (`Cow::Owned(SubfieldColumn::default())`) rather than panicking —
    /// same best-effort contract as every other disk-backed read in this
    /// crate. `store` is `None` when the owning segment's file failed to
    /// open or disk-back is off, in which case a spilled column cannot
    /// legitimately exist in practice.
    fn materialized(
        &self,
        store: Option<&subfield_segment::SubfieldSegment>,
    ) -> Cow<'_, SubfieldColumn> {
        let Some(disk) = self.disk else {
            return Cow::Borrowed(self);
        };
        let rebuilt = store
            .and_then(|store| Self::read_spilled(disk, store))
            .unwrap_or_default();
        Cow::Owned(rebuilt)
    }

    /// Plan segments S3b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §S3b): the DICT-only half of [`Self::materialized`] — 2 grouped
    /// `pread`s (offsets, dict bytes) when spilled, skipping the
    /// potentially large codes region entirely — used by the S3b streamed
    /// merge's dict pass (`merge_subfield_column_streamed`), which needs
    /// every source's vocabulary but not yet its per-doc codes. A resident
    /// column (never spilled) returns a cheap clone of `self.dict`
    /// (already in RAM, no I/O). `None` on any I/O/decode failure — same
    /// best-effort contract as `materialized`, but propagated as `None`
    /// (rather than degrading to empty) so the streamed merge can fall
    /// back to the fully-resident path for this one column instead of
    /// silently losing values.
    fn dict_only(
        &self,
        store: Option<&subfield_segment::SubfieldSegment>,
    ) -> Option<Vec<Box<str>>> {
        let Some(disk) = self.disk else {
            return Some(self.dict.clone());
        };
        Self::read_spilled_dict(disk, store?)
    }

    /// Plan segments S3b: the CODES-only half of [`Self::materialized`] —
    /// 1 `pread` when spilled, WITHOUT re-reading the dict header (already
    /// consumed by [`Self::dict_only`] in the streamed merge's first pass)
    /// — used by the streamed merge's codes pass. A resident column
    /// returns a cheap clone of `self.codes`. `None` on any I/O/decode
    /// failure (same rationale as `dict_only`).
    fn codes_only(&self, store: Option<&subfield_segment::SubfieldSegment>) -> Option<Vec<u32>> {
        let Some(disk) = self.disk else {
            return Some(self.codes.clone());
        };
        Self::read_spilled_codes(disk, store?)
    }

    /// Decode a spilled column's DICT region only — factored out of
    /// [`Self::read_spilled`] so [`Self::dict_only`] can skip the codes
    /// `pread` entirely. Returns `None` on any I/O or decode failure.
    fn read_spilled_dict(
        disk: SubfieldColumnDisk,
        store: &subfield_segment::SubfieldSegment,
    ) -> Option<Vec<Box<str>>> {
        let offsets_len = u32::try_from((u64::from(disk.dict_len) + 1).saturating_mul(4)).ok()?;
        let offsets_bytes = store.read(disk.dict_offsets_base, offsets_len)?;
        let dict_bytes = store.read(disk.dict_bytes_base, disk.dict_bytes_len)?;

        let mut offsets = Vec::with_capacity(disk.dict_len as usize + 1);
        for chunk in offsets_bytes.chunks_exact(4) {
            offsets.push(u32::from_le_bytes(chunk.try_into().ok()?));
        }
        let mut dict = Vec::with_capacity(disk.dict_len as usize);
        for window in offsets.windows(2) {
            let (start, end) = (window[0] as usize, window[1] as usize);
            let slice = dict_bytes.get(start..end)?;
            dict.push(String::from_utf8(slice.to_vec()).ok()?.into_boxed_str());
        }
        Some(dict)
    }

    /// Decode a spilled column's CODES region only — factored out of
    /// [`Self::read_spilled`] so [`Self::codes_only`] can skip the dict
    /// `pread`s entirely. Returns `None` on any I/O or decode failure.
    fn read_spilled_codes(
        disk: SubfieldColumnDisk,
        store: &subfield_segment::SubfieldSegment,
    ) -> Option<Vec<u32>> {
        let codes_bytes = store.read(disk.codes_base, disk.codes_len.saturating_mul(4))?;
        let mut codes = Vec::with_capacity(disk.codes_len as usize);
        for chunk in codes_bytes.chunks_exact(4) {
            codes.push(u32::from_le_bytes(chunk.try_into().ok()?));
        }
        Some(codes)
    }

    /// Decode a spilled column's blob back into an owned, fully resident
    /// [`SubfieldColumn`] (`disk: None`) — the inverse of [`Self::spill`].
    /// Returns `None` on any I/O or decode failure (truncated file,
    /// corrupt length…), letting the caller fall back to an empty column
    /// rather than panicking.
    fn read_spilled(
        disk: SubfieldColumnDisk,
        store: &subfield_segment::SubfieldSegment,
    ) -> Option<SubfieldColumn> {
        let dict = Self::read_spilled_dict(disk, store)?;
        let codes = Self::read_spilled_codes(disk, store)?;
        Some(SubfieldColumn {
            dict,
            codes,
            intern_index: HashMap::new(),
            disk: None,
        })
    }

    /// Lot C Phase 1 lever 2 memory accounting: approximate heap bytes
    /// held by this column, for [`crate::memory::document_index_memory_usage`].
    /// `dict`: one `Box<str>` fat-pointer header (16 B on 64-bit) + UTF-8
    /// bytes per DISTINCT value. `codes`: 4 B per `doc_id` slot (dense,
    /// includes `ABSENT` holes — same accounting convention as
    /// `field_stats_bytes` counting the full `doc_len_dense` slice).
    /// `intern_index`: duplicate `Box<str>` keys over the same distinct
    /// values as `dict`, plus a conservative per-entry hash-table
    /// overhead — bounded by `dict.len()`, negligible next to `codes`.
    ///
    /// Plan segments S5c: `dict`/`codes`/`intern_index` are all empty once
    /// [`Self::spill`] succeeded, so this naturally reads ~0 for a spilled
    /// column (just the tiny [`SubfieldColumnDisk`] descriptor, added
    /// explicitly below for an exact count) — no separate branch needed.
    pub fn memory_bytes(&self) -> u64 {
        let box_str_header = std::mem::size_of::<Box<str>>() as u64;
        let dict_bytes: u64 = self
            .dict
            .iter()
            .map(|value| box_str_header + value.len() as u64)
            .sum();
        let codes_bytes = self.codes.len() as u64 * std::mem::size_of::<u32>() as u64;
        // Conservative per-entry overhead for the `HashMap` bucket
        // metadata (hash + control byte + pointer-sized slack), on top of
        // the duplicated `Box<str>` key.
        let intern_entry_overhead = box_str_header + std::mem::size_of::<u32>() as u64 + 16;
        let intern_bytes: u64 = self
            .intern_index
            .keys()
            .map(|value| intern_entry_overhead + value.len() as u64)
            .sum();
        let disk_descriptor_bytes = if self.disk.is_some() {
            std::mem::size_of::<SubfieldColumnDisk>() as u64
        } else {
            0
        };
        dict_bytes + codes_bytes + intern_bytes + disk_descriptor_bytes
    }
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

/// Plan segments S1: `Σ`-aggregated BM25 length stats for one field,
/// across every sealed segment — see
/// [`DocumentIndex::field_stats_aggregated`]. `doc_count`/`total_terms`
/// are the raw sums (same domain as [`FieldLengthStats::doc_count`] /
/// `total_terms`); [`Self::avg_doc_len`] reproduces
/// [`FieldLengthStats::avg_doc_len`]'s exact formula (`total_terms as
/// f64 / doc_count as f64`, same guard), just fed the summed inputs —
/// with one segment the sum is a no-op, so the division is the SAME
/// single floating-point operation, bit-for-bit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AggregatedFieldStats {
    pub doc_count: u64,
    pub total_terms: u64,
    /// `0` means "no segment recorded a length" (norms disabled or no
    /// docs), mirroring [`FieldLengthStats::min_doc_len`]'s sentinel.
    pub min_doc_len: u64,
}

impl AggregatedFieldStats {
    /// Same formula and guard as [`FieldLengthStats::avg_doc_len`]:
    /// `None` unless both the doc count and the term count are
    /// positive.
    pub fn avg_doc_len(&self) -> Option<f64> {
        (self.doc_count > 0 && self.total_terms > 0)
            .then(|| self.total_terms as f64 / self.doc_count as f64)
    }

    /// `None` when no segment ever recorded a length (mirrors
    /// [`FieldLengthStats::min_doc_len`]).
    pub fn min_doc_len(&self) -> Option<u64> {
        (self.min_doc_len > 0).then_some(self.min_doc_len)
    }
}

impl DocumentIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plan segments S2: read-only handle on `segments[0]`. Valid ONLY
    /// when the caller has already established `segments.len() == 1`
    /// (e.g. via [`Self::segment_count`] or the multi-segment-aware
    /// [`Self::postings_disk_backed`] returning `false`) — every method
    /// below that still routes through this accessor (`terms`,
    /// `postings`, `block_metas`, `postings_with_block_metas`,
    /// `disk_cursor`, `field_stats`) is ITSELF only ever called from such
    /// a single-segment-gated call site (see the design's S2 read-path
    /// note), so `segments[0]` and "the only segment" coincide there.
    /// With budget flush unset (the S1 reversibility flag) that is
    /// unconditionally true forever — bit-identical to S1.
    fn segment(&self) -> &Segment {
        &self.segments[0]
    }

    /// Plan segments S2: mutable handle on the CURRENTLY ACTIVE segment
    /// — the one eager writes (`merge_analyzed`) and the next
    /// `materialize_terms*` target — which is always `segments.last()`.
    /// `Arc::make_mut` is used purely as the ownership container here —
    /// nothing ever clones a live segment's `Arc` out to a second owner,
    /// so `strong_count` is always 1 and this never actually clones the
    /// `Segment` (no new allocation over the pre-segment direct-field
    /// layout). With budget flush unset there is only ever one segment,
    /// so this is exactly the S1 `segments[0]` target — bit-identical.
    fn segment_mut(&mut self) -> &mut Segment {
        Arc::make_mut(
            self.segments
                .last_mut()
                .expect("DocumentIndex always holds at least one segment"),
        )
    }

    /// Plan segments S2: number of sealed/active segments currently held.
    /// `1` forever when `SURCH_FLUSH_BUDGET_BYTES` is unset (or forced
    /// off via [`Self::set_flush_budget_bytes_override`]) — the S1
    /// reversibility flag.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Plan segments S2: read-only handle on the currently active
    /// (last) segment — the immutable counterpart of [`Self::segment_mut`].
    fn active_segment(&self) -> &Segment {
        self.segments
            .last()
            .expect("DocumentIndex always holds at least one segment")
    }

    /// Plan segments S2: per-index override for the flush-by-budget
    /// threshold — see [`FlushBudgetOverride`]. MUST be called before any
    /// document is indexed to take effect deterministically (mirrors
    /// [`Self::set_postings_disk_enabled`]'s contract). `None` forces "no
    /// budget" (mono-segment) regardless of the env var; `Some(bytes)`
    /// forces that exact budget.
    pub fn set_flush_budget_bytes_override(&mut self, budget: Option<u64>) {
        self.flush_budget_override = FlushBudgetOverride::Forced(budget);
    }

    /// Plan segments S2: the flush-by-budget threshold this index should
    /// use right now — the per-index override if one was set, otherwise
    /// the process-wide [`flush_budget_bytes`] env var.
    fn resolved_flush_budget_bytes(&self) -> Option<u64> {
        match self.flush_budget_override {
            FlushBudgetOverride::UseEnv => flush_budget_bytes(),
            FlushBudgetOverride::Forced(budget) => budget,
        }
    }

    /// Plan segments S3: per-index override for the tiered-merge fan-in —
    /// see [`MergeFaninOverride`]. MUST be called before any document is
    /// indexed to take effect deterministically (mirrors
    /// [`Self::set_flush_budget_bytes_override`]'s contract). `0` forces
    /// "merge disabled" regardless of the env var; any other value forces
    /// that exact fan-in.
    pub fn set_merge_fanin_override(&mut self, fanin: usize) {
        self.merge_fanin_override = MergeFaninOverride::Forced(fanin);
    }

    /// Plan segments S3: the tiered-merge fan-in this index should use
    /// right now — the per-index override if one was set, otherwise the
    /// process-wide [`merge_fanin_bytes`] env var.
    fn resolved_merge_fanin(&self) -> usize {
        match self.merge_fanin_override {
            MergeFaninOverride::UseEnv => merge_fanin_bytes(),
            MergeFaninOverride::Forced(fanin) => fanin,
        }
    }

    /// Plan segments S3b: per-index override for the tiered-merge doc-count
    /// cap — see [`MergeMaxDocsOverride`]. MUST be called before any
    /// document is indexed to take effect deterministically (mirrors
    /// [`Self::set_flush_budget_bytes_override`]'s contract). `None` forces
    /// "no cap" regardless of the env var; `Some(docs)` forces that exact
    /// cap.
    pub fn set_merge_max_docs_override(&mut self, max_docs: Option<u64>) {
        self.merge_max_docs_override = MergeMaxDocsOverride::Forced(max_docs);
    }

    /// Plan segments S3b: the tiered-merge doc-count cap this index should
    /// use right now — the per-index override if one was set, otherwise
    /// the process-wide [`merge_max_docs`] env var.
    fn resolved_merge_max_docs(&self) -> Option<u64> {
        match self.merge_max_docs_override {
            MergeMaxDocsOverride::UseEnv => merge_max_docs(),
            MergeMaxDocsOverride::Forced(max_docs) => max_docs,
        }
    }

    /// Plan segments S2: materialize the active segment's pending
    /// `postings_builder`/sub-field-intern state IN PLACE — byte-for-byte
    /// the work [`Self::materialize_terms_and_finalize_postings`] always
    /// did before this refactor, just factored out so both the budget
    /// check and the `_refresh` path can share it without duplicating the
    /// no-clone builder move.
    fn materialize_active_segment_terms(&mut self) {
        // Plan segments S5c: resolved once, unconditionally — `terms`'s
        // disk mode and `subfield_values`' disk-back share the SAME
        // process flag (`SURCH_POSTINGS_DISK`, no new knob), so both
        // producers of a sealed segment must agree on it regardless of
        // which `if` branch below actually rebuilds `terms`.
        let disk_enabled = self.resolved_postings_disk_enabled();
        if self.terms_dirty {
            let builder = std::mem::replace(&mut self.postings_builder, PostingsBuilder::new());
            let new_terms = builder.build_with_disk_flag(disk_enabled);
            self.segment_mut().terms = new_terms;
            self.terms_dirty = false;
            self.terms_build_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.postings_builder = PostingsBuilder::new();
        }
        self.segment_mut().seal_subfield_columns(disk_enabled);
    }

    /// Plan segments S2: if the active segment actually holds at least
    /// one document (`next_doc_id_hint > active.doc_base` — a freshly
    /// sealed-and-replaced segment starts empty, so re-sealing it again
    /// with nothing written in between would be a wasted no-op entry),
    /// stamp its final `doc_count` and APPEND a fresh, empty `Segment` to
    /// `self.segments`, which becomes the new active one. Must be called
    /// AFTER [`Self::materialize_active_segment_terms`] so the
    /// about-to-be-former-active segment's `terms`/`subfield_values` are
    /// already sealed.
    fn start_new_active_segment_if_nonempty(&mut self) {
        let active_doc_base = self.active_segment().doc_base;
        if self.next_doc_id_hint <= active_doc_base {
            return;
        }
        self.segment_mut().doc_count = self.next_doc_id_hint - active_doc_base;
        self.segments.push(Arc::new(Segment {
            doc_base: self.next_doc_id_hint,
            ..Segment::default()
        }));
    }

    /// Plan segments S3 (`docs/paper/design-segments-pic-borne-2026-07-05.md`,
    /// "merge tiered inline sur runs adjacents"): after a seal has just
    /// appended a fresh sealed segment (`start_new_active_segment_if_nonempty`),
    /// repeatedly find and merge the longest eligible run of ADJACENT,
    /// comparably-sized SEALED segments — never the currently-active
    /// (last, still-being-written) one — until none remains. Merges can
    /// cascade: a freshly-merged segment becomes a candidate for the next
    /// tier up on a LATER call (each call only merges runs that exist
    /// RIGHT NOW; a merge changes segment sizes, so the search restarts
    /// from scratch after every merge inside the `while` loop below).
    ///
    /// No-op — and therefore free, a single `OnceLock`/override read plus
    /// an O(sealed segments) scan that finds nothing — when the resolved
    /// fan-in is `< 2` (`SURCH_MERGE_FANIN=0`, the S3 reversibility flag,
    /// or a forced `1`, which is not a merge). With budget flush unset
    /// (S1/S2 reversibility) `segments.len()` never exceeds 1, so there is
    /// never more than zero sealed segments to consider either way.
    fn maybe_merge_after_seal(&mut self) {
        let fanin = self.resolved_merge_fanin();
        if fanin < 2 {
            return;
        }
        let max_docs = self.resolved_merge_max_docs();
        while let Some((start, end)) = self.find_eligible_merge_run(fanin, max_docs) {
            self.merge_segments_run(start, end);
        }
    }

    /// Plan segments S3: the longest run of ADJACENT, comparably-sized
    /// SEALED segments (`self.segments[..len-1]` — the last entry is
    /// always the currently-active, not-yet-sealed segment, see
    /// [`Self::active_segment`]) whose length is `>= fanin`, as a
    /// half-open `[start, end)` index range. "Comparably-sized" (the
    /// design doc's LogMergePolicy-style policy): every segment's
    /// `doc_count` inside the run stays within ~10x of the run's smallest
    /// — tracked incrementally as a running `(min, max)` over `doc_count`
    /// while extending a candidate run rightward. Ties (same longest
    /// length) are broken leftmost. Returns `None` when no eligible run
    /// exists (including the trivial "fewer than `fanin` sealed segments
    /// at all" case).
    ///
    /// Plan segments S3b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §S3b, "cap de taille de tier"): `max_docs` (when `Some`, from
    /// [`Self::resolved_merge_max_docs`]) ALSO bounds the running `Σ
    /// doc_count` of a candidate run — extension stops the moment the NEXT
    /// segment would push the run's total past the cap, exactly like the
    /// existing size-ratio bound above (same `while` loop, one more early
    /// `break`). This bounds [`Self::merge_segments_run`]'s transient (the
    /// merged term dictionary/field-stats/sub-field columns it builds
    /// before spilling) to `O(cap)` instead of `O(corpus)` at the
    /// corpus's last tier — the measured 28M@4g wall (design doc,
    /// "PLANCHER 28M FINAL"). `None` (the default, unset env) preserves
    /// the pre-S3b behaviour exactly (no size ceiling, only the ratio
    /// bound).
    fn find_eligible_merge_run(
        &self,
        fanin: usize,
        max_docs: Option<u64>,
    ) -> Option<(usize, usize)> {
        /// Segments inside an eligible run must stay within this ratio of
        /// the run's smallest `doc_count` — the design doc's "~10×".
        const SIZE_RATIO: u64 = 10;

        let sealed_end = self.segments.len().saturating_sub(1);
        if sealed_end < fanin {
            return None;
        }
        let sizes: Vec<u64> = self.segments[..sealed_end]
            .iter()
            .map(|s| u64::from(s.doc_count))
            .collect();

        let mut best: Option<(usize, usize)> = None;
        for start in 0..sizes.len() {
            let mut end = start + 1;
            let mut run_min = sizes[start];
            let mut run_max = sizes[start];
            let mut run_sum = sizes[start];
            while end < sizes.len() {
                let candidate_min = run_min.min(sizes[end]);
                let candidate_max = run_max.max(sizes[end]);
                if candidate_max > candidate_min.saturating_mul(SIZE_RATIO) {
                    break;
                }
                let candidate_sum = run_sum.saturating_add(sizes[end]);
                if let Some(cap) = max_docs {
                    if candidate_sum > cap {
                        break;
                    }
                }
                run_min = candidate_min;
                run_max = candidate_max;
                run_sum = candidate_sum;
                end += 1;
            }
            let len = end - start;
            if len >= fanin {
                let is_longer = match best {
                    None => true,
                    Some((best_start, best_end)) => len > best_end - best_start,
                };
                if is_longer {
                    best = Some((start, end));
                }
            }
        }
        best
    }

    /// Plan segments S3: merge sealed segments `self.segments[start..end]`
    /// (an ADJACENT run — `start`/`end` are always produced by
    /// [`Self::find_eligible_merge_run`], never called with an arbitrary
    /// range) into ONE new sealed segment, replacing them in place.
    ///
    /// Doc_ids stay GLOBAL and are never renumbered (Fable's arbitrage):
    /// the merged segment's `doc_base` is the run's first segment's own
    /// `doc_base`, and `doc_count` is the Σ of the run's — the run is
    /// contiguous by construction (segments are only ever pushed with
    /// `doc_base == previous doc_base + previous doc_count`, see
    /// [`Self::start_new_active_segment_if_nonempty`]), so this covers
    /// exactly the same global doc_id range as the run did, with no gaps.
    ///
    /// `terms` streams through [`crate::postings::merge_term_dictionaries`]
    /// (bounded memory: one merged term's postings at a time). The eager
    /// per-doc columns (`field_stats`, `subfield_values`, `live_docs`) are
    /// concatenated with a per-segment LOCAL offset equal to that
    /// segment's `doc_base` minus the merged segment's own `doc_base` —
    /// exactly "ancien local + offset du segment dans le run", since the
    /// run's contiguity makes that offset equal to the cumulative
    /// `doc_count` of the run's earlier segments.
    fn merge_segments_run(&mut self, start: usize, end: usize) {
        let merge_started_at = std::time::Instant::now();
        let run = &self.segments[start..end];
        let merged_doc_base = run[0].doc_base;
        let merged_doc_count: u32 = run.iter().map(|segment| segment.doc_count).sum();

        let disk_enabled = self.resolved_postings_disk_enabled();
        debug_assert!(
            run.iter()
                .all(|segment| segment.terms.disk_backed() == disk_enabled),
            "every segment of a DocumentIndex shares the same postings-disk mode \
             for its whole process lifetime (see `postings_disk_enabled`'s doc comment)"
        );
        let term_dicts: Vec<&TermDictionary> = run.iter().map(|segment| &segment.terms).collect();
        let terms = merge_term_dictionaries(&term_dicts, disk_enabled);
        let field_stats = merge_field_stats(run);
        // Plan segments S3b (design doc §S3b, "spill incrémental des
        // colonnes fusionnées pendant le merge"): unlike the S5c
        // build-then-spill pattern this replaces, `merge_subfield_values`
        // now streams each spilled path directly into the merged segment's
        // OWN file source-by-source (never holding the whole run's merged
        // `codes` resident) and hands back the resulting file directly —
        // no separate `seal_subfield_columns` pass needed for the paths it
        // successfully streamed.
        let (subfield_values, subfield_segment) = merge_subfield_values(run, disk_enabled);
        let live_docs = merge_live_docs(run, merged_doc_base);
        let merged_segment_count = run.len();

        let merged = Segment {
            terms,
            field_stats,
            subfield_values,
            live_docs,
            doc_base: merged_doc_base,
            doc_count: merged_doc_count,
            subfield_segment,
        };
        self.segments
            .splice(start..end, std::iter::once(Arc::new(merged)));

        tracing_log_segment_merge(
            merged_segment_count,
            merged_doc_count,
            merge_started_at.elapsed(),
        );
    }

    /// Plan segments S2: if `postings_builder.memory_bytes()` has reached
    /// the configured flush budget, seal the active segment (materialize
    /// its terms, finalize sub-field interning) and start a fresh active
    /// one — turning what used to be a single ever-growing builder into
    /// real, appended, immutable segments. This is what bounds the
    /// indexation memory pic (the design's diagnostic "maladie A").
    ///
    /// Plan segments S3: also runs the tiered-merge policy right after the
    /// seal ([`Self::maybe_merge_after_seal`]) — a no-op when
    /// `SURCH_MERGE_FANIN=0`/forced off, so this stays the exact S2
    /// behaviour when merging is disabled.
    ///
    /// Call this ONCE PER BULK CHUNK, after merging the chunk's documents
    /// (`surch-api::InMemoryIndex::append_to_index`) — NEVER from the
    /// `rebuild_index()` replay path, which must always reproduce a
    /// mono-segment index (tombstone reclamation lands in S4). No-op
    /// (and therefore free — a single `OnceLock` read) when
    /// `SURCH_FLUSH_BUDGET_BYTES` is unset/0, the S1 reversibility flag.
    pub fn maybe_flush_by_budget(&mut self) {
        let Some(budget) = self.resolved_flush_budget_bytes() else {
            return;
        };
        if self.postings_builder.memory_bytes() < budget {
            return;
        }
        self.materialize_active_segment_terms();
        self.start_new_active_segment_if_nonempty();
        self.maybe_merge_after_seal();
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
        // Plan segments S2: a genuinely NEW doc_id can only ever land in
        // the ACTIVE segment's own range — doc_ids are monotonic and
        // never reused (guaranteed by the caller, see
        // `surch-api::InMemoryIndex::next_doc_id`), so a doc_id BELOW the
        // active segment's `doc_base` necessarily re-uses an id an
        // already-sealed segment's range consumed: rejected as a
        // duplicate (keeps `merge_analyzed`'s `doc_id >= doc_base`
        // invariant a true invariant instead of a reachable panic).
        // Within the active range, the check is the same live-docs probe
        // as S1, just translated to the segment's LOCAL indexing — with
        // one segment `doc_base == 0`, so local == global, bit-identical.
        let active = self.active_segment();
        let active_doc_base = active.doc_base;
        let documents = documents
            .into_iter()
            .map(|(doc_id, fields)| {
                let is_duplicate = match doc_id.checked_sub(active_doc_base) {
                    // Below the active segment's range: an id a sealed
                    // segment already consumed (never reused upstream).
                    None => true,
                    Some(local) => active.live_docs.contains(local),
                };
                if is_duplicate || !seen.insert(doc_id) {
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
            let disk_enabled = self.resolved_postings_disk_enabled();
            let new_terms = self
                .postings_builder
                .clone()
                .build_with_disk_flag(disk_enabled);
            self.segment_mut().terms = new_terms;
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
        let disk_enabled = self.resolved_postings_disk_enabled();
        let new_terms = self
            .postings_builder
            .clone()
            .build_with_disk_flag(disk_enabled);
        self.segment_mut().terms = new_terms;
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

    /// Lot C Phase 0 : fusionne `materialize_terms()` + `finalize_postings()`
    /// pour le chemin `_refresh`, SANS le clone du builder.
    ///
    /// `materialize_terms()` doit rester clone-based (un search entre deux
    /// bulks matérialise le FST mais le builder doit conserver l'historique
    /// pour les writes incrémentaux suivants). Mais au `_refresh` le builder
    /// est droppé juste après : on le sort donc par `mem::replace` et on
    /// build depuis la valeur possédée, économisant une copie pleine du
    /// builder (~la moitié de la RAM d'index) au pic du refresh. C'est le
    /// prérequis anti-OOM pour tourner sous une limite mémoire (Lot C).
    ///
    /// À n'utiliser QUE là où le builder n'est plus nécessaire ensuite (le
    /// cycle de refresh) — sur le chemin search, utiliser `materialize_terms()`.
    ///
    /// Plan segments S2 : ce scellement matérialise TOUJOURS les postings
    /// de l'actif en place (Lot C `C2` : meme mouvement pour le builder
    /// d'interning des sub-fields — voir `SubfieldColumn::finalize` pour
    /// la preuve qu'il est sans danger de le vider ici, write-only,
    /// toujours repeuple depuis zero par le prochain write via
    /// `clear()`). Si le budget de flush est CONFIGURE
    /// (`SURCH_FLUSH_BUDGET_BYTES`), `_refresh` referme EN PLUS la
    /// génération courante comme un segment scellé de plus (même
    /// mécanique que [`Self::maybe_flush_by_budget`]) — "un segment de
    /// plus par refresh". Budget non configuré (flag de réversibilité
    /// S1) : ce scellement reste EXACTEMENT l'ancien comportement,
    /// remplace `segments[0]` en place, pour toujours mono-segment.
    ///
    /// Plan segments S3 : lance ENSUITE la politique de merge tiered
    /// ([`Self::maybe_merge_after_seal`]) — no-op quand
    /// `SURCH_MERGE_FANIN=0`/forcé off, donc ce chemin reste EXACTEMENT le
    /// comportement S2 quand le merge est désactivé.
    pub fn materialize_terms_and_finalize_postings(&mut self) {
        self.materialize_active_segment_terms();
        if self.resolved_flush_budget_bytes().is_some() {
            self.start_new_active_segment_if_nonempty();
            self.maybe_merge_after_seal();
        }
    }

    /// Plan segments S2: `rebuild_index()` (update/delete/`set_mapping`
    /// on the `surch-api` side) always calls this FIRST, then repopulates
    /// from every currently-live doc — it must always reproduce a
    /// mono-segment index (tombstone-aware merge lands in S4), regardless
    /// of how many segments a prior budget flush / `_refresh` had sealed.
    pub fn clear(&mut self) {
        self.postings_builder = PostingsBuilder::new();
        self.prefix_postings.clear();
        // The fresh `TermDictionary::default()` is in sync with the
        // fresh `PostingsBuilder::new()` (both empty), so the index is
        // clean as far as the deferred-rebuild contract is concerned.
        // Keep the per-index counter (an `Arc<AtomicU64>`) untouched
        // so cumulative diagnostics across rebuilds remain coherent.
        self.terms_dirty = false;
        self.next_doc_id_hint = 0;
        if self.segments.len() == 1 {
            // S1 fast path (also the ONLY path when budget flush is
            // unset): reset the single sealed segment's contents in
            // place (through `Arc::make_mut`, same no-clone guarantee as
            // every other write path) rather than replacing `segments[0]`
            // wholesale, so `live_docs`' bitmap keeps its allocated
            // capacity across a `rebuild_index()` cycle exactly like
            // before this refactor (`LiveDocsBitset::clear()` does not
            // deallocate). `doc_base` is reset to `0` — bit-identical to
            // S1, where it is implicitly always `0`.
            let segment = self.segment_mut();
            segment.live_docs.clear();
            segment.terms = TermDictionary::default();
            segment.field_stats.clear();
            segment.subfield_values.clear();
            // Plan segments S5c: every column was just cleared, so any
            // spilled file this segment held is now dead weight — drop it
            // (removes the tempfile via `Drop`) rather than leaking it
            // until the segment itself is replaced.
            segment.subfield_segment = None;
            segment.doc_base = 0;
            segment.doc_count = 0;
        } else {
            // S2: coming back from a genuinely multi-segment state (a
            // budget flush had fired before this rebuild). Collapse to a
            // single fresh segment — `rebuild_index()` always reproduces
            // a mono-segment index. Loses the `live_docs` capacity-reuse
            // micro-optimisation above, which only matters on the
            // already-rare update/delete/set_mapping path, not the bulk
            // hot path this feature targets.
            self.segments = vec![Arc::new(Segment::default())];
        }
    }

    /// Plan segments S2: GLOBAL live doc_ids across every sealed segment.
    /// `live_docs` is indexed LOCALLY per segment (see [`Segment`]'s doc),
    /// so each segment's local ids are offset back by its own `doc_base`
    /// before being collected — a no-op offset (`+ 0`) when there is only
    /// one segment, i.e. bit-identical to S1.
    pub fn doc_ids(&self) -> Vec<u32> {
        self.segments
            .iter()
            .flat_map(|segment| {
                let doc_base = segment.doc_base;
                segment.live_docs.iter().map(move |local| local + doc_base)
            })
            .collect()
    }

    /// Stored field retrieval is the caller's responsibility (sources live
    /// in `surch-api::AppState`); this method only returns the previously
    /// indexed analyzed fields when a stored-fields writer has been wired
    /// in, which is not the in-memory path. Always returns `None` for the
    /// current `DocumentIndex` layout.
    pub fn stored_document(&self, _doc_id: u32) -> Option<&StoredDocument> {
        None
    }

    /// Single-segment passthrough (`segments[0]`). Enumerating terms
    /// across N real segments would need a genuine k-way-merged
    /// `TermsEnum` (streaming FST merge, S3's tiered-merge territory) —
    /// out of scope here: this method is only ever used by this crate's
    /// own single-segment tests/admin tooling (grep-audited), never by
    /// any `surch-api` read path, so it is left untouched.
    pub fn terms(&self, field: &str) -> TermsEnum {
        self.segment().terms.terms(field)
    }

    /// Single-segment passthrough (`segments[0]`). Plan segments S2: every
    /// `surch-api::state` call site is gated behind
    /// [`Self::postings_disk_backed`] returning `false`, which (per that
    /// method's doc) is only possible when `segment_count() == 1` — so
    /// this is never reached in a genuinely multi-segment index; left
    /// unchanged (no extra allocation on the RAM hot path).
    pub fn postings(&self, field: &str, term: &str) -> Option<PostingsEnum<'_>> {
        self.segment().terms.postings(field, term)
    }

    /// Returns the pre-computed per-block stats for `(field, term)`,
    /// aligned with [`postings`] chunks of 128 entries. See
    /// [`crate::postings::BlockMeta`] for the schema. Single-segment
    /// passthrough, see [`Self::postings`]'s doc for why this is safe
    /// unchanged under S2.
    pub fn block_metas(&self, field: &str, term: &str) -> Option<&[BlockMeta]> {
        self.segment().terms.block_metas(field, term)
    }

    /// Runtime view that ties a term's postings to its FoR-aligned block
    /// metadata in a single lookup. The search scoring path prefers this
    /// over separate [`postings`]/[`block_metas`] calls so it can borrow
    /// both zero-copy from the live term dictionary. Single-segment
    /// passthrough, see [`Self::postings`]'s doc for why this is safe
    /// unchanged under S2 (every call site is gated behind
    /// `!postings_disk_backed()`, itself gated on `segment_count() == 1`).
    pub fn postings_with_block_metas(&self, field: &str, term: &str) -> Option<PostingsList<'_>> {
        self.segment().terms.postings_with_block_metas(field, term)
    }

    /// Lot C `C1b` sous-pas 2: whether THIS index's currently-built
    /// `TermDictionary` is disk-backed (`doc_ids_flat`/`freqs_flat`
    /// empty, the segment + persisted block directory are the sole
    /// source of truth). Read-path callers (`surch-api::state`) branch
    /// on this — not on the process-wide
    /// [`crate::postings::postings_disk_enabled`] flag — so a query
    /// always agrees with what the dictionary it is about to read
    /// actually contains.
    ///
    /// Plan segments S2: `true` whenever there is more than one segment,
    /// REGARDLESS of any individual segment's own RAM/disk layout — this
    /// is the single switch every `surch-api::state` call site already
    /// branches on to pick its "owned, correct-first" fallback
    /// ([`Self::decode_from_segment`]/`disk_cursor`-based) over the
    /// zero-copy RAM path, so reusing it also routes multi-segment
    /// queries through that same owned/merged fallback with ZERO changes
    /// to those call sites — see the design's S2 read-path note. With
    /// exactly one segment (the S1 case, forever true while
    /// `SURCH_FLUSH_BUDGET_BYTES` is unset) this reduces to that
    /// segment's own flag, bit-identical to before this refactor.
    pub fn postings_disk_backed(&self) -> bool {
        self.segments.len() > 1 || self.segment().terms.disk_backed()
    }

    /// Lot C `C1b` sous-pas 2: block-addressed disk cursor over
    /// `(field, term)`'s postings — the production read path for the
    /// conjunction/leapfrog functions in `surch-api::state` when
    /// [`Self::postings_disk_backed`] is `true`. See
    /// [`crate::postings::TermDictionary::disk_cursor`].
    ///
    /// Reste le wrapper historique mono-segment, volontairement lossy. P2
    /// construit sa vue multi-segment avec [`Self::segmented_postings_checked`]
    /// afin de préserver la distinction absence/erreur dans chaque segment.
    pub fn disk_cursor(&self, field: &str, term: &str) -> Option<DiskPostingsCursor<'_>> {
        self.segment().terms.disk_cursor(field, term)
    }

    /// Variante vérifiée du curseur mono-segment. Les appelants historiques
    /// conservent [`Self::disk_cursor`] et son `Option` volontairement lossy.
    pub fn disk_cursor_checked(
        &self,
        field: &str,
        term: &str,
    ) -> CheckedPostings<DiskPostingsCursor<'_>> {
        self.segment().terms.disk_cursor_checked(field, term)
    }

    /// Vue checked d'un terme sur tous les segments, destinée aux
    /// conjonctions P2. Une absence réelle ne vide que la case du segment
    /// concerné ; une erreur de lecture ou de métadonnée interrompt la
    /// construction entière afin que l'appelant puisse revenir au chemin de
    /// référence sans publier un résultat partiel.
    pub fn segmented_postings_checked(
        &self,
        field: &str,
        term: &str,
    ) -> CheckedPostings<SegmentedPostings<'_>> {
        let mut per_segment = Vec::with_capacity(self.segments.len());
        let mut global_df = 0u64;
        let mut any = false;

        for segment in &self.segments {
            let cursor = if segment.terms.disk_backed() {
                segment
                    .terms
                    .disk_cursor_p2_checked(field, term)?
                    .map(SegmentPostingsCursor::Disk)
            } else {
                match segment
                    .terms
                    .postings_with_block_metas_checked(field, term)?
                {
                    Some(list) => {
                        let local_df = list.doc_freq_from_block_metas() as u64;
                        let iter = list
                            .skip_iter()
                            .map_err(|_| PostingsReadError::Corrupt)?
                            .ok_or(PostingsReadError::Corrupt)?;
                        Some(SegmentPostingsCursor::Ram {
                            iter,
                            freqs: list.freqs(),
                            local_df,
                        })
                    }
                    None => None,
                }
            };

            if let Some(cursor) = cursor {
                global_df = global_df
                    .checked_add(cursor.local_df())
                    .ok_or(PostingsReadError::Corrupt)?;
                any = true;
                per_segment.push(Some(cursor));
            } else {
                per_segment.push(None);
            }
        }

        Ok(any.then_some(SegmentedPostings {
            per_segment,
            global_df,
        }))
    }

    /// Lot C `C1b` sous-pas 2: decode `(field, term)`'s FULL postings
    /// from the disk segment into owned `Vec`s — the production read
    /// path for the OR-match scoring arena (`SearchScoringContext`,
    /// surch-api) and for candidate-resolution helpers that already
    /// collect into an owned structure (`match_hits_internal`,
    /// `conjunction_of_matches`) when [`Self::postings_disk_backed`] is
    /// `true`. See [`crate::postings::TermDictionary::decode_from_segment`].
    ///
    /// Plan segments S2: with exactly one segment this is the unchanged
    /// S1 passthrough (no extra allocation, pure disk read — the C1b
    /// contract). With more than one, this MERGES every segment's own
    /// postings (concatenated in ascending `doc_base` order — each
    /// segment's postings only ever cover its own contiguous doc_id
    /// range, so the concatenation is already globally doc_id-ascending,
    /// no re-sort needed). Per segment the source is picked by ITS OWN
    /// layout: a RAM-backed segment is read from its resident
    /// `doc_ids_flat`/`freqs_flat` channels (authoritative — cannot have
    /// lost coverage), a disk-backed one from its persisted segment via
    /// `TermDictionary::decode_from_segment`. Deliberately NOT the
    /// SHADOW disk copy for a RAM segment: shadow writes are best-effort
    /// (an I/O failure leaves sentinel `(0, 0)` descriptors without ever
    /// being a correctness problem for the RAM engine), so relying on
    /// them here could silently drop a term's postings. Returns `None`
    /// only when NO segment has any postings for `(field, term)`.
    pub fn decode_from_segment(&self, field: &str, term: &str) -> Option<(Vec<u32>, Vec<u32>)> {
        if self.segments.len() == 1 {
            return self.segment().terms.decode_from_segment(field, term);
        }
        let mut doc_ids = Vec::new();
        let mut freqs = Vec::new();
        let mut any = false;
        for segment in &self.segments {
            if segment.terms.disk_backed() {
                if let Some((ids, fr)) = segment.terms.decode_from_segment(field, term) {
                    any = true;
                    doc_ids.extend(ids);
                    freqs.extend(fr);
                }
            } else if let Some(list) = segment.terms.postings_with_block_metas(field, term) {
                any = true;
                doc_ids.extend_from_slice(list.doc_ids());
                freqs.extend_from_slice(list.freqs());
            }
        }
        any.then_some((doc_ids, freqs))
    }

    /// Variante vérifiée de la fusion des postings. Un terme absent d'un
    /// segment n'ajoute rien ; une lecture disque impossible invalide toute
    /// la fusion, afin qu'un futur chemin rapide ne publie jamais un préfixe.
    pub fn decode_from_segment_checked(
        &self,
        field: &str,
        term: &str,
    ) -> CheckedPostings<(Vec<u32>, Vec<u32>)> {
        if self.segments.len() == 1 {
            return self
                .segment()
                .terms
                .decode_from_segment_checked(field, term);
        }
        let mut doc_ids = Vec::new();
        let mut freqs = Vec::new();
        let mut any = false;
        for segment in &self.segments {
            if segment.terms.disk_backed() {
                if let Some((ids, fr)) = segment.terms.decode_from_segment_checked(field, term)? {
                    any = true;
                    doc_ids.extend(ids);
                    freqs.extend(fr);
                }
            } else if let Some(list) = segment
                .terms
                .postings_with_block_metas_checked(field, term)?
            {
                any = true;
                doc_ids.extend_from_slice(list.doc_ids());
                freqs.extend_from_slice(list.freqs());
            }
        }
        Ok(any.then_some((doc_ids, freqs)))
    }

    /// Lot C `C1b` sous-pas 2: per-index override for the disk-backed
    /// postings flag, bypassing the process-wide
    /// [`crate::postings::postings_disk_enabled`] `OnceLock` — see
    /// `postings_disk_enabled_override`'s field doc. MUST be called
    /// before any document is indexed: it only takes effect at the next
    /// `PostingsBuilder::build_with_disk_flag` call (the next
    /// materialize/`_refresh`), and does not retroactively convert an
    /// already-built `TermDictionary`'s RAM/disk layout.
    pub fn set_postings_disk_enabled(&mut self, enabled: bool) {
        self.postings_disk_enabled_override = Some(enabled);
    }

    /// Lot C `C1b` sous-pas 2: the disk-backed flag value the NEXT
    /// `PostingsBuilder::build_with_disk_flag` call should use — the
    /// per-index override if one was set, otherwise the process-wide
    /// [`crate::postings::postings_disk_enabled`] flag (the historical,
    /// unoverridden default).
    fn resolved_postings_disk_enabled(&self) -> bool {
        self.postings_disk_enabled_override
            .unwrap_or_else(postings_disk_enabled)
    }

    /// `segments[0]`'s own stats. The BM25-facing aggregate (`Σ
    /// doc_count` / `Σ total_terms / Σ doc_count`) lives on
    /// [`Self::field_stats_aggregated`]; this accessor stays for the
    /// zero-copy `doc_len_dense` borrow, valid when the caller has
    /// established `segment_count() == 1` (see
    /// `surch-api::AppState::field_scoring_stats`'s fast path) —
    /// [`Self::field_stats_segments`] is the genuine multi-segment
    /// counterpart.
    pub fn field_stats(&self, field: &str) -> Option<&FieldLengthStats> {
        self.segment().field_stats.get(field)
    }

    /// BM25 stats aggregated GLOBALLY across every sealed segment —
    /// `Σ doc_count`, `Σ total_terms / Σ doc_count` for `avg_doc_len`,
    /// min-of-mins for `min_doc_len`. Non-negotiable for oracle parity
    /// once `segments.len() > 1` (S2+): the design mandates BM25 idf/avg
    /// be computed over the WHOLE corpus, not per segment. With exactly
    /// one segment (S1) every `Σ` reduces to that segment's own value —
    /// same single addition/division operation `FieldLengthStats`'s own
    /// `avg_doc_len()`/`min_doc_len()` used to perform directly, so the
    /// result is bit-for-bit identical to the pre-segment code path.
    /// Returns `None` when no segment has ever recorded stats for
    /// `field` (mirrors the previous `field_stats(field)?` early-return).
    pub fn field_stats_aggregated(&self, field: &str) -> Option<AggregatedFieldStats> {
        let mut doc_count = 0u64;
        let mut total_terms = 0u64;
        let mut min_doc_len = 0u64;
        let mut found = false;
        for segment in &self.segments {
            let Some(stats) = segment.field_stats.get(field) else {
                continue;
            };
            found = true;
            doc_count += stats.doc_count;
            total_terms += stats.total_terms;
            if let Some(segment_min) = stats.min_doc_len() {
                min_doc_len = if min_doc_len == 0 {
                    segment_min
                } else {
                    min_doc_len.min(segment_min)
                };
            }
        }
        found.then_some(AggregatedFieldStats {
            doc_count,
            total_terms,
            min_doc_len,
        })
    }

    /// Plan segments S2: `(doc_base, FieldLengthStats)` pairs for `field`,
    /// one per sealed segment that recorded any stats for it, in
    /// ascending `doc_base` order. Used by
    /// `surch-api::AppState::field_scoring_stats` to resolve
    /// `doc_len(global_doc_id)` through the right segment
    /// (`partition_point` over `doc_base`) once `segment_count() > 1` —
    /// the `segment_count() == 1` fast path there uses
    /// [`Self::field_stats`] directly instead (zero-copy, no `Vec`
    /// allocation), so this is only ever called in the genuinely
    /// multi-segment case.
    pub fn field_stats_segments(&self, field: &str) -> Vec<(u32, &FieldLengthStats)> {
        self.segments
            .iter()
            .filter_map(|segment| {
                segment
                    .field_stats
                    .get(field)
                    .map(|stats| (segment.doc_base, stats))
            })
            .collect()
    }

    /// Returns the in-memory `field -> FieldLengthStats` map for
    /// `segments[0]`. Plan segments S2: kept for the existing
    /// single-segment callers/tests; [`Self::field_stats_maps`] is the
    /// genuine multi-segment counterpart (used by the memory accounting
    /// walker in `crate::memory`, which must reach every segment's own
    /// map, not just one).
    pub fn field_stats_map(&self) -> &BTreeMap<String, FieldLengthStats> {
        &self.segment().field_stats
    }

    /// Plan segments S2: the `field -> FieldLengthStats` map of EVERY
    /// sealed segment, for `crate::memory`'s byte-accounting walker to
    /// sum over the whole index instead of just `segments[0]`. With
    /// exactly one segment this yields the same single map
    /// [`Self::field_stats_map`] does, wrapped in a one-element `Vec`.
    pub fn field_stats_maps(&self) -> Vec<&BTreeMap<String, FieldLengthStats>> {
        self.segments.iter().map(|s| &s.field_stats).collect()
    }

    /// Returns the names of every field that currently has indexed
    /// postings, in lexicographic order. Used by the memory accounting
    /// helper to enumerate every `(field, term)` pair. Plan segments S1:
    /// single-segment passthrough, see [`Self::terms`]'s doc.
    pub fn field_names(&self) -> Vec<String> {
        self.segment().terms.field_names()
    }

    /// #17 memory accounting: total FST byte size across fields, summed
    /// over every sealed segment (Σ — with one segment, S1, this is that
    /// segment's own byte count, unchanged from before this refactor).
    pub fn fst_bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.terms.fst_bytes()).sum()
    }

    /// Lot C Phase 1 memory accounting: real bytes held by the flat
    /// postings buffers (term strings + `doc_ids_flat` + `freqs_flat`,
    /// summed over fields AND over every sealed segment). See
    /// [`crate::postings::TermDictionary::postings_bytes`].
    pub fn postings_bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.terms.postings_bytes()).sum()
    }

    /// #17 memory accounting: total bytes held by precomputed roaring
    /// bitmaps, summed over every sealed segment.
    pub fn roaring_bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.terms.roaring_bytes()).sum()
    }

    /// #17 memory accounting: per-term `Vec<BlockMeta>` capacity bytes,
    /// summed over every sealed segment.
    pub fn block_metas_bytes(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| s.terms.block_metas_bytes())
            .sum()
    }

    /// Plan segments S5 (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §"Dimensionnement S5"): RESIDENT per-term CSR/directory metadata
    /// bytes (`offsets`, `block_offsets`, `segment_descriptors`,
    /// `block_directory`, `block_dir_offsets`) that no other gauge counted
    /// before this — see
    /// [`crate::postings::TermDictionary::postings_directory_bytes`] for
    /// la justification complète. S5b déverse les cinq tableaux de service
    /// sous `SURCH_POSTINGS_DISK`, mais P2 conserve une attestation exacte
    /// des répertoires ; cette jauge la compte sur tous les segments scellés.
    pub fn postings_directory_bytes(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| s.terms.postings_directory_bytes())
            .sum()
    }

    /// #17c memory accounting: Vec capacity slack across every term's
    /// `Vec<Posting>` and `Vec<u32>` channels. Surfaces the bytes
    /// allocated-but-unused after the FST build — typically up to ~50 %
    /// of the live `postings_bytes` because of `Vec`'s geometric growth
    /// (~doubling) leaving the last realloc half-filled. Summed over
    /// every sealed segment.
    pub fn postings_capacity_slack_bytes(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| s.terms.postings_capacity_slack_bytes())
            .sum()
    }

    /// Lot C `C1a-batché`: bytes physically written to the SHADOW disk
    /// postings segment (`surch_index_disk_postings_bytes`), summed over
    /// every sealed segment. See
    /// [`crate::postings::TermDictionary::postings_segment_bytes`] — this
    /// is a raw disk-footprint measurement, deliberately NOT part of
    /// [`crate::memory::MemoryUsage`] (the segment is SHADOW: the same
    /// bytes are, today, ALSO fully resident via `postings_bytes`, so
    /// adding this in would double-count against the RAM total).
    pub fn postings_segment_bytes(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| s.terms.postings_segment_bytes())
            .sum()
    }

    /// Lot C `C1a-batché` hardening: number of terms with no disk
    /// coverage because their FoR encode failed at build time
    /// (`surch_index_disk_postings_skipped_terms`), summed over every
    /// sealed segment. See
    /// [`crate::postings::TermDictionary::postings_segment_skipped_terms`]
    /// for the diagnostic pairing with [`Self::postings_segment_bytes`].
    pub fn postings_segment_skipped_terms(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| s.terms.postings_segment_skipped_terms())
            .sum()
    }

    /// Plan segments S5c: raw disk-footprint of the sub-field spill
    /// segments (`surch_index_disk_subfield_values_bytes`), summed over
    /// every sealed segment — mirrors [`Self::postings_segment_bytes`]'s
    /// pairing with `subfield_values_bytes`: deliberately NOT part of
    /// [`crate::memory::MemoryUsage`] (these are bytes ON DISK/page-cache,
    /// the RAM the same values used to occupy is exactly what
    /// `subfield_values_bytes` stops counting once spilled — summing both
    /// would double-count nothing, but they measure different axes: RAM
    /// vs disk footprint).
    pub fn subfield_segment_bytes(&self) -> u64 {
        self.segments
            .iter()
            .filter_map(|s| s.subfield_segment.as_ref())
            .map(|store| store.bytes_written())
            .sum()
    }

    /// #17c memory accounting: taille on-heap du `PostingsBuilder` retenu.
    /// Lot 1.5 garde le builder live entre rebuilds incrémentaux, donc
    /// pour 1.36 M docs ça peut peser GROS et n'était pas compté ailleurs.
    /// Suspect #1 du gap heap ~4 GiB sur deces (cf docs/paper/scoreboard-2026-06-10-mesured.md).
    /// Unaffected by the segments refactor: `postings_builder` is the
    /// active builder, not sealed segment state.
    pub fn postings_builder_bytes(&self) -> u64 {
        self.postings_builder.memory_bytes()
    }

    /// #17c walker complet: real heap bytes held by the `live_docs`
    /// presence bitmap (Lot C Phase 1 lever A), summed over every sealed
    /// segment. One bit per doc_id, resized to the highest doc_id seen —
    /// replaces the previous `BTreeSet<u32>` lazy approximation (~32
    /// B/entry incl. node overhead, ~43 MiB on the deces 1.36 M corpus)
    /// with an exact `bits.capacity()` read (~170 KiB on the same
    /// corpus).
    pub fn live_docs_bytes(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| s.live_docs.memory_bytes())
            .sum()
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
    /// Plan segments S1: genuine union over every sealed segment's own
    /// `BTreeSet<u32>` — with one segment (S1) this collects exactly the
    /// same set the previous direct-field call returned (a `BTreeSet`
    /// built from one source is identical to that source).
    pub fn term_prefix_doc_ids(&self, field: &str, prefix: &str) -> BTreeSet<u32> {
        self.segments
            .iter()
            .flat_map(|segment| segment.terms.prefix_doc_ids(field, prefix))
            .collect()
    }

    /// Plan segments S1: `Σ` over every sealed segment's live-doc count
    /// — with one segment this is that segment's own `count()`, O(1),
    /// unchanged from before this refactor.
    pub fn live_doc_count(&self) -> usize {
        self.segments.iter().map(|s| s.live_docs.count()).sum()
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
    ///
    /// Plan segments S1: `subfield_values`/`field_stats`/`live_docs` now
    /// live on the single active [`Segment`] (mutated in place via
    /// [`Self::segment_mut`]); `prefix_postings`/`postings_builder` stay
    /// direct `DocumentIndex` fields, untouched. The two `segment_mut()`
    /// calls below are scoped to their own block so each mutable borrow
    /// ends before the next `self.prefix_postings`/`self.postings_builder`
    /// statement — same field-mutation order as before this refactor, so
    /// the merged state is identical.
    ///
    /// Plan segments S2: `postings_builder`/`prefix_postings` keep the
    /// GLOBAL `doc_id` unchanged (postings never need a remap — see the
    /// design note on [`Segment`]). The active segment's own eager
    /// columns (`subfield_values`, `field_stats`, `live_docs`) are
    /// indexed by the LOCAL id (`doc_id - active.doc_base`) instead —
    /// with budget flush unset the active segment's `doc_base` is always
    /// `0`, so `local_doc_id == doc_id` and every write below is
    /// byte-for-byte the S1 behaviour.
    fn merge_analyzed(&mut self, document: AnalyzedDocument) -> Result<()> {
        let doc_id = document.doc_id;
        let local_doc_id = doc_id.checked_sub(self.active_segment().doc_base).expect(
            "doc_id must be >= the active segment's doc_base (monotonic, \
                 non-reused doc_id invariant)",
        );
        {
            let segment = self.segment_mut();
            for (path, stored) in document.subfield_values {
                segment
                    .subfield_values
                    .entry(path)
                    .or_default()
                    .set(local_doc_id, stored);
            }
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
        {
            let segment = self.segment_mut();
            for (field, doc_len, norms_enabled) in document.field_lengths {
                segment
                    .field_stats
                    .entry(field)
                    .or_default()
                    .record_doc_len(local_doc_id, doc_len, norms_enabled);
            }
            segment.live_docs.insert(local_doc_id);
        }
        self.next_doc_id_hint = self.next_doc_id_hint.max(doc_id.saturating_add(1));
        Ok(())
    }

    /// A10 (Phase 4): stored projected value for `(field_path, doc_id)`.
    ///
    /// Returns `Some(value)` when `field_path` is a declared multi-field
    /// sub-field (`parent.sub`) that was fanned out at write time for this
    /// doc, with the sub-field's analyzer/normalizer already applied. The
    /// query side uses this for `sort`/`agg`/`composite` on `.raw`/`.norm`
    /// without re-normalizing the parent's `_source` on read. Returns `None`
    /// for top-level fields and for docs missing the sub-field value.
    ///
    /// Plan segments S2: `doc_id` is a GLOBAL id; `SubfieldColumn` is
    /// indexed LOCALLY per segment (see [`Segment`]'s doc), so this
    /// resolves the owning segment via `partition_point` over `doc_base`
    /// (segments are always kept in ascending `doc_base` order) before
    /// indexing locally. With exactly one segment `doc_base` is always
    /// `0`, so the fast path below is the exact pre-S2 global lookup.
    ///
    /// Plan segments S5c: this is NOT a hot production path (grep-audited:
    /// only this crate's own tests call it; the real sort/agg reader is
    /// [`Self::subfield_projection`]), so it is allowed to pay a
    /// materialize-then-copy cost on a spilled column — hence `Option<String>`
    /// rather than the previous zero-copy `Option<&str>` (a borrow into a
    /// column that may now be a locally-rebuilt temporary cannot outlive
    /// this call).
    pub fn subfield_value(&self, field_path: &str, doc_id: u32) -> Option<String> {
        if self.segments.len() == 1 {
            let segment = self.segment();
            let column = segment.subfield_values.get(field_path)?;
            return column
                .materialized(segment.subfield_segment.as_deref())
                .get(doc_id)
                .map(str::to_string);
        }
        let idx = self.segments.partition_point(|s| s.doc_base <= doc_id);
        if idx == 0 {
            return None;
        }
        let segment = &self.segments[idx - 1];
        let local = doc_id - segment.doc_base;
        let column = segment.subfield_values.get(field_path)?;
        column
            .materialized(segment.subfield_segment.as_deref())
            .get(local)
            .map(str::to_string)
    }

    /// A10 (Phase 4): whether `field_path` carries write-time fanned-out
    /// sub-field projections. Used by the query side to choose between the
    /// stored sub-field and the legacy `lookup_sort_value` parent alias.
    /// Plan segments S1: true iff ANY segment carries the path (with one
    /// segment, identical to the previous single-map lookup).
    pub fn has_subfield_values(&self, field_path: &str) -> bool {
        self.segments
            .iter()
            .any(|segment| segment.subfield_values.contains_key(field_path))
    }

    /// A10 (Phase 4): the per-doc stored sub-field projection map of
    /// `segments[0]`. Plan segments S2: kept for the existing
    /// single-segment callers/tests; [`Self::subfield_values_maps`] is the
    /// genuine multi-segment counterpart.
    pub fn subfield_values_map(&self) -> &BTreeMap<String, SubfieldColumn> {
        &self.segment().subfield_values
    }

    /// Plan segments S2: `(doc_base, subfield_values)` pairs across every
    /// sealed segment, in ascending `doc_base` order. Used by the memory
    /// accounting walker (`crate::memory::subfield_values_bytes`, which
    /// only needs the byte totals, `doc_base` unused there) and by
    /// `surch-api::AppState::subfield_projection` (sort/agg on a
    /// `.raw`/`.norm` sub-field), which DOES need `doc_base`:
    /// `SubfieldColumn::iter()`'s yielded `doc_id`s are LOCAL to their
    /// owning segment (see [`Segment`]'s doc), so a caller resolving a
    /// GLOBAL doc_id (e.g. `uid_for_doc_id`) must add the segment's own
    /// `doc_base` back. With exactly one segment this yields the same
    /// single map [`Self::subfield_values_map`] does (with `doc_base ==
    /// 0`), wrapped in a one-element `Vec`.
    pub fn subfield_values_maps(&self) -> Vec<(u32, &BTreeMap<String, SubfieldColumn>)> {
        self.segments
            .iter()
            .map(|s| (s.doc_base, &s.subfield_values))
            .collect()
    }

    /// Plan segments S5c (`docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §S5c): GLOBAL `(doc_id, value)` pairs for `field_path` across every
    /// sealed segment that declares it, transparently materializing any
    /// disk-spilled [`SubfieldColumn`] (one bulk `pread` per source
    /// segment via [`SubfieldColumn::materialized`], never one per doc —
    /// matches the full-column-scan access pattern the design doc's §S5c
    /// study found for `AppState::subfield_projection`'s sort/agg reader,
    /// the sole production caller of this method). Returns `None` iff no
    /// segment ever declared the path (mirrors [`Self::has_subfield_values`]).
    ///
    /// Replaces the historical pattern of the caller walking
    /// [`Self::subfield_values_maps`] and calling `SubfieldColumn::iter()`
    /// itself: that pattern cannot survive a spilled column (it has no way
    /// to hand the owning segment's `subfield_segment` file handle back to
    /// `iter()`), so the whole "resolve segment, materialize, offset
    /// `doc_base`" plumbing now lives here, next to the segment/disk
    /// internals it depends on.
    pub fn subfield_projection(&self, field_path: &str) -> Option<BTreeMap<u32, String>> {
        let mut found = false;
        let mut projection = BTreeMap::new();
        for segment in &self.segments {
            let Some(column) = segment.subfield_values.get(field_path) else {
                continue;
            };
            found = true;
            let materialized = column.materialized(segment.subfield_segment.as_deref());
            for (local_doc_id, value) in materialized.iter() {
                projection.insert(segment.doc_base + local_doc_id, value.to_owned());
            }
        }
        found.then_some(projection)
    }
}

/// Plan segments S3: merge `run`'s per-field [`FieldLengthStats`] into one
/// map, indexed LOCALLY to the merged segment (the run-relative offset —
/// each source segment's own `doc_count` bytes, in run order, see
/// [`merge_live_docs`] for the same offset applied to `live_docs`). For
/// each field present in at least one of `run`'s
/// segments, `doc_count`/`total_terms` are summed directly from each
/// source's own EXACT (non-quantized) fields — never re-derived from
/// `doc_len_dense` (the design doc's explicit warning: that channel is
/// Lucene-`SmallFloat`-quantized, re-deriving `total_terms` from it would
/// drift `avg_doc_len` and break oracle parity) — exactly the same Σ
/// [`DocumentIndex::field_stats_aggregated`] already performs across
/// segments for BM25 stats today. `doc_len_dense` itself is a genuine
/// per-doc concat: each source segment's own dense byte slice, padded
/// with the `0` ("absent") sentinel up to that segment's `doc_count` when
/// short (a field can stop being written before every doc in the source
/// segment, or be entirely absent from a source), so segment `i`'s bytes
/// always land at the SAME local offset the run's doc_id offset uses for
/// `live_docs`/`subfield_values` below.
fn merge_field_stats(run: &[Arc<Segment>]) -> BTreeMap<String, FieldLengthStats> {
    let mut field_names: BTreeSet<&str> = BTreeSet::new();
    for segment in run {
        field_names.extend(segment.field_stats.keys().map(String::as_str));
    }

    let mut merged = BTreeMap::new();
    for field in field_names {
        let mut doc_len_dense: Vec<u8> = Vec::new();
        let mut doc_count = 0u64;
        let mut total_terms = 0u64;
        let mut min_doc_len = 0u64;
        for segment in run {
            let segment_len = segment.doc_count as usize;
            match segment.field_stats.get(field) {
                Some(stats) => {
                    doc_count += stats.doc_count;
                    total_terms += stats.total_terms;
                    if let Some(segment_min) = stats.min_doc_len() {
                        min_doc_len = if min_doc_len == 0 {
                            segment_min
                        } else {
                            min_doc_len.min(segment_min)
                        };
                    }
                    let bytes = stats.doc_len_dense();
                    debug_assert!(
                        bytes.len() <= segment_len,
                        "a sealed segment's doc_len_dense never exceeds its doc_count \
                         (resize-on-write is bounded by the local ids merge_analyzed wrote)"
                    );
                    doc_len_dense.extend_from_slice(bytes);
                    if bytes.len() < segment_len {
                        doc_len_dense.resize(doc_len_dense.len() + (segment_len - bytes.len()), 0);
                    }
                }
                None => {
                    doc_len_dense.resize(doc_len_dense.len() + segment_len, 0);
                }
            }
        }
        merged.insert(
            field.to_string(),
            FieldLengthStats {
                doc_count,
                total_terms,
                doc_len_dense,
                min_doc_len,
            },
        );
    }
    merged
}

/// Plan segments S3b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
/// §S3b, "spill incrémental des colonnes fusionnées pendant le merge"):
/// merge `run`'s per-path [`SubfieldColumn`]s into one map, indexed
/// LOCALLY to the merged segment, and (best-effort, when `disk_enabled`)
/// spill every path DIRECTLY into a fresh per-merge [`SubfieldSegment`]
/// file — returned alongside the map for the caller
/// ([`DocumentIndex::merge_segments_run`]) to stash on the merged
/// `Segment` — instead of the pre-S3b two-step "build the whole merged
/// column resident, THEN spill it" (S5c's `seal_subfield_columns`, called
/// after this function used to return a fully resident map).
///
/// The pre-S3b resident build was the diagnosed transient at the corpus's
/// LAST merge tier (design doc, "PLANCHER 28M FINAL"): a run's `Σ
/// doc_count` codes, for EVERY path, held resident simultaneously (all
/// paths are inserted into `merged` before any of them gets spilled) —
/// ~28M × 4 B × (number of sub-field columns). S3b eliminates the "many
/// full columns resident at once" half of that by streaming each path
/// through [`merge_subfield_column_streamed`] (dict pass, then
/// source-by-source codes pass, `append()`ed directly — never a resident
/// `Σ doc_count`-sized `codes` array) — a successfully streamed column's
/// resulting [`SubfieldColumn`] is already spilled (`dict`/`codes` empty)
/// the moment it lands in `merged`. `disk_enabled == false` (or a failed
/// [`SubfieldSegment::try_new`], or a per-PATH streaming failure — a
/// source unreadable, an `append` I/O error) falls back to
/// [`merge_subfield_column_resident`] (the historical resident
/// union-dict/remap-codes body) for that one path, then gets one more
/// best-effort [`SubfieldColumn::spill`] attempt via the SAME output file
/// (mirrors [`Segment::seal_subfield_columns`]'s per-column loop) — so a
/// rare failure degrades to "this one path pays the old O(run docs)
/// transient", never to data loss or a panic.
fn merge_subfield_values(
    run: &[Arc<Segment>],
    disk_enabled: bool,
) -> (
    BTreeMap<String, SubfieldColumn>,
    Option<Arc<SubfieldSegment>>,
) {
    let mut paths: BTreeSet<&str> = BTreeSet::new();
    for segment in run {
        paths.extend(segment.subfield_values.keys().map(String::as_str));
    }

    let mut store = if disk_enabled {
        SubfieldSegment::try_new()
    } else {
        None
    };

    let mut merged = BTreeMap::new();
    for path in paths {
        let streamed = store
            .as_mut()
            .and_then(|store| merge_subfield_column_streamed(run, path, store));
        let mut column = match streamed {
            Some(column) => column,
            None => merge_subfield_column_resident(run, path),
        };
        column.finalize();
        merged.insert(path.to_string(), column);
    }

    // Best-effort catch-up: any path that fell back to the resident
    // merge above (streaming failed, or was never attempted for that
    // path) still gets ONE spill attempt via the SAME shared file — the
    // exact per-column loop `Segment::seal_subfield_columns` runs for a
    // freshly-sealed (non-merge) segment.
    let mut any_spilled = false;
    if let Some(store) = store.as_mut() {
        for column in merged.values_mut() {
            if column.disk.is_some() || (!column.dict.is_empty() && column.spill(store)) {
                any_spilled = true;
            }
        }
    }

    let subfield_segment = if any_spilled {
        store.map(Arc::new)
    } else {
        None
    };
    (merged, subfield_segment)
}

/// Plan segments S3b: the historical (pre-S3b) resident merge body for
/// ONE sub-field path — "union des dicts + remap des codes": `set()`
/// re-interns each value into the NEW column's own `dict`, so a value
/// shared by several source segments' dictionaries collapses to ONE code
/// in the merged column instead of concatenating raw code integers (which
/// would be wrong the moment two source segments assigned the same code
/// to two DIFFERENT values). Kept as the fallback [`merge_subfield_values`]
/// uses when the streamed path ([`merge_subfield_column_streamed`]) is
/// unavailable (`disk_enabled == false`) or fails for this one path.
///
/// A SOURCE segment's own column may already be disk-spilled — `pread`s
/// go through [`SubfieldColumn::materialized`], called ONCE PER SOURCE
/// SEGMENT (a bulk read, not one per doc) before the per-doc loop below
/// reads it, so this never pays one `pread` per value even when merging
/// already-spilled sources.
fn merge_subfield_column_resident(run: &[Arc<Segment>], path: &str) -> SubfieldColumn {
    let mut column = SubfieldColumn::default();
    let mut run_offset: u32 = 0;
    for segment in run {
        if let Some(source) = segment.subfield_values.get(path) {
            let materialized = source.materialized(segment.subfield_segment.as_deref());
            for local in 0..segment.doc_count {
                if let Some(value) = materialized.get(local) {
                    column.set(run_offset + local, value.to_string());
                }
            }
        }
        run_offset += segment.doc_count;
    }
    column
}

/// Plan segments S3b: best-effort STREAMED merge+spill of ONE sub-field
/// `path`'s column across `run`, appending DIRECTLY into `store` — never
/// materializes the merged column's `codes` (the run's `Σ doc_count`, the
/// diagnosed transient) as one resident array.
///
/// Two passes, matching the on-disk layout's ordering constraint (the
/// dict header must be fully known — and written — before the codes
/// region that follows it):
///
/// 1. **Dict pass**: for each source (in run order), fetch just its OWN
///    dict via [`SubfieldColumn::dict_only`] (2 grouped `pread`s when
///    spilled — skips the, potentially large, codes region entirely) and
///    re-intern every value into a fresh accumulator column, remembering
///    the resulting `old_code -> merged_code` remap table (bounded by
///    that ONE source's vocabulary, never by doc count). The merged dict
///    (bounded by total DISTINCT values, not doc count — see
///    [`SubfieldColumn`]'s struct doc) is then written ONCE via
///    [`write_subfield_dict_header`].
/// 2. **Codes pass**: for each source (in the SAME run order), fetch just
///    its OWN codes via [`SubfieldColumn::codes_only`] (1 `pread` when
///    spilled — the dict header is never re-read), translate them through
///    that source's remap table, and [`append_codes_chunk`] the result
///    DIRECTLY to `store` — ONE `append()` per SOURCE SEGMENT (bounded by
///    the run's fan-in, never per document — the C0 lesson,
///    `subfield_segment`'s module doc). `store.append` is a monotonic
///    writer, so consecutive per-source chunks land contiguously with no
///    gaps, exactly matching [`SubfieldColumn::spill`]'s single-blob
///    `codes` region — the reader ([`SubfieldColumn::read_spilled`]) does
///    one `pread` over the whole span and cannot tell it was written in
///    several calls.
///
/// Returns `None` on ANY failure (a source's dict/codes unreadable, or an
/// `append` I/O error) — [`merge_subfield_values`] then falls back to
/// [`merge_subfield_column_resident`] for this path only. Bytes may
/// already have been appended to `store` before a failure — wasted disk
/// space, never referenced by any descriptor, same best-effort contract
/// as `postings::persist_or_keep_term_directory`. Also returns `None`
/// (not a failure) when the path has no values in `run` at all — matches
/// [`SubfieldColumn::spill`]'s "nothing to gain" no-op guard.
fn merge_subfield_column_streamed(
    run: &[Arc<Segment>],
    path: &str,
    store: &mut subfield_segment::SubfieldSegment,
) -> Option<SubfieldColumn> {
    // Pass 1: union dict + per-source remap tables.
    let mut dict_column = SubfieldColumn::default();
    let mut remaps: Vec<Option<Vec<u32>>> = Vec::with_capacity(run.len());
    for segment in run {
        match segment.subfield_values.get(path) {
            Some(source) => {
                let dict = source.dict_only(segment.subfield_segment.as_deref())?;
                let mut remap = Vec::with_capacity(dict.len());
                for value in dict {
                    remap.push(dict_column.intern(String::from(value)));
                }
                remaps.push(Some(remap));
            }
            None => remaps.push(None),
        }
    }
    if dict_column.dict.is_empty() {
        return None;
    }
    let dict_len = dict_column.dict.len() as u32;
    let (dict_offsets_base, dict_bytes_base, dict_bytes_len) =
        write_subfield_dict_header(&dict_column.dict, store)?;

    // Pass 2: stream each source's own codes, remapped, appended directly.
    let mut codes_base: Option<u64> = None;
    let mut codes_len: u64 = 0;
    for (segment, remap) in run.iter().zip(remaps.iter()) {
        let translated = match remap {
            Some(remap) => {
                let source = segment
                    .subfield_values
                    .get(path)
                    .expect("a remap was only built above when this source's path is present");
                let source_codes = source.codes_only(segment.subfield_segment.as_deref())?;
                translate_source_codes(&source_codes, segment.doc_count, remap)
            }
            None => vec![SubfieldColumn::ABSENT; segment.doc_count as usize],
        };
        let base = append_codes_chunk(&translated, store)?;
        if codes_base.is_none() {
            codes_base = Some(base);
        }
        codes_len += translated.len() as u64;
    }
    let codes_base =
        codes_base.unwrap_or_else(|| dict_bytes_base.saturating_add(u64::from(dict_bytes_len)));
    let codes_len = u32::try_from(codes_len).ok()?;

    Some(SubfieldColumn {
        dict: Vec::new(),
        codes: Vec::new(),
        intern_index: HashMap::new(),
        disk: Some(SubfieldColumnDisk {
            dict_offsets_base,
            dict_len,
            dict_bytes_base,
            dict_bytes_len,
            codes_base,
            codes_len,
        }),
    })
}

/// Plan segments S3b: encode+append just the dict header (CSR offsets +
/// UTF-8 bytes) portion of the S5c on-disk column layout — the STREAMED
/// counterpart of [`SubfieldColumn::spill`]'s combined blob, letting the
/// codes region ([`append_codes_chunk`]) follow afterwards, source by
/// source, without ever holding the merged codes array in RAM. Returns
/// `(dict_offsets_base, dict_bytes_base, dict_bytes_len)` — the
/// [`SubfieldColumnDisk`] fields this call determines — or `None` on the
/// same u32-overflow guard [`SubfieldColumn::spill`] uses, or an `append`
/// I/O failure.
fn write_subfield_dict_header(
    dict: &[Box<str>],
    store: &mut subfield_segment::SubfieldSegment,
) -> Option<(u64, u64, u32)> {
    let max_u32 = u32::MAX as usize;
    let dict_total_bytes: usize = dict.iter().map(|value| value.len()).sum();
    if dict_total_bytes > max_u32 || dict.len() >= max_u32 / 4 {
        return None;
    }
    let mut dict_offsets: Vec<u32> = Vec::with_capacity(dict.len() + 1);
    let mut dict_bytes: Vec<u8> = Vec::new();
    let mut offset = 0u32;
    dict_offsets.push(0);
    for value in dict {
        offset = offset.saturating_add(value.len() as u32);
        dict_offsets.push(offset);
        dict_bytes.extend_from_slice(value.as_bytes());
    }
    let mut blob = Vec::with_capacity(
        dict_offsets
            .len()
            .saturating_mul(4)
            .saturating_add(dict_bytes.len()),
    );
    for entry in &dict_offsets {
        blob.extend_from_slice(&entry.to_le_bytes());
    }
    blob.extend_from_slice(&dict_bytes);
    let base = store.append(&blob)?;
    let dict_offsets_len = (dict_offsets.len() as u64).saturating_mul(4);
    let dict_bytes_base = base.saturating_add(dict_offsets_len);
    Some((base, dict_bytes_base, dict_bytes.len() as u32))
}

/// Plan segments S3b: translate ONE source segment's OWN `codes` (local to
/// that source, length `<= segment.doc_count`, resize-on-write like every
/// other dense per-doc column) through `remap` (`old_code -> merged_code`,
/// built by [`merge_subfield_column_streamed`]'s dict pass), producing
/// exactly `doc_count` entries — positions past `source_codes.len()`, and
/// the [`SubfieldColumn::ABSENT`] sentinel itself, both translate to
/// `ABSENT` (never looked up in `remap`), matching
/// [`merge_subfield_column_resident`]'s "absent stays absent" semantics.
/// An out-of-range code (would only happen if `remap` and `source_codes`
/// disagree on the source's own dict size — never reachable given both
/// are derived from the SAME source column) degrades to `ABSENT` rather
/// than panicking.
fn translate_source_codes(source_codes: &[u32], doc_count: u32, remap: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(doc_count as usize);
    for local in 0..doc_count {
        let code = source_codes
            .get(local as usize)
            .copied()
            .unwrap_or(SubfieldColumn::ABSENT);
        let translated = if code == SubfieldColumn::ABSENT {
            SubfieldColumn::ABSENT
        } else {
            remap
                .get(code as usize)
                .copied()
                .unwrap_or(SubfieldColumn::ABSENT)
        };
        out.push(translated);
    }
    out
}

/// Plan segments S3b: append one source's already-remapped `codes` chunk
/// to `store` as ONE `append()` call (bounded by the run's fan-in — never
/// per document, the C0 lesson) — the STREAMED counterpart of
/// [`SubfieldColumn::spill`]'s codes region. Returns the byte offset the
/// chunk starts at (the FIRST call's return value becomes the merged
/// column's `codes_base`; later calls land contiguously right after,
/// since `store.append` is a monotonic writer).
fn append_codes_chunk(codes: &[u32], store: &mut subfield_segment::SubfieldSegment) -> Option<u64> {
    let max_u32 = u32::MAX as usize;
    if codes.len() > max_u32 / 4 {
        return None;
    }
    let mut blob = Vec::with_capacity(codes.len() * 4);
    for &code in codes {
        blob.extend_from_slice(&code.to_le_bytes());
    }
    store.append(&blob)
}

/// Plan segments S3: merge `run`'s per-segment [`LiveDocsBitset`]s into
/// one, remapping each segment's LOCAL doc_ids to the merged segment's own
/// local indexing — `segment.doc_base - merged_doc_base` is that
/// segment's cumulative offset inside the run (the run is contiguous by
/// construction, see [`DocumentIndex::merge_segments_run`]'s doc comment).
/// S3 carries no tombstones yet, so this is a pure concatenation: no
/// doc_id is ever dropped or deduplicated (a run's segments cover
/// disjoint doc_id ranges by construction).
fn merge_live_docs(run: &[Arc<Segment>], merged_doc_base: u32) -> LiveDocsBitset {
    let mut merged = LiveDocsBitset::default();
    for segment in run {
        let offset = segment.doc_base - merged_doc_base;
        for local in segment.live_docs.iter() {
            merged.insert(local + offset);
        }
    }
    merged
}

/// Plan segments S3 observability: emit one `tracing::debug!` per
/// completed segment merge (run size, resulting doc_count, wall-clock
/// duration) — the design doc's "log debug par merge". Kept as its own
/// tiny function (rather than an inline `tracing::debug!` call at the
/// single call site) so the event's shape is documented once.
fn tracing_log_segment_merge(
    merged_segment_count: usize,
    merged_doc_count: u32,
    elapsed: std::time::Duration,
) {
    tracing::debug!(
        merged_segment_count,
        merged_doc_count,
        elapsed_ms = elapsed.as_secs_f64() * 1000.0,
        "surch_index: segment merge completed"
    );
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
        assert_eq!(
            index.subfield_value("NOM.raw", 1).as_deref(),
            Some("etienne dupre")
        );
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

        assert_eq!(
            index.subfield_value("NOM.exact", 1).as_deref(),
            Some("Dupré Martin")
        );
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

    // ---------------------------------------------------------------------
    // Plan segments S1: `Vec<Arc<Segment>>` structural gate.
    // ---------------------------------------------------------------------

    /// After any build/refresh, `DocumentIndex` must hold EXACTLY one
    /// segment (S1 scope — see `docs/paper/design-segments-pic-borne-2026-07-05.md`),
    /// and the read-path aggregates must equal that single segment's own
    /// values via the identical arithmetic (Σ of one term / a single
    /// division), never a different computation that merely happens to
    /// agree numerically.
    #[test]
    fn single_segment_invariant_and_aggregated_stats_match_the_segment() {
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("name", "dupont martin")], &IndexMapping::default())
            .expect("doc 1");
        index
            .add_document_with_mapping(2, [("name", "dupre")], &IndexMapping::default())
            .expect("doc 2");

        assert_eq!(
            index.segments.len(),
            1,
            "S1: DocumentIndex must always hold exactly one segment"
        );
        let segment = &index.segments[0];

        let field_stats = segment
            .field_stats
            .get("name")
            .expect("segment recorded stats for \"name\"");
        let aggregated = index
            .field_stats_aggregated("name")
            .expect("aggregated stats for \"name\"");

        assert_eq!(aggregated.doc_count, field_stats.doc_count);
        assert_eq!(aggregated.total_terms, field_stats.total_terms);
        assert_eq!(aggregated.avg_doc_len(), field_stats.avg_doc_len());
        assert_eq!(aggregated.min_doc_len(), field_stats.min_doc_len());

        // Every other read-path aggregate agrees with the single sealed
        // segment's own value too (Σ over one term).
        assert_eq!(index.live_doc_count(), segment.live_docs.count());
        assert_eq!(
            index.doc_ids(),
            segment.live_docs.iter().collect::<Vec<_>>()
        );
        assert_eq!(index.fst_bytes(), segment.terms.fst_bytes());
        assert_eq!(index.postings_bytes(), segment.terms.postings_bytes());
        assert_eq!(index.live_docs_bytes(), segment.live_docs.memory_bytes());
    }

    /// `clear()` must reset the single segment's contents (not shrink
    /// `segments` itself) so the S1 invariant holds even across a full
    /// `rebuild_index()` cycle (delete/update on the surch-api side).
    #[test]
    fn clear_keeps_single_segment_invariant_and_empties_it() {
        let mut index = DocumentIndex::new();
        index
            .add_document_with_mapping(1, [("name", "dupont")], &IndexMapping::default())
            .expect("doc 1");
        assert_eq!(index.segments.len(), 1);

        index.clear();

        assert_eq!(
            index.segments.len(),
            1,
            "clear() must not drop below the S1 invariant of exactly one segment"
        );
        assert_eq!(index.live_doc_count(), 0);
        assert!(index.field_stats("name").is_none());
        assert!(index.field_stats_aggregated("name").is_none());
    }

    // ---------------------------------------------------------------------
    // Plan segments S2: budget-triggered flush → real multi-segment.
    // ---------------------------------------------------------------------

    /// Multi-segment fixture: NOM multi-field mapping (so sub-field
    /// columns cross a segment boundary too), a per-doc unique NOM value
    /// and a BODY token (`commun`) shared by every doc (so one posting
    /// list genuinely spans segments). Budget forced to 1 byte (any
    /// non-empty builder crosses it) and the disk-postings override
    /// pinned OFF, so the test is deterministic regardless of the
    /// process's `SURCH_FLUSH_BUDGET_BYTES` / `SURCH_POSTINGS_DISK` env.
    ///
    /// Layout produced: docs 0-1 sealed by an explicit
    /// `maybe_flush_by_budget` (the per-chunk call site), docs 2-3 sealed
    /// by `materialize_terms_and_finalize_postings` (the `_refresh` call
    /// site), plus the fresh empty active segment = 3 segments.
    fn multi_segment_index() -> (DocumentIndex, IndexMapping) {
        multi_segment_index_with_disk(false)
    }

    fn multi_segment_index_with_disk(disk_enabled: bool) -> (DocumentIndex, IndexMapping) {
        let mapping = nom_multi_field_mapping();
        let mut index = DocumentIndex::new();
        index.set_flush_budget_bytes_override(Some(1));
        index.set_postings_disk_enabled(disk_enabled);
        // Plan segments S3: merge pinned OFF so the exact 3-segment layout
        // asserted below stays deterministic under any ambient
        // `SURCH_MERGE_FANIN` (same rationale as the two pins above).
        index.set_merge_fanin_override(0);

        index
            .add_documents_with_mapping_deferred(
                [
                    (0, [("NOM", "Dupont Martin"), ("BODY", "commun")]),
                    (1, [("NOM", "Dupré"), ("BODY", "commun")]),
                ],
                &mapping,
            )
            .expect("docs 0-1");
        index.maybe_flush_by_budget();
        index
            .add_documents_with_mapping_deferred(
                [
                    (2, [("NOM", "Bernard"), ("BODY", "commun")]),
                    (3, [("NOM", "Petit Durand"), ("BODY", "commun")]),
                ],
                &mapping,
            )
            .expect("docs 2-3");
        index.materialize_terms_and_finalize_postings();
        (index, mapping)
    }

    fn collect_segmented_doc_ids(postings: &mut SegmentedPostings<'_>) -> Vec<u32> {
        let mut doc_ids = Vec::new();
        for cursor in postings.per_segment.iter_mut().flatten() {
            let mut target = 0u32;
            loop {
                match cursor.advance_to_with_status(target) {
                    SegmentPostingsAdvance::Found(doc_id) => {
                        doc_ids.push(doc_id);
                        target = doc_id.saturating_add(1);
                    }
                    SegmentPostingsAdvance::Exhausted => break,
                    SegmentPostingsAdvance::Error => {
                        panic!("la fixture P2 ne doit pas avoir d'erreur de lecture")
                    }
                }
            }
        }
        doc_ids
    }

    /// Fixture multi-segment avec un premier posting assez large pour que
    /// retirer sa dernière entrée laisse les slices RAM dans les bornes,
    /// mais désaligne le nombre de métadonnées de blocs.
    fn multi_segment_index_with_large_common_term() -> DocumentIndex {
        multi_segment_index_with_common_term_postings(crate::postings::BLOCK_SIZE as u32 + 1)
    }

    /// Variante volontairement intra-bloc : 130 postings puis une
    /// troncature `130 -> 129` gardent deux métadonnées de bloc. Elle
    /// vérifie donc que la complétude checked ne dépend pas seulement de
    /// l'alignement du nombre de blocs.
    fn multi_segment_index_with_intra_block_common_term() -> DocumentIndex {
        multi_segment_index_with_common_term_postings(crate::postings::BLOCK_SIZE as u32 + 2)
    }

    fn multi_segment_index_with_common_term_postings(first_segment_postings: u32) -> DocumentIndex {
        let mapping = IndexMapping::default();
        let mut index = DocumentIndex::new();
        index.set_flush_budget_bytes_override(Some(1));
        index.set_postings_disk_enabled(false);
        index.set_merge_fanin_override(0);

        let first_segment: Vec<_> = (0..first_segment_postings)
            .map(|doc_id| (doc_id, [("BODY", "commun")]))
            .collect();
        index
            .add_documents_with_mapping_deferred(first_segment, &mapping)
            .expect("premier segment large");
        index.maybe_flush_by_budget();
        index
            .add_documents_with_mapping_deferred(
                [(first_segment_postings, [("BODY", "commun")])],
                &mapping,
            )
            .expect("second segment");
        index.materialize_terms_and_finalize_postings();
        index
    }

    #[test]
    fn budget_flush_seals_real_segments_with_global_reads_intact() {
        let (index, _mapping) = multi_segment_index();

        assert_eq!(
            index.segment_count(),
            3,
            "expected two sealed segments (budget flush + refresh) plus the fresh active one"
        );
        assert_eq!(index.segments[0].doc_base, 0);
        assert_eq!(index.segments[0].doc_count, 2);
        assert_eq!(index.segments[1].doc_base, 2);
        assert_eq!(index.segments[1].doc_count, 2);
        assert_eq!(index.segments[2].doc_base, 4, "fresh active segment");

        // Global doc_id reads across segments: live docs, counts, BM25 Σ.
        assert_eq!(index.doc_ids(), vec![0, 1, 2, 3]);
        assert_eq!(index.live_doc_count(), 4);
        let aggregated = index
            .field_stats_aggregated("NOM")
            .expect("aggregated stats for NOM across segments");
        assert_eq!(aggregated.doc_count, 4);
        // "Dupont Martin"(2) + "Dupré"(1) + "Bernard"(1) + "Petit Durand"(2)
        assert_eq!(aggregated.total_terms, 6);
        assert_eq!(aggregated.avg_doc_len(), Some(1.5));

        // Multi-segment forces the owned/merged read path.
        assert!(index.postings_disk_backed());
        let (doc_ids, freqs) = index
            .decode_from_segment("BODY", "commun")
            .expect("BODY=commun spans every segment");
        assert_eq!(doc_ids, vec![0, 1, 2, 3]);
        assert_eq!(freqs, vec![1, 1, 1, 1]);
        // A term entirely inside the SECOND segment resolves too (its
        // postings keep GLOBAL doc_ids — no remap).
        let (doc_ids, _freqs) = index
            .decode_from_segment("NOM", "bernard")
            .expect("NOM=bernard lives in the second sealed segment");
        assert_eq!(doc_ids, vec![2]);
    }

    #[test]
    fn p2_segmented_postings_checked_couvre_ram_disque_mixte_et_df_global() {
        let (ram, _) = multi_segment_index_with_disk(false);
        let (disk, _) = multi_segment_index_with_disk(true);
        let (mut mixed, _) = multi_segment_index_with_disk(false);
        Arc::make_mut(&mut mixed.segments[1]).terms = disk.segments[1].terms.clone();

        let assert_layout =
            |label: &str, index: &DocumentIndex, expected_first: bool, expected_second: bool| {
                let mut postings = index
                    .segmented_postings_checked("BODY", "commun")
                    .expect("la lecture checked doit réussir")
                    .expect("BODY=commun doit exister dans des segments");
                assert_eq!(postings.global_df(), 4, "[{label}] df global");
                assert_eq!(postings.per_segment.len(), 3, "[{label}] cases alignées");
                assert_eq!(
                    matches!(
                        postings.per_segment[0].as_ref(),
                        Some(SegmentPostingsCursor::Disk(_))
                    ),
                    expected_first,
                    "[{label}] variante du premier segment"
                );
                assert_eq!(
                    matches!(
                        postings.per_segment[1].as_ref(),
                        Some(SegmentPostingsCursor::Disk(_))
                    ),
                    expected_second,
                    "[{label}] variante du second segment"
                );
                assert!(
                    postings.per_segment[2].is_none(),
                    "[{label}] le segment actif vide doit rester une absence locale"
                );
                assert_eq!(
                    collect_segmented_doc_ids(&mut postings),
                    vec![0, 1, 2, 3],
                    "[{label}] les segments doivent se concaténer par doc_id"
                );
            };

        assert_layout("RAM", &ram, false, false);
        assert_layout("disque", &disk, true, true);
        assert_layout("mixte", &mixed, false, true);
    }

    #[test]
    fn p2_segmented_postings_checked_refuse_un_segment_illisible_sans_vue_partielle() {
        let (mut index, _) = multi_segment_index_with_disk(true);
        Arc::make_mut(&mut index.segments[1])
            .terms
            .test_remove_postings_segment();

        assert!(matches!(
            index.segmented_postings_checked("BODY", "commun"),
            Err(crate::postings::PostingsReadError::MissingCoverage)
        ));
    }

    #[test]
    fn checked_multi_segment_propage_un_segment_disque_illisible() {
        let (mut index, _mapping) = multi_segment_index_with_disk(true);
        Arc::make_mut(&mut index.segments[1])
            .terms
            .test_remove_postings_segment();

        assert_eq!(
            index.decode_from_segment_checked("BODY", "commun"),
            Err(crate::postings::PostingsReadError::MissingCoverage),
            "un segment illisible ne doit pas publier les segments lus avant lui"
        );
    }

    #[test]
    fn checked_multi_segment_rejette_un_prefixe_ram_borne() {
        let mut index = multi_segment_index_with_large_common_term();
        assert!(
            Arc::make_mut(&mut index.segments[0])
                .terms
                .test_truncate_ram_term_metadata("BODY", "commun"),
            "la fixture doit contenir un terme RAM dont le préfixe reste borné"
        );

        assert_eq!(
            index.decode_from_segment_checked("BODY", "commun"),
            Err(crate::postings::PostingsReadError::Corrupt),
            "un préfixe RAM borné ne doit pas être publié par la fusion"
        );
    }

    #[test]
    fn checked_multi_segment_rejette_la_troncature_ram_intra_bloc_130_vers_129() {
        let mut index = multi_segment_index_with_intra_block_common_term();
        assert_eq!(
            index.segments[0]
                .terms
                .postings("BODY", "commun")
                .expect("la fixture doit contenir le terme")
                .count(),
            130,
            "le premier segment doit reproduire le cas source 130 -> 129"
        );
        assert!(
            Arc::make_mut(&mut index.segments[0])
                .terms
                .test_truncate_ram_term_metadata("BODY", "commun"),
            "la fixture doit réduire exactement 130 postings RAM à 129"
        );

        assert_eq!(
            index.decode_from_segment_checked("BODY", "commun"),
            Err(crate::postings::PostingsReadError::Corrupt),
            "la lecture checked doit refuser 130 -> 129 même avec deux blocs"
        );
    }

    #[test]
    fn subfields_and_doc_len_resolve_across_segment_boundaries() {
        let (index, _mapping) = multi_segment_index();

        // Sub-field columns are LOCAL per segment; the public API takes a
        // GLOBAL doc_id and must route it through the right segment.
        assert_eq!(
            index.subfield_value("NOM.raw", 0).as_deref(),
            Some("dupont martin")
        );
        assert_eq!(index.subfield_value("NOM.raw", 1).as_deref(), Some("dupre"));
        assert_eq!(
            index.subfield_value("NOM.raw", 2).as_deref(),
            Some("bernard")
        );
        assert_eq!(
            index.subfield_value("NOM.raw", 3).as_deref(),
            Some("petit durand")
        );
        assert_eq!(
            index.subfield_value("NOM.raw", 4),
            None,
            "doc_id 4 was never written (empty active segment)"
        );

        // doc_len is exposed per segment with its doc_base, LOCAL indexing.
        let per_segment = index.field_stats_segments("NOM");
        assert_eq!(per_segment.len(), 2, "two sealed segments recorded NOM");
        let (base0, stats0) = per_segment[0];
        let (base1, stats1) = per_segment[1];
        assert_eq!(base0, 0);
        assert_eq!(base1, 2);
        assert_eq!(stats0.doc_len(0), Some(2), "doc 0: 'Dupont Martin'");
        assert_eq!(stats0.doc_len(1), Some(1), "doc 1: 'Dupré'");
        assert_eq!(stats1.doc_len(0), Some(1), "doc 2 locally 0: 'Bernard'");
        assert_eq!(
            stats1.doc_len(1),
            Some(2),
            "doc 3 locally 1: 'Petit Durand'"
        );
        // The dense arrays are sized to their OWN segment, not the corpus
        // (the design's "maladie B" guard).
        assert!(stats1.doc_len_dense().len() <= 2);
    }

    #[test]
    fn stale_doc_id_below_active_doc_base_is_rejected_as_duplicate() {
        let (mut index, mapping) = multi_segment_index();
        // doc_id 1 belongs to the FIRST sealed segment's range — re-adding
        // it must be the same `DuplicateDocId` error S1 raised, never a
        // panic in `merge_analyzed`'s local-id translation.
        let err = index
            .add_documents_with_mapping_deferred([(1, [("NOM", "Rejoue")])], &mapping)
            .expect_err("re-using a sealed segment's doc_id must fail");
        assert_eq!(err, DocumentIndexError::DuplicateDocId { doc_id: 1 });
    }

    #[test]
    fn clear_collapses_multi_segment_back_to_one() {
        let (mut index, mapping) = multi_segment_index();
        assert!(index.segment_count() > 1);

        index.clear();

        assert_eq!(
            index.segment_count(),
            1,
            "rebuild_index()'s clear() must always reproduce a mono-segment index"
        );
        assert_eq!(index.live_doc_count(), 0);
        assert_eq!(index.doc_ids(), Vec::<u32>::new());
        // The rebuild replay starts over from doc_id 0 — accepted again.
        index
            .add_documents_with_mapping_deferred([(0, [("NOM", "Neuf")])], &mapping)
            .expect("doc_id 0 must be insertable again after clear()");
        assert_eq!(index.doc_ids(), vec![0]);
    }

    #[test]
    fn refresh_seals_the_delta_as_one_more_segment() {
        // Budget too high for any per-chunk flush: only the `_refresh`
        // sealing (`materialize_terms_and_finalize_postings` with a budget
        // CONFIGURED) creates segments.
        let mapping = nom_multi_field_mapping();
        let mut index = DocumentIndex::new();
        index.set_flush_budget_bytes_override(Some(u64::MAX));
        index.set_postings_disk_enabled(false);
        // Plan segments S3: merge pinned OFF — the exact segment counts
        // asserted below must not depend on the ambient `SURCH_MERGE_FANIN`.
        index.set_merge_fanin_override(0);

        index
            .add_documents_with_mapping_deferred([(0, [("NOM", "Dupont")])], &mapping)
            .expect("doc 0");
        index.maybe_flush_by_budget();
        assert_eq!(
            index.segment_count(),
            1,
            "a huge budget must not flush mid-ingestion"
        );
        index.materialize_terms_and_finalize_postings();
        assert_eq!(index.segment_count(), 2, "refresh sealed generation 1");

        index
            .add_documents_with_mapping_deferred([(1, [("NOM", "Dupré")])], &mapping)
            .expect("doc 1");
        index.materialize_terms_and_finalize_postings();
        assert_eq!(index.segment_count(), 3, "refresh sealed generation 2");

        // A refresh with NOTHING written in between must not push a
        // useless empty segment.
        index.materialize_terms_and_finalize_postings();
        assert_eq!(index.segment_count(), 3, "empty delta seals nothing");

        assert_eq!(index.doc_ids(), vec![0, 1]);
        assert_eq!(
            index.subfield_value("NOM.raw", 0).as_deref(),
            Some("dupont")
        );
        assert_eq!(index.subfield_value("NOM.raw", 1).as_deref(), Some("dupre"));
    }

    #[test]
    fn budget_forced_off_stays_mono_segment_regardless_of_env() {
        let mapping = nom_multi_field_mapping();
        let mut index = DocumentIndex::new();
        // `Forced(None)`: the S1 reversibility contract, pinned so this
        // test stays green even under `SURCH_FLUSH_BUDGET_BYTES=... cargo test`.
        index.set_flush_budget_bytes_override(None);
        // Pinned OFF for the same determinism reason (`SURCH_POSTINGS_DISK`),
        // so the `postings_disk_backed()` assertion below is unambiguous.
        index.set_postings_disk_enabled(false);

        index
            .add_documents_with_mapping_deferred(
                [(0, [("NOM", "Dupont")]), (1, [("NOM", "Dupré")])],
                &mapping,
            )
            .expect("docs 0-1");
        index.maybe_flush_by_budget();
        assert_eq!(
            index.segment_count(),
            1,
            "budget off: maybe_flush_by_budget must be a strict no-op"
        );
        index.materialize_terms_and_finalize_postings();

        assert_eq!(
            index.segment_count(),
            1,
            "budget off must keep the historical mono-segment layout forever \
             (refresh replaces segments[0] in place, never appends)"
        );
        assert_eq!(index.segments[0].doc_base, 0);
        assert!(!index.postings_disk_backed());
        assert_eq!(index.doc_ids(), vec![0, 1]);
        assert_eq!(
            index.subfield_value("NOM.raw", 0).as_deref(),
            Some("dupont")
        );
        assert_eq!(index.subfield_value("NOM.raw", 1).as_deref(), Some("dupre"));
    }

    // ---------------------------------------------------------------------
    // Plan segments S3: tiered merge (`docs/paper/design-segments-pic-borne-2026-07-05.md`)
    // ---------------------------------------------------------------------

    /// Deterministic `doc_count`-doc corpus (5 cycling `NOM` values, a
    /// shared `BODY` term so at least one term spans every segment), one
    /// doc PER CHUNK with a forced tiny budget so every chunk seals its
    /// own segment (mirrors `surch-api`'s "call `maybe_flush_by_budget`
    /// once per bulk chunk" contract), then a final `_refresh`-style seal.
    /// `fanin`/`disk_enabled` are forced explicitly — never read from the
    /// process env — so the test is deterministic regardless of
    /// `SURCH_MERGE_FANIN`/`SURCH_POSTINGS_DISK` in the ambient test
    /// process (same rationale as every other override in this module).
    fn build_merge_test_index(
        doc_count: u32,
        fanin: usize,
        disk_enabled: bool,
    ) -> (DocumentIndex, IndexMapping) {
        build_merge_test_index_with_cap(doc_count, fanin, disk_enabled, None)
    }

    /// Same corpus/knobs as [`build_merge_test_index`], plus an explicit
    /// [`DocumentIndex::set_merge_max_docs_override`] — Plan segments S3b
    /// (`docs/paper/design-segments-pic-borne-2026-07-05.md` §S3b, "cap de
    /// taille de tier"). `max_docs: None` reproduces `build_merge_test_index`
    /// exactly (the reversibility default).
    fn build_merge_test_index_with_cap(
        doc_count: u32,
        fanin: usize,
        disk_enabled: bool,
        max_docs: Option<u64>,
    ) -> (DocumentIndex, IndexMapping) {
        let mapping = nom_multi_field_mapping();
        let mut index = DocumentIndex::new();
        index.set_flush_budget_bytes_override(Some(1));
        index.set_merge_fanin_override(fanin);
        index.set_postings_disk_enabled(disk_enabled);
        index.set_merge_max_docs_override(max_docs);

        const NAMES: [&str; 5] = [
            "Dupont Martin",
            "Dupré",
            "Bernard",
            "Petit Durand",
            "Lefèvre Noir",
        ];
        for doc_id in 0..doc_count {
            let name = NAMES[doc_id as usize % NAMES.len()];
            index
                .add_documents_with_mapping_deferred(
                    [(doc_id, [("NOM", name), ("BODY", "commun lorem")])],
                    &mapping,
                )
                .unwrap_or_else(|err| panic!("doc {doc_id} should index: {err:?}"));
            index.maybe_flush_by_budget();
        }
        index.materialize_terms_and_finalize_postings();
        (index, mapping)
    }

    #[test]
    fn merge_fanin_zero_never_merges() {
        // S3 reversibility flag: `SURCH_MERGE_FANIN=0` (forced here via the
        // per-index override) must reproduce EXACTLY the S2 layout — one
        // sealed segment per doc (budget=1 flushes every chunk) plus the
        // fresh active one, unaffected by the merge feature existing at all.
        const DOC_COUNT: u32 = 10;
        let (index, _mapping) = build_merge_test_index(DOC_COUNT, 0, false);
        assert_eq!(
            index.segment_count(),
            DOC_COUNT as usize + 1,
            "fanin=0 must never merge"
        );
    }

    #[test]
    fn merge_cascades_and_matches_unmerged_oracle() {
        const DOC_COUNT: u32 = 40;
        // `merged`: fan-in 3 forces real, cascading merges. `oracle`: the
        // SAME corpus with merging disabled (fanin=0) — a known-correct
        // multi-segment layout (proven by the S2 tests above) used as the
        // ground truth every read below must agree with bit-for-bit.
        let (merged, _mapping) = build_merge_test_index(DOC_COUNT, 3, false);
        let (oracle, _mapping) = build_merge_test_index(DOC_COUNT, 0, false);

        assert_eq!(
            oracle.segment_count(),
            DOC_COUNT as usize + 1,
            "sanity: the un-merged oracle must have one segment per doc"
        );
        assert!(
            merged.segment_count() < oracle.segment_count(),
            "merge (fanin=3) must produce fewer segments than the unmerged \
             oracle: merged={}, oracle={}",
            merged.segment_count(),
            oracle.segment_count()
        );
        assert!(
            merged.segment_count() <= 15,
            "merge should have cascaded well below one segment per doc \
             (LogMergePolicy-style tiering), got {} segments for {DOC_COUNT} docs",
            merged.segment_count()
        );

        // Global doc-id / live-doc reads agree exactly.
        assert_eq!(merged.doc_ids(), oracle.doc_ids());
        assert_eq!(merged.live_doc_count(), oracle.live_doc_count());

        // (b) BM25 stats aggregated per field agree exactly — doc_count,
        // total_terms, min_doc_len (avg_doc_len is derived from the first
        // two, see `AggregatedFieldStats::avg_doc_len`), summed during the
        // merge's field_stats concat, never re-derived from the quantized
        // `doc_len_dense` byte.
        for field in ["NOM", "BODY"] {
            assert_eq!(
                merged.field_stats_aggregated(field),
                oracle.field_stats_aggregated(field),
                "field_stats_aggregated diverged for {field}"
            );
        }

        // Per-term postings (doc_freq + doc_ids + freqs) agree exactly —
        // the proxy for "search results identical" at this crate's level
        // (BM25 scoring itself lives in `surch-api`; see
        // `segment_merge_parity.rs` for the end-to-end `_search` gate).
        for term in ["commun", "lorem"] {
            let sorted_pairs = |index: &DocumentIndex| -> Vec<(u32, u32)> {
                let (ids, freqs) = index
                    .decode_from_segment("BODY", term)
                    .unwrap_or_else(|| panic!("BODY={term} must be present"));
                let mut pairs: Vec<(u32, u32)> = ids.into_iter().zip(freqs).collect();
                pairs.sort_unstable();
                pairs
            };
            assert_eq!(
                sorted_pairs(&merged),
                sorted_pairs(&oracle),
                "BODY={term} postings diverged between merged and oracle"
            );
        }
        for name in [
            "dupont martin",
            "dupre",
            "bernard",
            "petit durand",
            "lefevre noir",
        ] {
            assert_eq!(
                merged.decode_from_segment("NOM.raw", name),
                oracle.decode_from_segment("NOM.raw", name),
                "NOM.raw={name} postings diverged between merged and oracle"
            );
        }

        // (e) sub-field values (dict remap) and doc_len (dense concat with
        // local-offset remap) agree for EVERY doc — proof the merge's
        // per-segment offset arithmetic survived cascading merges.
        // `resolve_doc_len` reproduces `surch-api`'s multi-segment
        // resolution (last segment whose `doc_base <= doc_id`, then LOCAL
        // indexing) — a local index past a segment's own range reads the
        // dense slice out of bounds and yields `None`, never a wrong byte.
        let resolve_doc_len = |index: &DocumentIndex, field: &str, doc_id: u32| -> Option<u64> {
            index
                .field_stats_segments(field)
                .into_iter()
                .rev()
                .find(|(doc_base, _)| *doc_base <= doc_id)
                .and_then(|(doc_base, stats)| stats.doc_len(doc_id - doc_base))
        };
        for doc_id in 0..DOC_COUNT {
            assert_eq!(
                merged.subfield_value("NOM.raw", doc_id),
                oracle.subfield_value("NOM.raw", doc_id),
                "NOM.raw diverged for doc {doc_id}"
            );
            for field in ["NOM", "BODY"] {
                assert_eq!(
                    resolve_doc_len(&merged, field, doc_id),
                    resolve_doc_len(&oracle, field, doc_id),
                    "doc_len diverged for doc {doc_id} field {field}"
                );
                assert!(
                    resolve_doc_len(&merged, field, doc_id).is_some(),
                    "every doc has a recorded {field} length in this corpus \
                     (guards against a trivially-equal None == None pass)"
                );
            }
        }
    }

    #[test]
    fn merge_cap_bounds_run_doc_count_and_matches_unmerged_oracle() {
        // Plan segments S3b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
        // §S3b, "cap de taille de tier"): a small `SURCH_MERGE_MAX_DOCS`
        // cap must (a) still cascade merges up to the cap, (b) never let
        // any sealed segment's `doc_count` exceed it, and (c) still
        // produce results identical to the unmerged oracle — the cap only
        // changes segmentation granularity, never correctness.
        const DOC_COUNT: u32 = 60;
        const FANIN: usize = 3;
        const CAP: u64 = 5;
        let (capped, _mapping) =
            build_merge_test_index_with_cap(DOC_COUNT, FANIN, false, Some(CAP));
        let (uncapped, _mapping) = build_merge_test_index(DOC_COUNT, FANIN, false);
        let (oracle, _mapping) = build_merge_test_index(DOC_COUNT, 0, false);

        let sealed_end = capped.segments.len() - 1;
        assert!(
            capped.segments[..sealed_end]
                .iter()
                .all(|segment| u64::from(segment.doc_count) <= CAP),
            "every sealed segment must respect the {CAP}-doc cap, got sizes {:?}",
            capped.segments[..sealed_end]
                .iter()
                .map(|s| s.doc_count)
                .collect::<Vec<_>>()
        );
        assert!(
            capped.segment_count() > uncapped.segment_count(),
            "a small cap must force MORE (smaller) segments than the \
             uncapped merge policy: capped={}, uncapped={}",
            capped.segment_count(),
            uncapped.segment_count()
        );
        assert!(
            capped.segment_count() < oracle.segment_count(),
            "the cap must still cascade merges below one segment per doc: \
             capped={}, oracle={}",
            capped.segment_count(),
            oracle.segment_count()
        );

        // Parity vs the unmerged oracle: the cap must never lose or
        // duplicate data, only change how many segments it lands in.
        assert_eq!(capped.doc_ids(), oracle.doc_ids());
        assert_eq!(capped.live_doc_count(), oracle.live_doc_count());
        for field in ["NOM", "BODY"] {
            assert_eq!(
                capped.field_stats_aggregated(field),
                oracle.field_stats_aggregated(field),
                "field_stats_aggregated diverged for {field} under a cap"
            );
        }
        for term in ["commun", "lorem"] {
            let sorted_pairs = |index: &DocumentIndex| -> Vec<(u32, u32)> {
                let (ids, freqs) = index
                    .decode_from_segment("BODY", term)
                    .unwrap_or_else(|| panic!("BODY={term} must be present"));
                let mut pairs: Vec<(u32, u32)> = ids.into_iter().zip(freqs).collect();
                pairs.sort_unstable();
                pairs
            };
            assert_eq!(
                sorted_pairs(&capped),
                sorted_pairs(&oracle),
                "BODY={term} postings diverged under a cap"
            );
        }
        for doc_id in 0..DOC_COUNT {
            assert_eq!(
                capped.subfield_value("NOM.raw", doc_id),
                oracle.subfield_value("NOM.raw", doc_id),
                "NOM.raw diverged for doc {doc_id} under a cap"
            );
        }
    }

    #[test]
    fn merge_ram_and_disk_modes_produce_identical_reads() {
        // (d) the same merge policy driving a RAM-backed vs a disk-backed
        // `TermDictionary` per source segment must read back identically —
        // `merge_term_dictionaries` branches per SOURCE (`disk_backed()`),
        // not on a single global assumption.
        const DOC_COUNT: u32 = 24;
        let (ram, _mapping) = build_merge_test_index(DOC_COUNT, 3, false);
        let (disk, _mapping) = build_merge_test_index(DOC_COUNT, 3, true);

        assert!(ram.segment_count() > 1, "sanity: merge must have run");
        assert_eq!(ram.doc_ids(), disk.doc_ids());
        assert_eq!(ram.live_doc_count(), disk.live_doc_count());
        for field in ["NOM", "BODY"] {
            assert_eq!(
                ram.field_stats_aggregated(field),
                disk.field_stats_aggregated(field),
                "field_stats_aggregated diverged RAM vs disk for {field}"
            );
        }
        for term in ["commun", "lorem"] {
            let sorted_pairs = |index: &DocumentIndex| -> Vec<(u32, u32)> {
                let (ids, freqs) = index
                    .decode_from_segment("BODY", term)
                    .unwrap_or_else(|| panic!("BODY={term} must be present"));
                let mut pairs: Vec<(u32, u32)> = ids.into_iter().zip(freqs).collect();
                pairs.sort_unstable();
                pairs
            };
            assert_eq!(
                sorted_pairs(&ram),
                sorted_pairs(&disk),
                "BODY={term} postings diverged RAM vs disk merged reads"
            );
        }
        for doc_id in 0..DOC_COUNT {
            assert_eq!(
                ram.subfield_value("NOM.raw", doc_id),
                disk.subfield_value("NOM.raw", doc_id),
                "NOM.raw diverged RAM vs disk for doc {doc_id}"
            );
        }
        // Plan segments S5c: same corpus, now via the FULL-COLUMN-SCAN
        // accessor `AppState::subfield_projection` actually uses — must
        // agree exactly with the per-doc accessor above.
        assert_eq!(
            ram.subfield_projection("NOM.raw"),
            disk.subfield_projection("NOM.raw"),
            "subfield_projection diverged RAM vs disk merged reads"
        );
        // White-box: prove this test actually exercised the spilled merge
        // path (`merge_subfield_values` reading from an ALREADY-spilled
        // source, since `build_merge_test_index(_, _, true)` spills at
        // EVERY per-chunk seal before any cascade even runs), not a
        // silent resident fallback — and that the merged segment itself
        // was re-spilled, now via the S3b STREAMED path
        // (`merge_subfield_column_streamed`, see
        // `merge_subfield_streamed_matches_resident_merge` below for the
        // dedicated streamed-vs-resident parity check).
        assert!(
            disk.subfield_segment_bytes() > 0,
            "merged disk-enabled index must have spilled subfield bytes on disk"
        );
        assert_eq!(
            ram.subfield_segment_bytes(),
            0,
            "RAM-mode index must never create a subfield segment file"
        );
        for segment in &disk.segments[..disk.segments.len() - 1] {
            if let Some(column) = segment.subfield_values.get("NOM.raw") {
                assert!(
                    column.disk.is_some(),
                    "every sealed segment's NOM.raw column must be spilled under disk_enabled"
                );
            }
        }
    }

    #[test]
    fn merge_subfield_streamed_matches_resident_merge() {
        // Plan segments S3b (`docs/paper/design-segments-pic-borne-2026-07-05.md`
        // §S3b): white-box parity between the new STREAMED merge+spill
        // (`merge_subfield_column_streamed` — dict pass, then a
        // per-SOURCE codes pass appended directly to a fresh file, NEVER
        // holding the run's whole `Σ doc_count`-sized `codes` array
        // resident) and the historical fully-resident merge
        // (`merge_subfield_column_resident`, kept as the best-effort
        // fallback) — both consuming the SAME already-spilled source
        // segments, so this exercises `SubfieldColumn::dict_only`/
        // `codes_only`'s `pread` paths, not their resident-clone
        // shortcuts.
        const DOC_COUNT: u32 = 11;
        let (index, _mapping) = build_merge_test_index(DOC_COUNT, 0, true);
        // fanin=0: never auto-merges, so every doc got its OWN sealed
        // segment — `run` is exactly the sealed prefix (excludes the
        // final, still-empty active segment).
        let run: &[Arc<Segment>] = &index.segments[..index.segments.len() - 1];
        assert!(run.len() >= 3, "sanity: need several sources to merge");
        assert!(
            run.iter().all(|segment| segment
                .subfield_values
                .get("NOM.raw")
                .is_some_and(|column| column.disk.is_some())),
            "every source's NOM.raw column must already be spilled for this test \
             to actually exercise dict_only/codes_only's pread paths"
        );

        let mut store = SubfieldSegment::try_new().expect("tempfile for the merged column");
        let streamed = merge_subfield_column_streamed(run, "NOM.raw", &mut store)
            .expect("streaming must succeed under normal (non-failing) I/O");
        assert!(
            streamed.disk.is_some(),
            "a successful streamed merge must produce an already-spilled column"
        );
        let resident = merge_subfield_column_resident(run, "NOM.raw");
        assert!(
            resident.disk.is_none(),
            "the resident fallback must produce a RAM column (nothing to \
             compare against otherwise)"
        );

        let merged_doc_count: u32 = run.iter().map(|segment| segment.doc_count).sum();
        let streamed_materialized = streamed.materialized(Some(&store));
        for doc_id in 0..merged_doc_count {
            assert_eq!(
                streamed_materialized.get(doc_id),
                resident.get(doc_id),
                "streamed vs resident merge diverged for local doc_id {doc_id}"
            );
        }
    }

    #[test]
    fn subfield_spill_is_transparent_across_a_plain_seal() {
        // Plan segments S5c: NOT a merge scenario — exercises the ordinary
        // seal path (`materialize_terms_and_finalize_postings` →
        // `Segment::seal_subfield_columns`) directly on a mono-segment
        // index, forcing exactly one real `_refresh` so the column is
        // genuinely spilled (not merely finalized). Compares against a RAM
        // oracle built from the SAME corpus with the flag off.
        let mapping = nom_multi_field_mapping();
        const DOC_COUNT: u32 = 30;

        let build = |disk_enabled: bool| -> DocumentIndex {
            let mut index = DocumentIndex::new();
            // Pinned deterministic regardless of ambient env (same
            // rationale as every other override in this module): forcing
            // "no budget" keeps this a single-segment test, isolating the
            // ordinary seal path from S3's merge path (covered separately
            // by `merge_ram_and_disk_modes_produce_identical_reads`).
            index.set_flush_budget_bytes_override(None);
            index.set_postings_disk_enabled(disk_enabled);
            for doc_id in 0..DOC_COUNT {
                index
                    .add_documents_with_mapping_deferred(
                        [(doc_id, [("NOM", format!("Nom Numero {doc_id}"))])],
                        &mapping,
                    )
                    .unwrap_or_else(|err| panic!("doc {doc_id}: {err:?}"));
            }
            index.materialize_terms_and_finalize_postings();
            index
        };

        let ram = build(false);
        let disk = build(true);
        assert_eq!(ram.segment_count(), 1, "sanity: mono-segment RAM oracle");
        assert_eq!(disk.segment_count(), 1, "sanity: mono-segment disk index");

        // White-box: the disk-enabled index actually spilled — proves the
        // parity checks below exercise the pread path, not a silent
        // resident fallback.
        let disk_segment = &disk.segments[0];
        assert!(
            disk_segment.subfield_segment.is_some(),
            "disk-enabled seal must create a shared subfield segment file"
        );
        let disk_column = disk_segment
            .subfield_values
            .get("NOM.raw")
            .expect("NOM.raw column present");
        assert!(disk_column.disk.is_some(), "column must be spilled");
        assert!(
            disk_column.dict.is_empty() && disk_column.codes.is_empty(),
            "resident dict/codes must be emptied once spilled"
        );
        let ram_segment = &ram.segments[0];
        assert!(ram_segment.subfield_segment.is_none());
        let ram_column = ram_segment
            .subfield_values
            .get("NOM.raw")
            .expect("NOM.raw column present (ram)");
        assert!(ram_column.disk.is_none());

        // Parity: single-doc accessor.
        for doc_id in 0..DOC_COUNT {
            assert_eq!(
                ram.subfield_value("NOM.raw", doc_id),
                disk.subfield_value("NOM.raw", doc_id),
                "NOM.raw diverged for doc {doc_id}"
            );
        }
        // Parity: the full-column-scan accessor sort/agg actually uses.
        assert_eq!(
            ram.subfield_projection("NOM.raw"),
            disk.subfield_projection("NOM.raw"),
            "subfield_projection diverged flag on vs off"
        );
        assert!(
            ram.subfield_projection("NOM.raw")
                .is_some_and(|projection| projection.len() as u32 == DOC_COUNT),
            "sanity: every doc has a NOM.raw value in this fixture"
        );

        // Gauge: resident bytes collapse, disk footprint appears — the
        // `~0 (ou la moitié spillée)` gate from the design doc.
        assert!(
            disk_column.memory_bytes() < ram_column.memory_bytes(),
            "spilled column ({} bytes) must be smaller than the resident \
             one ({} bytes)",
            disk_column.memory_bytes(),
            ram_column.memory_bytes()
        );
        assert!(disk.subfield_segment_bytes() > 0);
        assert_eq!(ram.subfield_segment_bytes(), 0);
    }
}
