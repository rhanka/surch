use std::{
    cell::RefCell,
    collections::{btree_map::Entry, BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::{Arc, RwLock},
};

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use rayon::prelude::*;
use serde_json::Value;
use surch_index::{
    document_index::DocumentIndex,
    mapping::{AnalysisSettings, FieldMapping, FieldType, IndexMapping},
    memory::{document_index_memory_usage, MemoryUsage},
    postings::{BlockMeta, PostingsBlockSkipIter, PostingsList},
    roaring::RoaringDocSet,
};

/// `mmap M1` (P1 du plan persistance segments + manifest, cf.
/// `docs/paper/persistence-iceberg-architecture.md` §10) — cherry-pick
/// adapte du commit `3c864af` reverte a tort le 2026-06-09. Le timeout
/// 58 min observe sur `mmap M1.1` (`c2fd9ad`) etait du au **bug Artillery
/// bulk-search stall pre-existant** ET a l'**extension ext4 1-10 ms par
/// `pwrite` en bout de fichier** (1.36 M × 5 ms ≈ 113 min, cf.
/// `docs/paper/memory-campaign-blocker.md` §"Diagnostic"). On corrige les
/// deux : (1) on cherry-pick l'esprit du store file-backed (pas le commit
/// litteral car il faut composer avec l'option B compression
/// post-refresh mergee depuis dans `febbc86`) ; (2) on ajoute
/// `posix_fallocate(64 MiB)` a la creation du segment pour pre-allouer
/// une extent contigue, ce qui ramene chaque `pwrite` a un cout O(1)
/// independant de la taille courante du fichier.
///
/// Le module reste a l'interieur de `state.rs` — il n'a pas vocation a
/// etre reutilise par d'autres crates, et P2 (manifest atomique) le
/// migrera vers `surch-store` avec une API segments multi-instances.
#[cfg(target_os = "linux")]
mod source_store {
    use std::{
        fs::{File, OpenOptions},
        os::unix::{fs::FileExt, io::AsRawFd},
        path::PathBuf,
        sync::Arc,
    };

    /// Pre-allocation initiale du segment `source.dat`, en octets.
    /// 64 MiB couvre confortablement plusieurs dizaines de milliers de
    /// docs deces avant qu'un `posix_fallocate` supplementaire ne soit
    /// necessaire ; l'append boucle re-fallocate par chunks de 64 MiB
    /// chaque fois que `next_offset + bytes.len() > fallocated_len` (cf.
    /// `SourceStore::ensure_capacity`). Choix justifie : sur deces
    /// 1.36 M docs × ~300 oct/doc serialise = ~400 MiB, soit ~7
    /// arrondis de 64 MiB ; sur SciFact 5 k docs = un seul.
    const FALLOCATE_CHUNK: i64 = 64 * 1024 * 1024;

    /// `mmap M1` — segment `_source` append-only file-backed, indexe par
    /// `doc_id` interne. `idx[doc_id] = Some((offset, length))` pour
    /// les docs vivants, `None` pour les supprimes (sparse). Reads via
    /// Linux `pread` (`FileExt::read_exact_at`) — concurrent search
    /// share `Arc<File>` sans verrou applicatif.
    #[derive(Debug)]
    pub(super) struct SourceStore {
        file: Arc<File>,
        next_offset: u64,
        /// Taille deja `posix_fallocate`-ee, en octets. On
        /// re-fallocate par chunks de 64 MiB chaque fois que la
        /// prochaine ecriture deborderait. Le but est d'eviter
        /// l'extension ext4 *par bloc 4 KiB* a chaque `pwrite`,
        /// remplacee par une extension *par bunch de 64 MiB*. Sur
        /// deces : 7 fallocate au lieu de 1.36 M extensions.
        fallocated_len: i64,
        /// Plus grosse taille `next_offset` jamais atteinte depuis la
        /// creation du segment. Pas reset par `reset()` — sert a la
        /// gauge `surch_index_disk_segment_peak_bytes` (axe #19) pour
        /// que le scrape post-refresh voie la mesure pic-disque,
        /// pas le 0 post-truncate de la compaction.
        peak_offset: u64,
        path: PathBuf,
    }

    impl Default for SourceStore {
        fn default() -> Self {
            let id = uuid::Uuid::new_v4();
            let path = std::env::temp_dir().join(format!("surch-source-{id}.dat"));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .expect("failed to create surch source store tempfile");
            let mut store = Self {
                file: Arc::new(file),
                next_offset: 0,
                fallocated_len: 0,
                peak_offset: 0,
                path,
            };
            store.fallocate(FALLOCATE_CHUNK);
            store
        }
    }

    impl SourceStore {
        /// `posix_fallocate(fd, 0, fallocated_len + extra)` via
        /// l'appel direct `libc::posix_fallocate`. Mute
        /// `fallocated_len`. Pas d'erreur fatale si le syscall echoue
        /// (ENOSPC, EINVAL) — on log seulement, le fichier garde sa
        /// taille courante et chaque `pwrite` retombera sur
        /// l'extension ext4 classique (le bug original).
        #[allow(unsafe_code)]
        fn fallocate(&mut self, extra: i64) {
            let new_len = self.fallocated_len.saturating_add(extra);
            // SAFETY: `self.file.as_raw_fd()` est valide pendant la vie
            // de `self.file` (Arc) ; `posix_fallocate` ne touche pas
            // les bytes (zero-fill mais a la longueur, pas du
            // contenu) ; les arguments sont des entiers positifs.
            let ret = unsafe { libc::posix_fallocate(self.file.as_raw_fd(), 0, new_len) };
            if ret == 0 {
                self.fallocated_len = new_len;
            } else {
                tracing::warn!(
                    target: "surch_api::source_store",
                    errno = ret,
                    fallocated_len = self.fallocated_len,
                    new_len,
                    "posix_fallocate failed; falling back to per-pwrite ext4 expansion (mmap M1.1 timeout risk)"
                );
            }
        }

        fn ensure_capacity(&mut self, write_end: u64) {
            while (write_end as i64) > self.fallocated_len {
                self.fallocate(FALLOCATE_CHUNK);
                // Si fallocate a echoue, on sort de la boucle apres
                // un seul tour pour eviter le spin.
                if self.fallocated_len == 0 {
                    break;
                }
            }
        }

        /// Append `bytes` au segment, retourne `(offset, length)` pour
        /// stocker dans le `BTreeMap<Arc<str>, SourceBlob>`.
        pub(super) fn append(&mut self, bytes: &[u8]) -> (u64, u32) {
            let offset = self.next_offset;
            let length = bytes.len() as u32;
            self.ensure_capacity(offset.saturating_add(length as u64));
            self.file
                .write_all_at(bytes, offset)
                .expect("source store segment file should accept positional write");
            self.next_offset = offset.saturating_add(length as u64);
            if self.next_offset > self.peak_offset {
                self.peak_offset = self.next_offset;
            }
            (offset, length)
        }

        /// Read `length` bytes at `offset` via `pread`. Concurrent-safe
        /// (no lock applicatif), share `Arc<File>` entre threads.
        pub(super) fn read(&self, offset: u64, length: u32) -> Vec<u8> {
            let mut buf = vec![0u8; length as usize];
            self.file
                .read_exact_at(&mut buf, offset)
                .expect("source store segment file should accept positional read");
            buf
        }

        /// Reset le segment a vide : truncate le fichier a 0, re-fallocate
        /// le chunk initial. Appele par `compact_after_refresh` une
        /// fois que TOUS les `SourceBlob::OnDisk` ont ete migres en
        /// `Compressed` — les bytes du segment sont alors orphelins
        /// (plus reference par aucun slot `documents`).
        pub(super) fn reset(&mut self) {
            if let Err(err) = self.file.set_len(0) {
                tracing::warn!(
                    target: "surch_api::source_store",
                    ?err,
                    "failed to truncate source store segment; segment file will keep its on-disk size"
                );
                return;
            }
            self.next_offset = 0;
            self.fallocated_len = 0;
            self.fallocate(FALLOCATE_CHUNK);
        }

        /// Taille on-disk effective du segment (bytes ecrits, hors
        /// reserve `posix_fallocate`). Exposee pour l'axe #19 (mesure
        /// disque) — la gauge dediee sera branchee par P2 (manifest +
        /// `_cat/indices?bytes=b`). `allow(dead_code)` en attendant.
        pub(super) fn bytes_written(&self) -> u64 {
            self.next_offset
        }

        /// Pic on-disk depuis la creation : `max(next_offset)` jamais
        /// reset par `reset()`. C'est la mesure utile pour l'axe disque
        /// #19 puisque le scrape #20 arrive APRES `_refresh` qui
        /// declenche `compact_after_refresh` -> `reset()` -> 0.
        pub(super) fn peak_bytes_written(&self) -> u64 {
            self.peak_offset
        }
    }

    impl Drop for SourceStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Fallback non-Linux : tampon in-RAM derriere la meme API. Cible de
/// release est Linux distroless cc-debian12, mais le code doit
/// continuer a builder/tester sur macOS dev sans pulling `libc::
/// posix_fallocate`.
#[cfg(not(target_os = "linux"))]
mod source_store {
    #[derive(Debug, Default)]
    pub(super) struct SourceStore {
        buf: Vec<u8>,
    }

    impl SourceStore {
        pub(super) fn append(&mut self, bytes: &[u8]) -> (u64, u32) {
            let offset = self.buf.len() as u64;
            self.buf.extend_from_slice(bytes);
            (offset, bytes.len() as u32)
        }

        pub(super) fn read(&self, offset: u64, length: u32) -> Vec<u8> {
            let start = offset as usize;
            let end = start + length as usize;
            self.buf[start..end].to_vec()
        }

        pub(super) fn reset(&mut self) {
            self.buf.clear();
            self.buf.shrink_to_fit();
        }

        pub(super) fn bytes_written(&self) -> u64 {
            self.buf.len() as u64
        }

        pub(super) fn peak_bytes_written(&self) -> u64 {
            // Fallback non-Linux : pas de notion de pic distincte du courant
            // (Vec<u8> est resetable instantanement, on n'a pas la
            // segmentation linux/fichier qui motive `peak_offset`).
            self.buf.len() as u64
        }
    }
}

use source_store::SourceStore;

/// Campagne mémoire — option B (cf. `docs/paper/memory-pivot-decision.md`)
/// **composee avec `mmap M1` (P1 persistance)**.
///
/// Apres `mmap M1`, la variante "fresh source" du bulk path n'est plus
/// `Raw(Arc<str>)` en RAM : c'est `OnDisk { offset, length }`, un
/// pointeur dans le segment `source.dat` file-backed (cf. module
/// `source_store`). Les bytes vivent dans le page-cache OS, pas dans le
/// heap du process. Apres `_refresh`, `compact_after_refresh` lit chaque
/// `OnDisk`, compresse, et remplace le slot par `Compressed(Arc<[u8]>)`
/// — RAM moyenne ~400 MiB compresses, segment `source.dat` truncate a 0.
///
/// Décisions :
/// - Hot path INSERT (`upsert_document_deferred`) : append au segment,
///   stocke `OnDisk`. Cout : 1× `pwrite` (~5 µs amorti grace au
///   `posix_fallocate`) + 8 octets de side-table. Gate indexation
///   >= 14 000 docs/s preserve.
/// - Hot path search post-refresh : `Compressed` decompresses via
///   `Decompress` thread-local (~5-10 µs / blob).
/// - Hot path search PENDANT le bulk (cf. `append_to_index` /
///   `rebuild_index` qui passent par `parsed_source`) : la voie OnDisk
///   fait un `pread` (~5-10 µs SSD NVMe) + parse. Marginal sur top-K=20.
/// - Gauge `stored_fields_bytes` ne compte que les bytes RAM
///   (`Compressed.len()`) — les `OnDisk` sont sortis du heap par
///   construction. La taille on-disk visible est exposee separement via
///   `source_store.bytes_written()` (axe #19).
#[derive(Clone, Debug)]
enum SourceBlob {
    /// Pointeur dans le segment `source.dat` file-backed. Etat
    /// transitoire entre l'INSERT bulk et le premier `_refresh`. Le
    /// `length` `u32` borne un doc a 4 GiB — largement au-dela des
    /// `_source` matchID / BEIR observes (< 1 MiB).
    OnDisk { offset: u64, length: u32 },
    /// Bytes deflate-bruts produits par `compact_after_refresh()`.
    /// Decode via [`SourceBlob::decode_compressed`] (Decompress thread-local).
    Compressed(Arc<[u8]>),
}

impl SourceBlob {
    /// Taille en **RAM** des bytes utiles (sans le header Arc), pour la
    /// gauge `surch_index_stored_fields_bytes`. Les blobs `OnDisk`
    /// retournent 0 : leurs bytes vivent dans le segment file-backed
    /// (page-cache OS), pas dans le heap du process — c'est exactement
    /// le levier RAM que `mmap M1` apporte.
    fn payload_len(&self) -> usize {
        match self {
            Self::OnDisk { .. } => 0,
            Self::Compressed(b) => b.len(),
        }
    }

    /// Décode un blob compressé en `Vec<u8>`. Réutilise un `Decompress`
    /// thread-local pour amortir l'init (le bug #15 inc3c était
    /// l'allocation per-call d'un `Decoder` haut-niveau ; on garde le
    /// codec entre appels).
    ///
    /// Boucle obligatoire `decompress_vec` jusqu'à `Status::StreamEnd`
    /// — c'est le fix correctness inc3b/inc3c : un seul appel peut
    /// retourner `Status::Ok` avec un output buffer rempli sans avoir
    /// fini, ce qui tronquait silencieusement les gros docs.
    fn decode_compressed(bytes: &[u8]) -> Vec<u8> {
        thread_local! {
            static DECODER: RefCell<Decompress> = RefCell::new(Decompress::new(false));
            static SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(4096));
        }

        DECODER.with(|decoder_cell| {
            SCRATCH.with(|scratch_cell| {
                let mut decoder = decoder_cell.borrow_mut();
                let mut scratch = scratch_cell.borrow_mut();
                decoder.reset(false);
                scratch.clear();
                // Heuristique : sortie attendue ~3-4× l'entrée pour du JSON.
                let initial = bytes.len().saturating_mul(4).max(4096);
                let cur_cap = scratch.capacity();
                if cur_cap < initial {
                    scratch.reserve(initial - cur_cap);
                }

                loop {
                    let prev_out = decoder.total_out();
                    let input_pos = decoder.total_in() as usize;
                    let status = decoder
                        .decompress_vec(&bytes[input_pos..], &mut scratch, FlushDecompress::Finish)
                        .expect(
                            "stored compressed _source decodes (compact_after_refresh contract)",
                        );
                    match status {
                        Status::StreamEnd => break,
                        Status::Ok => {
                            // Made progress mais pas encore fini — agrandir le
                            // buffer puis reprendre. Si on n'a fait AUCUN
                            // progrès on évite la boucle infinie.
                            if decoder.total_out() == prev_out {
                                let extra = scratch.capacity().max(4096);
                                scratch.reserve(extra);
                            }
                        }
                        Status::BufError => {
                            // Pas assez de place en sortie — agrandir.
                            let extra = scratch.capacity().max(4096);
                            scratch.reserve(extra);
                        }
                    }
                }
                scratch.clone()
            })
        })
    }

    /// Compresse les bytes d'un `Raw` pour produire un `Compressed`.
    /// Réutilise un `Compress` thread-local (cf. inc3c).
    /// `Compression::fast()` (level 1) : ~3× plus rapide que `default()`
    /// pour ~10 % de ratio en moins ; option B est dominée par la
    /// SOMME (refresh + decode hot path), pas par le ratio.
    fn encode_for_compact(raw: &[u8]) -> Vec<u8> {
        thread_local! {
            static ENCODER: RefCell<Compress> = RefCell::new(
                Compress::new(Compression::fast(), false),
            );
            static SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(4096));
        }

        ENCODER.with(|encoder_cell| {
            SCRATCH.with(|scratch_cell| {
                let mut encoder = encoder_cell.borrow_mut();
                let mut scratch = scratch_cell.borrow_mut();
                encoder.reset();
                scratch.clear();
                // Sortie deflate généralement << entrée pour du JSON
                // (ratio ~3-4×), mais on prévoit large pour éviter la
                // ré-alloc pendant la boucle.
                let initial = raw.len().max(4096);
                let cur_cap = scratch.capacity();
                if cur_cap < initial {
                    scratch.reserve(initial - cur_cap);
                }

                loop {
                    let prev_out = encoder.total_out();
                    let input_pos = encoder.total_in() as usize;
                    let status = encoder
                        .compress_vec(&raw[input_pos..], &mut scratch, FlushCompress::Finish)
                        .expect("Compress::compress_vec only fails on programmer error");
                    match status {
                        Status::StreamEnd => break,
                        Status::Ok | Status::BufError => {
                            if encoder.total_out() == prev_out {
                                let extra = scratch.capacity().max(4096);
                                scratch.reserve(extra);
                            }
                        }
                    }
                }
                scratch.clone()
            })
        })
    }
}

/// Helper pour parser un [`SourceBlob`] en `Value`. Reutilise par
/// `parsed_source`, `rebuild_index` et `documents_paginated`.
///
/// `mmap M1` : le `store` est lu pour les blobs `OnDisk` (pread sur le
/// segment file-backed) ; les `Compressed` decodent via le `Decompress`
/// thread-local (inchange option B).
fn parse_source_blob(blob: &SourceBlob, store: &SourceStore) -> Value {
    match blob {
        SourceBlob::OnDisk { offset, length } => {
            let bytes = store.read(*offset, *length);
            serde_json::from_slice(&bytes).expect("stored OnDisk _source is valid JSON")
        }
        SourceBlob::Compressed(bytes) => {
            let decoded = SourceBlob::decode_compressed(bytes.as_ref());
            serde_json::from_slice(&decoded)
                .expect("stored Compressed _source decodes to valid JSON")
        }
    }
}

use surch_search::scoring::{bm25_score, Bm25Config};

use crate::scroll::ScrollTable;
use crate::stats::{clear_memory_gauges, refresh_jemalloc_purge, refresh_memory_gauges};

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

// `mmap M1` : `InMemoryIndex` n'est plus `Clone`. Le champ
// `source_store` detient un `Arc<File>` qui ne devrait PAS etre
// duplique (deux indices partageant le meme segment file casseraient
// `next_offset` et les `OnDisk { offset, length }`). Le derive Clone
// originel n'etait pas utilise (verification par grep) — on le retire
// pour eviter un piege future.
#[derive(Debug, Default)]
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
    /// #15 memory: the `_source` is stored as the SERIALIZED JSON bytes,
    /// NOT a parsed `serde_json::Value` tree (the deces breakdown
    /// showed the parsed `Value` is the dominant RSS term — ~2-3× the serialized
    /// size). It is parsed back to a `Value` on access via [`Self::parsed_source`]
    /// — cheap because reads hydrate only the top-K window (~20 docs/query).
    ///
    /// Campagne mémoire option B (cf. `docs/paper/memory-pivot-decision.md`)
    /// composee avec `mmap M1` (P1 persistance) : la valeur est un
    /// [`SourceBlob`] qui peut être `OnDisk { offset, length }` (chemin
    /// INSERT bulk — pointeur dans `source_store`, page-cache OS) ou
    /// `Compressed(Arc<[u8]>)` (état après [`Self::compact_after_refresh`]).
    /// La voie d'INSERT et
    /// la voie `append_to_index` voient EXCLUSIVEMENT `OnDisk`, donc le
    /// gate indexation reste intact par construction. Le decode payé
    /// sur la voie search post-refresh (~5-10 µs / blob) ne touche que
    /// les ~20 docs du top-K.
    /// Lot C Phase 1 levier 3 : la clé est un `Arc<str>` PARTAGÉ avec
    /// `document_ids` (clé) et `reverse_document_ids` (valeur) — les 3
    /// maps portent le même buffer UTF-8 alloué UNE SEULE fois par
    /// document, au lieu de 3 `String` dupliquées (~44 o/UID × 1,36 M ×
    /// 2 copies redondantes sur le corpus deces). Voir
    /// [`Self::upsert_document_deferred`] pour le point d'insertion
    /// partagée (un seul `Arc::from`, deux `Arc::clone`).
    documents: BTreeMap<Arc<str>, SourceBlob>,
    /// `mmap M1` — segment `source.dat` file-backed sous `TMPDIR`,
    /// pre-alloue par `posix_fallocate(64 MiB)`. Append-only pendant le
    /// bulk, truncate a 0 a la fin de `compact_after_refresh` une fois
    /// tous les blobs migres en `Compressed`. Le store est cree
    /// paresseusement via `Default` (un tempfile par index) ; il est
    /// supprime au `Drop` de `InMemoryIndex`.
    source_store: SourceStore,
    /// Lot C Phase 1 levier 3 : clé `Arc<str>` partagée avec `documents`
    /// (clé) et `reverse_document_ids` (valeur) — même buffer, pas de
    /// copie des octets UTF-8 de l'UID.
    document_ids: BTreeMap<Arc<str>, u32>,
    /// Lot C Phase 1 levier 3 : valeur `Arc<str>` partagée avec
    /// `documents` et `document_ids` (mêmes instances, `Arc::clone`
    /// uniquement).
    reverse_document_ids: BTreeMap<u32, Arc<str>>,
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
    /// Dense per-`doc_id` **Lucene `SmallFloat`-quantized** length (`0` =
    /// absent), borrowed ZERO-COPY from the index's
    /// `FieldLengthStats::doc_len_dense`. Each byte must be decoded via
    /// [`surch_index::decode_doc_len_byte`] (or [`Self::doc_len`]) before
    /// being fed to BM25 — the encoding is the same one Lucene's
    /// `BM25Similarity` uses, so the reconstructed value is the value the
    /// scorer must consume (see `docs/paper/ndcg-trec-covid-rootcause-22.md`).
    ///
    /// Empty slice when `norms_enabled` is false. The switch from
    /// `Vec<u64>` to `Vec<u8>` also drops `field_stats_bytes` ~8× on the
    /// 1.36 M docs × ~6 indexed fields corpus (~65 MiB freed).
    pub doc_len_dense: &'a [u8],
    /// Precomputed smallest reconstructed `doc_len` (`0` = none),
    /// threaded from the index's incrementally-maintained
    /// `FieldLengthStats::min_doc_len` so the WAND upper bound no
    /// longer re-scans the dense slice per query. Already in the
    /// reconstructed-length domain (same as [`Self::doc_len`]).
    pub min_doc_len: u64,
}

impl<'a> FieldScoringStats<'a> {
    pub fn doc_len(&self, doc_id: u32) -> Option<u64> {
        self.doc_len_dense
            .get(doc_id as usize)
            .copied()
            .filter(|&byte| byte > 0)
            .map(surch_index::decode_doc_len_byte)
    }

    pub fn min_doc_len(&self) -> Option<u64> {
        (self.min_doc_len > 0).then_some(self.min_doc_len)
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
/// the live `doc_ids` / `freqs` SoA slices (Lot C Phase 1 levier 5:
/// `surch_index::postings::PostingsList::doc_ids` / `::freqs`) and the
/// [`BlockMeta`] slice straight out of the in-memory term dictionary,
/// eliminating both per-token allocations.
///
/// Parity: the postings come from the `TermDictionary` in ascending
/// `doc_id` order with exactly one `(doc_id, freq)` pair per
/// `(doc_id, field, term)` triple (the `analyzed_terms` invariant in
/// `DocumentIndex::add_validated_document`). So these borrowed slices are
/// element-for-element the same sequence the owned `term_freq_by_doc_id`
/// held, only with `freq` kept as `u32` (widened to `u64` at the exact
/// points the scorer consumes it). `doc_freq` equals `doc_ids.len()`,
/// matching the owned struct's `term_freq_by_doc_id.len()`.
#[derive(Clone, Copy, Debug)]
pub struct TermScoringView<'a> {
    pub doc_freq: u64,
    /// Borrowed `doc_id` channel, sorted ascending, one entry per doc.
    pub doc_ids: &'a [u32],
    /// Borrowed `freq` channel, index-aligned with `doc_ids` (posting `i`
    /// is `(doc_ids[i], freqs[i])`).
    pub freqs: &'a [u32],
    /// Borrowed per-block stats aligned with `doc_ids.chunks(BLOCK_SIZE)`.
    pub block_metas: &'a [BlockMeta],
}

impl<'a> TermScoringView<'a> {
    /// Empty view (term absent / field unknown). `doc_freq == 0` so the
    /// scorer skips it exactly as it skipped the default
    /// [`TermScoringStats`].
    pub fn empty() -> Self {
        Self {
            doc_freq: 0,
            doc_ids: &[],
            freqs: &[],
            block_metas: &[],
        }
    }

    /// Term frequency for `doc_id`, or 0 when the doc is absent. Binary
    /// search over the ascending-`doc_id` channel — identical lookup
    /// semantics to [`TermScoringStats::term_freq`], widening the stored
    /// `u32` freq to `u64`.
    pub fn term_freq(&self, doc_id: u32) -> u64 {
        self.doc_ids
            .binary_search(&doc_id)
            .ok()
            .map(|idx| u64::from(self.freqs[idx]))
            .unwrap_or(0)
    }

    /// Greatest term frequency across the postings (widened to `u64`).
    /// Matches [`TermScoringStats::max_term_freq`].
    pub fn max_term_freq(&self) -> u64 {
        self.freqs
            .iter()
            .map(|&freq| u64::from(freq))
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
        // Lot C Phase 1 levier 3 : un SEUL `Arc<str>` porte l'UID, partagé
        // entre `documents` (clé), `document_ids` (clé) et
        // `reverse_document_ids` (valeur) via `Arc::clone` — les 3 handles
        // pointent sur le MEME buffer alloué une fois. `entry()` consomme
        // toujours le buffer `Arc::from(id)` construit ci-dessous : sur un
        // INSERT (`Entry::Vacant`) il devient la clé stockée ; sur un
        // UPDATE (`Entry::Occupied`, id déjà présent) il est droppé et on
        // réutilise `Arc::clone(occupied.key())` — le buffer déjà partagé
        // par `document_ids`/`reverse_document_ids` — pour que `documents`
        // reste aligné sur la MEME instance (partage vrai y compris sur
        // update, pas seulement sur le chemin append-only dominant du
        // bulk matchID).
        let uid: Arc<str> = match self.document_ids.entry(Arc::from(id)) {
            Entry::Vacant(entry) => {
                let doc_id = self.next_doc_id;
                self.next_doc_id += 1;
                let uid = Arc::clone(entry.key());
                self.reverse_document_ids.insert(doc_id, Arc::clone(&uid));
                entry.insert(doc_id);
                uid
            }
            Entry::Occupied(entry) => Arc::clone(entry.key()),
        };

        // `mmap M1` + option B : on serialise puis on append au segment
        // `source.dat` file-backed (cf. module `source_store`). Le slot
        // `documents` stocke `OnDisk { offset, length }` — 12 octets en
        // RAM au lieu des bytes JSON eux-memes. La compression a lieu a
        // `_refresh` via `compact_after_refresh` (option B), qui
        // migrera ces blobs vers `Compressed(Arc<[u8]>)` puis truncate
        // le segment.
        //
        // Coût bulk : 1× pwrite (~5 µs amorti grace au
        // `posix_fallocate` qui evite l'extension ext4 par bloc 4 KiB)
        // au lieu de 1× `Arc::from` + insertion BTreeMap. Gate
        // indexation >= 14 000 docs/s preserve.
        //
        // `ensure_fields` a besoin du `Value` parse, donc analyse
        // AVANT serialisation. Updates (meme `id`) ecrasent le slot
        // `documents` — les bytes precedents dans `source.dat` sont
        // orphelins (acceptable pour P1 ; P2/P3 ajoutent une
        // compaction segments). En pratique le bulk matchID est
        // append-only par construction, donc zero orphelin.
        self.mapping.ensure_fields(&source);
        let serialized =
            serde_json::to_vec(&source).expect("a validated _source serialises to JSON");
        let (offset, length) = self.source_store.append(&serialized);
        self.documents
            .insert(uid, SourceBlob::OnDisk { offset, length });
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
        let store_ref = &self.source_store;
        let documents = self
            .documents
            .iter()
            .filter_map(|(id, blob)| {
                self.document_ids.get(id).map(|doc_id| {
                    // #15 + `mmap M1` : on parse le `_source` stocke
                    // (OnDisk -> pread sur le segment file-backed,
                    // Compressed -> decode thread-local). Seul chemin
                    // d'INDEXATION qui touche le decode/pread ; pas le
                    // hot path bulk steady-state.
                    let parsed = parse_source_blob(blob, store_ref);
                    (*doc_id, indexed_fields_for_document(&parsed, &self.mapping))
                })
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
        // Étape 1 du plan indexation 2× ES : parallélisation tokenization.
        // À ce point, `ensure_fields` a déjà été appelé pour CHAQUE doc du
        // batch (via `upsert_document_deferred`), donc `self.mapping` est
        // stable pour la suite. `parsed_source` et `indexed_fields_for_document`
        // ne touchent que des structures immutables (`documents` BTreeMap,
        // `mapping`, `reverse_document_ids`), donc l'itération `rayon::par_iter`
        // est thread-safe.
        //
        // `mmap M1` : sur le hot path bulk steady-state, `parsed_source`
        // emprunte la voie `OnDisk` → `pread` sur le segment `source.dat`
        // file-backed (les bytes viennent juste d'etre ecrits par
        // `upsert_document_deferred`, page-cache OS chaud). Aucun decode
        // codec ici. Chaque worker thread partage `Arc<File>` (Sync), le
        // `pread` syscall est concurrent-safe par construction. Cout
        // per-doc dominant ≈ 50 %, gain attendu ~1.4× sur 2 cores W=2.
        let self_ref = &*self;
        let documents = new_doc_ids
            .par_iter()
            .filter_map(|&doc_id| {
                let id = self_ref.reverse_document_ids.get(&doc_id)?;
                let source = self_ref.parsed_source(id)?;
                Some((
                    doc_id,
                    indexed_fields_for_document(&source, &self_ref.mapping),
                ))
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
        // Lot C Phase 0 : méthode combinée sans clone du builder — évite le
        // pic transitoire ~1,3 GiB (builder + clone) au refresh, prérequis
        // anti-OOM sous limite mémoire. Cf. document_index.rs.
        self.index.materialize_terms_and_finalize_postings();
        self.terms_finalized = true;
        // P1 mmap M1 + Option B se neutralisent : Option B compressait
        // OnDisk -> Compressed(Arc<[u8]>) en RAM, ce qui rapatriait les
        // bytes que P1 avait sorti sur disque. On ne compacte plus.
        // Gain attendu : −stored_fields_bytes sur la gauge. Le segment
        // `source.dat` conserve les bytes (pread O(1) sur la lecture).
        // Voir docs/paper/persistence-iceberg-architecture.md.
    }

    /// Convertit en bloc tous les `SourceBlob::OnDisk` du `documents` en
    /// `SourceBlob::Compressed` (deflate). Appelée UNE SEULE FOIS par
    /// `_refresh`, en dehors du hot path bulk : c'est le contrat de
    /// l'option B (cf. `docs/paper/memory-pivot-decision.md`) compose
    /// avec `mmap M1`.
    ///
    /// Sequence par blob :
    /// 1. Lit les bytes depuis `source.dat` via `pread`.
    /// 2. Encode deflate via `Compress` thread-local.
    /// 3. Remplace le slot par `Compressed(Arc<[u8]>)`.
    ///
    /// Une fois TOUS les blobs migres en `Compressed`, on tronque le
    /// segment `source.dat` a 0 (`source_store.reset()`) — les bytes
    /// du fichier sont alors orphelins, et la prochaine vague de bulk
    /// recommencera a offset 0. Sur deces post-refresh : segment
    /// `source.dat` 0 octets, RAM compressed ~400 MiB.
    ///
    /// Budget chiffré : ~5-10 µs / blob compression (Compress
    /// thread-local) + ~5-10 µs / blob pread (SSD NVMe). Sur deces
    /// 1.36 M docs : ~15-25 s ajoutes au `_refresh` ; gate non-bloquant
    /// (le `_refresh` n'est pas dans le hot path latence).
    ///
    /// Gain RAM cumule (`mmap M1` + option B) : `stored_fields_bytes`
    /// gauge passe de 1187 MiB en RAM heap → ~400 MiB en RAM compressed
    /// + 0 octet on-disk (segment truncate). RSS attendu deces 6621 → ~5500 MiB.
    ///
    /// NOTE: Plus appelée depuis `finalize_terms_for_refresh` — avec P1
    /// mmap M1 actif les blobs OnDisk restent sur disque (pread O(1)) et
    /// le heap baisse plus que via la compression option B. Conservée pour
    /// usage manuel/test et resurrection éventuelle (`P2 manifest` notamment).
    #[allow(dead_code)]
    fn compact_after_refresh(&mut self) {
        // Itération sur les clés pour eviter une re-clone du blob ;
        // `get_mut` puis remplacement sur place via `*slot = ...` evite
        // un re-hash dans `BTreeMap`.
        // Lot C Phase 1 levier 3 : `id.clone()` clone l'`Arc<str>` partagé
        // (bump du strong_count, pas de recopie d'octets).
        let ids: Vec<Arc<str>> = self
            .documents
            .iter()
            .filter_map(|(id, blob)| match blob {
                SourceBlob::OnDisk { .. } => Some(Arc::clone(id)),
                SourceBlob::Compressed(_) => None,
            })
            .collect();
        for id in ids {
            let Some(slot) = self.documents.get_mut(&id) else {
                continue;
            };
            if let SourceBlob::OnDisk { offset, length } = *slot {
                let raw_bytes = self.source_store.read(offset, length);
                let compressed = SourceBlob::encode_for_compact(&raw_bytes);
                *slot = SourceBlob::Compressed(Arc::from(compressed.into_boxed_slice()));
            }
        }
        // Tous les `OnDisk` sont desormais migres ; les bytes du
        // segment `source.dat` n'ont plus de reference depuis
        // `documents`. On tronque a 0 + re-fallocate le chunk initial
        // pour la prochaine vague bulk.
        let still_on_disk = self
            .documents
            .values()
            .any(|blob| matches!(blob, SourceBlob::OnDisk { .. }));
        if !still_on_disk {
            self.source_store.reset();
        }
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
            // Lot C Phase 1 levier 3: `reverse_document_ids` value is now a
            // shared `Arc<str>`; `term_hits` keeps its public `Vec<String>`
            // contract, so the public id is materialised here (bounded by
            // the matched-doc set, not the full corpus).
            .filter_map(|doc_id| {
                self.reverse_document_ids
                    .get(&doc_id)
                    .map(|s| s.to_string())
            })
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
                        .filter_map(|doc_id| {
                            self.reverse_document_ids.get(doc_id).map(|s| s.to_string())
                        })
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
                    .filter_map(|doc_id| {
                        self.reverse_document_ids.get(doc_id).map(|s| s.to_string())
                    })
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
        // O(1) cache-friendly doc_id indexing in the hot loop. Bytes are
        // Lucene `SmallFloat`-quantized lengths; reconstruction is folded
        // into `FieldScoringStats::doc_len` and the hot-path call sites
        // (search.rs / state.rs `bm25_field_score`).
        let doc_len_dense: &[u8] = if norms_enabled {
            stats.doc_len_dense()
        } else {
            &[]
        };

        let min_doc_len = if norms_enabled {
            stats.min_doc_len().unwrap_or(0)
        } else {
            0
        };

        Some(FieldScoringStats {
            doc_count: stats.doc_count,
            avg_doc_len,
            norms_enabled,
            doc_len_dense,
            min_doc_len,
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
                let doc_ids = list.doc_ids();
                let freqs = list.freqs();
                let block_metas = list.block_metas();
                debug_assert_eq!(
                    block_metas.len(),
                    doc_ids.len().div_ceil(128),
                    "block_metas alignment with postings chunks broken \
                     (field={field}, term={term}, postings={}, metas={})",
                    doc_ids.len(),
                    block_metas.len(),
                );
                TermScoringView {
                    doc_freq: doc_ids.len() as u64,
                    doc_ids,
                    freqs,
                    block_metas,
                }
            }
            None => TermScoringView::empty(),
        }
    }

    fn match_hits(&self, field: &str, value: &str, require_all_terms: bool) -> Vec<String> {
        self.match_hits_internal(field, value, require_all_terms)
            .into_iter()
            .filter_map(|doc_id| {
                self.reverse_document_ids
                    .get(&doc_id)
                    .map(|s| s.to_string())
            })
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
                Some(list) if !list.doc_ids().is_empty() => lists.push(list),
                _ => return Vec::new(),
            }
        }
        // Drive the rarest term; advance_to the others.
        lists.sort_by_key(|l| l.doc_ids().len());

        // A1 fast path: two high-`df` terms (both have a precomputed roaring
        // bitmap) AND word-parallel instead of the scalar O(df_rare) leapfrog.
        // Recall-only here (no scoring), so it just emits the intersection
        // doc_ids ascending — bit-identical to the leapfrog set below (the
        // roaring module is oracle-tested against a naive set intersection).
        if lists.len() == 2 {
            if let (Some(r0), Some(r1)) = (lists[0].roaring(), lists[1].roaring()) {
                let mut out = Vec::new();
                r0.intersect_for_each(r1, |doc_id| out.push(doc_id));
                return out;
            }
        }

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
        let mut cur: Vec<Option<u32>> = iters.iter_mut().map(|it| it.advance_to(0)).collect();
        let mut out = Vec::new();
        // Drive the rarest term's compact doc_id channel (4 B/entry, half the
        // cache footprint an AoS `Posting` would carry) — the conjunction
        // never needs `freq` here.
        'docs: for &target in lists[0].doc_ids() {
            for (i, it) in iters.iter_mut().enumerate() {
                if cur[i].is_some_and(|c| c < target) {
                    cur[i] = it.advance_to(target);
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
            let set: BTreeSet<u32> = l.doc_ids().iter().copied().collect();
            acc = Some(match acc {
                None => set,
                Some(prev) => prev.intersection(&set).copied().collect(),
            });
        }
        acc.unwrap_or_default().into_iter().collect()
    }

    /// Parse a stored `_source` (serialized bytes, #15) back into an owned
    /// `Arc<Value>` for the consumers that need a `&Value` (build_hit,
    /// query_matches, lookup_sort_value, …). `None` when `id` is unknown.
    ///
    /// `mmap M1` + option B : dispatch sur la variante `SourceBlob`.
    /// `OnDisk { offset, length }` paie un `pread` sur le segment
    /// file-backed (~5-10 µs SSD NVMe), `Compressed(Arc<[u8]>)` paie
    /// un decode thread-local (~5-10 µs) sur la voie search
    /// post-refresh.
    fn parsed_source(&self, id: &str) -> Option<Arc<Value>> {
        let blob = self.documents.get(id)?;
        Some(Arc::new(parse_source_blob(blob, &self.source_store)))
    }

    fn documents_by_internal_ids(&self, index: &str, internal_ids: &[u32]) -> Vec<StoredDocument> {
        internal_ids
            .iter()
            .filter_map(|doc_id| {
                let id = self.reverse_document_ids.get(doc_id)?;
                self.parsed_source(id).map(|source| StoredDocument {
                    index: index.to_owned(),
                    // Lot C Phase 1 levier 3: `id` borrows the shared
                    // `Arc<str>`; `StoredDocument::id` stays a plain
                    // `String` (no serde `rc` feature enabled), so it is
                    // materialised here for this hydrated document only.
                    id: id.to_string(),
                    source,
                })
            })
            .collect()
    }

    /// Hydrate documents while CARRYING each one's internal `doc_id`. The
    /// candidate-resolution path already knows the internal ids (postings are
    /// `u32`-keyed), so threading them to scoring avoids re-deriving them with
    /// an `O(n)` public-`_id` String hashmap round-trip per matched doc — the
    /// deces `bool`/`function_score` tail, which scales with the (high-`df`)
    /// candidate-set size. Skipped ids (deleted/unknown) carry no pair, so the
    /// `(doc_id, doc)` alignment is always exact (no positional assumption).
    fn documents_with_internal_ids(
        &self,
        index: &str,
        internal_ids: &[u32],
    ) -> Vec<(u32, StoredDocument)> {
        internal_ids
            .iter()
            .filter_map(|&doc_id| {
                let id = self.reverse_document_ids.get(&doc_id)?;
                self.parsed_source(id).map(|source| {
                    (
                        doc_id,
                        StoredDocument {
                            index: index.to_owned(),
                            id: id.to_string(),
                            source,
                        },
                    )
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
        // Lot C Phase 1 : `finalize_terms_for_refresh` (au-dessus, sous
        // le write-lock) vient de droper le `PostingsBuilder` — c'est
        // le free-storm qui libère les millions de petites `Vec`
        // par-terme. On purge les extents jemalloc AVANT de relire les
        // gauges (`refresh_memory_gauges` ci-dessous se termine par
        // `refresh_jemalloc_gauges()`), pour que la mesure exposée
        // reflète le RSS post-purge plutôt que le pic transitoire.
        // Purge hors du lock (pas besoin du store) et jamais sur le
        // hot path de requête : uniquement ici, sur `_refresh`, un
        // évènement rare. Voir `stats::refresh_jemalloc_purge` pour le
        // détail de l'API jemalloc utilisée.
        refresh_jemalloc_purge();
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
                    } else if let Some(&doc_id) = data.document_ids.get(id.as_str()) {
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
                        if let Some(&doc_id) = data.document_ids.get(id.as_str()) {
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
        let column = data.index.subfield_values_map().get(field_path)?;
        // Lot C Phase 1 lever 2: `column.iter()` yields owned `doc_id: u32`
        // (dense-array index) + borrowed `&str` (dict-interned, zero-copy)
        // instead of the previous `BTreeMap<u32, String>::iter()` pairs of
        // borrowed `&u32`/`&String` — same ascending-doc_id, absent-omitted
        // contract, so the resulting projection is unchanged.
        let projection = column
            .iter()
            .filter_map(|(doc_id, value)| {
                data.reverse_document_ids
                    .get(&doc_id)
                    .map(|public_id| (public_id.to_string(), value.to_owned()))
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
            .and_then(|data| data.parsed_source(id).map(|source| (*source).clone()))
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
                // #15: full scan re-parses each stored `_source` (admin/agg
                // path; the hot top-K path parses only the window).
                data.documents.keys().filter_map(move |id| {
                    data.parsed_source(id).map(|source| StoredDocument {
                        index: index.to_owned(),
                        id: id.to_string(),
                        source,
                    })
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
            .map(|(id, blob)| StoredDocument {
                index: index.to_owned(),
                id: id.to_string(),
                // #15 + option B + `mmap M1` : parse le `_source`
                // stocke (OnDisk -> pread, Compressed -> decode
                // thread-local) en `Value` ; cette voie ne traite que
                // la fenetre `[from..from+size)`.
                source: Arc::new(parse_source_blob(blob, &data.source_store)),
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
                data.parsed_source(id).map(|source| StoredDocument {
                    index: index.to_owned(),
                    id: id.clone(),
                    source,
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

    /// Fused single-token conjunction scoring — the deces `bool`/`full` tail
    /// closer (campaign #18). `clauses` are the required single-clause `(field,
    /// raw_value)` leaves of the deces shape (extracted by
    /// `reduce_deces_conjunction` in `search.rs`); every leaf must match.
    ///
    /// `run_topk_exact_bool` otherwise resolves the intersection, throws the
    /// `freq` away, then re-derives it per candidate per term with a
    /// `binary_search` O(log df) inside `bm25_field_score` — the measured CPU
    /// tail. This walks the intersection ONCE over the SoA `doc_ids`/`freqs`
    /// slices (Lot C Phase 1 levier 5; driver = rarest term, galloping cursors
    /// over the rest), capturing each term's `freq` at the matched position
    /// (O(1), `freqs[idx]`, same index the galloping cursor just landed on in
    /// `doc_ids`) and accumulating the BM25 sum inline.
    ///
    /// Returns `Some(scored)` — one `(score, doc_id)` per intersected candidate,
    /// ascending — **bit-identical** to `score_for_query` for this shape:
    /// `score = if S > 0 { S } else { 1.0 }`, `S = Σ_term bm25(term)` dropping a
    /// term scoring exactly `1.0` (the generic `should` sum's placeholder
    /// filter). `Some(empty)` means the intersection is empty. Returns `None` to
    /// DECLINE — a custom search-analyzer field (its recall token differs from
    /// its scoring token, so a captured `freq` would not match the generic
    /// scorer) or a value that is not exactly one token — so the caller falls
    /// back to the generic, oracle-equivalent path.
    pub fn fused_conjunction_scores(
        &self,
        index: &str,
        clauses: &[(&str, &str)],
    ) -> Option<Vec<(f64, u32)>> {
        if clauses.is_empty() {
            return None;
        }
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let data = store.indices.get(index)?;

        // Per-term: field stats + the SoA doc_ids/freqs channels (Lot C Phase 1
        // levier 5 — `doc_id` is only ever stored once, in `doc_ids`; `freqs` is
        // index-aligned with it so the matched `freq` is still an O(1) lookup at
        // the same index the galloping cursor landed on). `doc_freq ==
        // doc_ids.len()` (one posting per doc), matching `bm25_field_score`'s
        // `term_stats.doc_freq`.
        struct TermCtx<'a> {
            field_stats: FieldScoringStats<'a>,
            doc_freq: u64,
            doc_ids: &'a [u32],
            freqs: &'a [u32],
            roaring: Option<&'a RoaringDocSet>,
        }
        let mut terms: Vec<TermCtx<'_>> = Vec::with_capacity(clauses.len());
        for &(field, value) in clauses {
            // The generic path RECALLS with `normalized_terms_for_field` (custom
            // search analyzer / search_analyzer / custom_analyzer, else the
            // builtin) but SCORES with `analyzer(field).terms` (the field's main
            // analyzer). The fused walk captures the recall token's freq AND
            // scores it, so it is bit-identical only when the two normalisations
            // agree on a SINGLE token — true for builtin and custom-MAIN
            // analyzers (the deces French folding), false when a separate search
            // analyzer is set (then decline → generic path, the parity ref).
            let recall = normalized_terms_for_field(value, field, &data.mapping);
            if recall.len() != 1 || recall != data.mapping.analyzer(field).terms(value) {
                return None;
            }
            let token = recall.into_iter().next().expect("len checked == 1");
            let list = match data.index.postings_with_block_metas(field, &token) {
                Some(list) if !list.doc_ids().is_empty() => list,
                // A required term with no postings ⇒ the intersection is empty.
                _ => return Some(Vec::new()),
            };
            let field_stats = data.field_scoring_stats(field)?;
            terms.push(TermCtx {
                field_stats,
                doc_freq: list.doc_ids().len() as u64,
                doc_ids: list.doc_ids(),
                freqs: list.freqs(),
                roaring: list.roaring(),
            });
        }

        // Drive the rarest term; gallop the others.
        terms.sort_by_key(|t| t.doc_ids.len());
        let config = Bm25Config::default();

        // Monotonic galloping freq lookup over a term's SoA doc_ids/freqs: the
        // matched `freq` for `doc_id`, advancing `cursor` forward (ascending
        // `doc_id` callers only). Branchless `partition_point` like the walk.
        let freq_at = |doc_ids: &[u32], freqs: &[u32], cursor: &mut usize, doc_id: u32| -> u32 {
            let rest = &doc_ids[(*cursor).min(doc_ids.len())..];
            let mut hi = 1usize;
            while hi < rest.len() && rest[hi] < doc_id {
                hi <<= 1;
            }
            let lo = hi >> 1;
            let hi = hi.min(rest.len());
            let offset = lo + rest[lo..hi].partition_point(|&d| d < doc_id);
            *cursor += offset;
            match doc_ids.get(*cursor) {
                Some(&found) if found == doc_id => freqs[*cursor],
                _ => 0,
            }
        };

        // One term's BM25 contribution — replicates `bm25_field_score` exactly
        // (same `bm25_score` primitive + guards) but with the captured `freq`,
        // and folds in the generic `should` sum's `!= 1.0` placeholder filter.
        let term_contrib = |t: &TermCtx<'_>, doc_id: u32, freq: u32| -> f64 {
            if freq == 0 || t.doc_freq == 0 || t.doc_freq > t.field_stats.doc_count {
                return 0.0;
            }
            let doc_len = if t.field_stats.norms_enabled {
                match t.field_stats.doc_len(doc_id) {
                    Some(len) => len,
                    None => return 0.0,
                }
            } else {
                1
            };
            match bm25_score(
                config,
                t.field_stats.doc_count,
                t.doc_freq,
                u64::from(freq),
                doc_len,
                t.field_stats.avg_doc_len,
            ) {
                Ok(score) if score != 1.0 => score,
                _ => 0.0,
            }
        };

        // A1 fast path: two high-`df` terms (the deces bool/full tail) whose
        // intersection is a dense posting-list overlap. AND their precomputed
        // roaring bitmaps word-parallel (`u64 & u64`, 64 doc_ids per op) instead
        // of the scalar O(df_rare) walk, then capture each term's `freq` with a
        // monotonic galloping sweep. Bit-identical to the walk below: the
        // intersection set is identical (roaring module is oracle-tested), the
        // `freq` is the same `freqs[idx]`, and the score formula is the same
        // `term_contrib` sum with the `!= 1.0` filter.
        if terms.len() == 2 {
            if let (Some(r0), Some(r1)) = (terms[0].roaring, terms[1].roaring) {
                let mut result: Vec<u32> = Vec::new();
                r0.intersect_for_each(r1, |doc_id| result.push(doc_id));
                let mut scored: Vec<(f64, u32)> = Vec::with_capacity(result.len());
                let (mut c0, mut c1) = (0usize, 0usize);
                for doc_id in result {
                    let f0 = freq_at(terms[0].doc_ids, terms[0].freqs, &mut c0, doc_id);
                    let f1 = freq_at(terms[1].doc_ids, terms[1].freqs, &mut c1, doc_id);
                    let sum =
                        term_contrib(&terms[0], doc_id, f0) + term_contrib(&terms[1], doc_id, f1);
                    scored.push((if sum > 0.0 { sum } else { 1.0 }, doc_id));
                }
                return Some(scored);
            }
        }

        // Follower galloping cursors over the SoA doc_ids/freqs. Monotonic:
        // driver doc_ids ascend strictly (one posting per doc), so each cursor
        // only moves forward.
        let mut cursors: Vec<usize> = vec![0; terms.len()];
        let mut scored: Vec<(f64, u32)> = Vec::new();
        'docs: for (driver_idx, &doc_id) in terms[0].doc_ids.iter().enumerate() {
            let mut sum = term_contrib(&terms[0], doc_id, terms[0].freqs[driver_idx]);
            for i in 1..terms.len() {
                let follower = &terms[i];
                let pos = cursors[i];
                let rest = &follower.doc_ids[pos..];
                // Galloping (exponential bound) + branchless `partition_point`:
                // first posting with `doc_id >= target`.
                let mut hi = 1usize;
                while hi < rest.len() && rest[hi] < doc_id {
                    hi <<= 1;
                }
                let lo = hi >> 1;
                let hi = hi.min(rest.len());
                let offset = lo + rest[lo..hi].partition_point(|&d| d < doc_id);
                cursors[i] = pos + offset;
                match follower.doc_ids.get(cursors[i]) {
                    Some(&found) if found == doc_id => {
                        sum += term_contrib(follower, doc_id, follower.freqs[cursors[i]]);
                    }
                    // Absent from this follower ⇒ not in the intersection.
                    _ => continue 'docs,
                }
            }
            scored.push((if sum > 0.0 { sum } else { 1.0 }, doc_id));
        }
        Some(scored)
    }

    /// Candidate set of a `should`-all-required conjunction of `match` clauses
    /// WITHOUT materialising any clause's full token union — the deces bool/full
    /// tail (#20: `posting_candidate_ids` spent ~7.5ms p95 building the
    /// `jean ∪ pierre` BTreeSet for a compound prénom, only to intersect down to
    /// ≤10 docs). Each clause is `(field, value, require_all_terms)`; a doc
    /// matches a clause when it has ANY token (OR) / ALL tokens (AND).
    ///
    /// Drives the clause with the smallest size estimate (∑df for OR, min df for
    /// AND), materialises ONLY its matching docs, then keeps those that are
    /// members of every other clause (a `binary_search` per token over the
    /// ascending `doc_id` channel — no union allocation). Returns the matching
    /// doc ids ascending, bit-identical to the intersection-of-unions.
    pub fn conjunction_of_matches(
        &self,
        index: &str,
        clauses: &[(&str, &str, bool)],
    ) -> Option<Vec<u32>> {
        if clauses.is_empty() {
            return None;
        }
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let data = store.indices.get(index)?;

        // Per clause: its tokens' ascending doc_id slices + the operator.
        struct ClauseTokens<'a> {
            require_all: bool,
            token_doc_ids: Vec<&'a [u32]>,
        }
        let mut clause_tokens: Vec<ClauseTokens<'_>> = Vec::with_capacity(clauses.len());
        for &(field, value, require_all) in clauses {
            let tokens = normalized_terms_for_field(value, field, &data.mapping);
            if tokens.is_empty() {
                return Some(Vec::new());
            }
            let mut token_doc_ids: Vec<&[u32]> = Vec::with_capacity(tokens.len());
            for token in &tokens {
                match data.index.postings_with_block_metas(field, token) {
                    Some(list) => token_doc_ids.push(list.doc_ids()),
                    // Absent token: AND ⇒ the clause (hence the conjunction) is
                    // empty; OR ⇒ this token simply contributes nothing.
                    None if require_all => return Some(Vec::new()),
                    None => {}
                }
            }
            if token_doc_ids.is_empty() {
                // Every token of an OR clause is absent ⇒ matches nothing.
                return Some(Vec::new());
            }
            clause_tokens.push(ClauseTokens {
                require_all,
                token_doc_ids,
            });
        }

        // Size estimate: OR ≈ ∑df (union upper bound), AND ≈ min df.
        let estimate = |c: &ClauseTokens<'_>| -> usize {
            if c.require_all {
                c.token_doc_ids.iter().map(|s| s.len()).min().unwrap_or(0)
            } else {
                c.token_doc_ids.iter().map(|s| s.len()).sum()
            }
        };
        let driver_idx = (0..clause_tokens.len())
            .min_by_key(|&i| estimate(&clause_tokens[i]))
            .expect("clause_tokens is non-empty");

        // Does `doc_id` match clause `c`? (binary_search per token, no alloc.)
        let clause_contains = |c: &ClauseTokens<'_>, doc_id: u32| -> bool {
            if c.require_all {
                c.token_doc_ids
                    .iter()
                    .all(|s| s.binary_search(&doc_id).is_ok())
            } else {
                c.token_doc_ids
                    .iter()
                    .any(|s| s.binary_search(&doc_id).is_ok())
            }
        };

        // Materialise ONLY the driver clause's matching docs (ascending, unique).
        let driver = &clause_tokens[driver_idx];
        let driver_docs: Vec<u32> = if driver.token_doc_ids.len() == 1 {
            // Single token: the posting list IS the matching set, already sorted.
            driver.token_doc_ids[0].to_vec()
        } else if driver.require_all {
            let mut acc: BTreeSet<u32> = driver.token_doc_ids[0].iter().copied().collect();
            for s in &driver.token_doc_ids[1..] {
                let next: BTreeSet<u32> = s.iter().copied().collect();
                acc.retain(|d| next.contains(d));
            }
            acc.into_iter().collect()
        } else {
            let mut acc: BTreeSet<u32> = BTreeSet::new();
            for s in &driver.token_doc_ids {
                acc.extend(s.iter().copied());
            }
            acc.into_iter().collect()
        };

        // Keep driver docs that are members of every OTHER clause.
        let out: Vec<u32> = driver_docs
            .into_iter()
            .filter(|&doc_id| {
                clause_tokens
                    .iter()
                    .enumerate()
                    .all(|(i, c)| i == driver_idx || clause_contains(c, doc_id))
            })
            .collect();
        Some(out)
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

    /// Hydrate documents carrying their internal `doc_id` (see
    /// `IndexData::documents_with_internal_ids`) so the scoring path skips the
    /// per-doc public-`_id` round-trip.
    pub fn documents_with_internal_ids(
        &self,
        index: &str,
        internal_ids: &[u32],
    ) -> Vec<(u32, StoredDocument)> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store.indices.get(index).map_or_else(Vec::new, |data| {
            data.documents_with_internal_ids(index, internal_ids)
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
        // `mmap M1` + option B : `_source` est soit `OnDisk { offset,
        // length }` (bytes dans le segment file-backed, RAM 0 — la
        // pression RSS effective est le page-cache OS), soit
        // `Compressed(Arc<[u8]>)` (bytes deflate en heap). La gauge
        // `stored_fields_bytes` ne compte que le RAM heap : 0 pour
        // OnDisk, `Arc<[u8]>::len()` pour Compressed. Sur deces
        // post-bulk avant refresh : ~0 MiB (tout est OnDisk).
        // Post-refresh : ~400 MiB compresses. Avant `mmap M1` la
        // meme gauge valait 1187 MiB en RAM avant refresh.
        usage.stored_fields_bytes = data
            .documents
            .values()
            .map(|blob| blob.payload_len() as u64)
            .sum();
        Some(usage)
    }

    /// API-side state overhead for `index` — the bytes NOT already counted by
    /// [`index_memory_usage`]. Returns `(documents_overhead, id_maps)` where
    /// `documents_overhead` is the per-entry overhead of `documents:
    /// BTreeMap<Arc<str>, SourceBlob>` ON TOP OF the `_source` payload
    /// (already reported via `stored_fields_bytes`): the UID's heap bytes,
    /// plus the `Arc` control-block header, l'enum discriminant + payload du
    /// `SourceBlob` (16 octets en inline pour `OnDisk { u64, u32 }`, 16
    /// octets `Arc<[u8]>` header pour `Compressed`), et un coût
    /// `BTreeMap` approximatif par entrée. `id_maps` couvre `document_ids`
    /// et `reverse_document_ids`.
    ///
    /// Lot C Phase 1 levier 3 : les 3 maps (`documents`, `document_ids`,
    /// `reverse_document_ids`) partagent le MEME `Arc<str>` par UID (voir
    /// [`InMemoryIndex::upsert_document_deferred`]) — le buffer UTF-8 de
    /// l'UID + son en-tête `Arc` (strong/weak `AtomicUsize`, 16 octets) ne
    /// doivent donc être comptés QU'UNE SEULE FOIS, via `documents` (la map
    /// qui existe 1:1 avec les deux autres, y compris sur les updates —
    /// cf. le commentaire de `upsert_document_deferred` sur la réutilisation
    /// de l'`Arc` existant). `document_ids` et `reverse_document_ids` ne
    /// portent plus, chacun, qu'un HANDLE vers ce même buffer : un fat
    /// pointer `Arc<str>` (ptr + len, 16 octets) stocké inline dans le
    /// nœud `BTreeMap`, à la place de l'ancienne `String` indépendante
    /// (24 octets ptr+len+cap + ses propres octets UTF-8 heap-alloués).
    /// Ce fat pointer plus petit reste absorbé dans le lump-sum
    /// `BTREE_NODE_OVERHEAD` (déjà une approximation grossière qui ne
    /// détaillait pas la taille inline de la clé/valeur) — inutile de
    /// rajouter un terme fixe dédié. Ce qui compte est la SUPPRESSION du
    /// terme `key.len()`/`value.len()` : compter les octets UTF-8 de l'UID
    /// une deuxième et une troisième fois — comme avant ce levier, quand
    /// chaque map détenait sa propre `String` — sur-compterait la même
    /// mémoire 3× alors qu'elle n'est plus allouée qu'une fois : la gauge
    /// ne refléterait pas le gain réel. Sur le corpus deces (UID matchID
    /// du type `ins_20240113_11_01004_2`, ~22-24 octets), le gain net
    /// attendu sur `documents_overhead + id_maps` est de l'ordre de
    /// `2 × UID_len - ARC_HEADER` ≈ 30-32 octets/document, soit
    /// ~40-45 MiB sur 1,36 M docs pour cette seule gauge — un plancher
    /// conservateur : le gain RÉEL en RSS est plus élevé car cette
    /// approximation ignore l'arrondi par classe de taille de
    /// l'allocateur (jemalloc), qui pénalise davantage les 3 petites
    /// allocations séparées de l'ancien design que l'unique allocation
    /// partagée du nouveau (cf. l'estimation ~90-180 MiB côté RSS réel).
    /// `strong_count`/`weak_count` ne sont PAS des allocations
    /// supplémentaires (juste des compteurs dans l'en-tête déjà compté)
    /// donc non pertinents ici.
    ///
    /// #17b: the structured index gauges only account for ~2.8 GiB of the
    /// inc1 RSS (~7.97 GiB). The remaining ~5.2 GiB is the target of this
    /// helper, complemented by the jemalloc gauges in `stats.rs`.
    pub fn index_state_memory_bytes(&self, index: &str) -> Option<(u64, u64)> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        let data = store.indices.get(index)?;
        // BTreeMap node overhead per entry: rough approximation that lumps the
        // balancing metadata together. The exact figure is implementation
        // defined, but the order of magnitude is enough to compare against
        // jemalloc allocated.
        const BTREE_NODE_OVERHEAD: u64 = 48;
        // `mmap M1` : taille en RAM de `SourceBlob` lui-meme (sans le
        // payload Compressed, qui est compte par `stored_fields_bytes`).
        // L'enum est dimensionne par la plus grande variante : OnDisk
        // = 12 octets (u64 + u32) ; Compressed = 16 octets (Arc<[u8]>
        // fat pointer = ptr + len). Avec discriminant + alignement,
        // l'enum total ≈ 24 octets. On approxime a 24 pour les deux
        // variantes (OnDisk allocate 0 sur le heap, Compressed allocate
        // les bytes deja comptes par la gauge).
        const SOURCE_BLOB_INLINE: u64 = 24;
        // Lot C Phase 1 levier 3 : en-tête `ArcInner` (strong: AtomicUsize
        // + weak: AtomicUsize = 16 octets sur 64 bits) du buffer `Arc<str>`
        // partagé — compté UNE FOIS ici, avec les octets UTF-8 de l'UID.
        // C'est un coût RÉEL et NOUVEAU par rapport à l'ancien `String`
        // (qui n'a pas d'en-tête de comptage de références), donc on
        // l'itemise explicitement au lieu de le laisser dans le lump-sum
        // `BTREE_NODE_OVERHEAD`.
        const ARC_HEADER: u64 = 16;
        let mut documents_overhead: u64 = 0;
        for key in data.documents.keys() {
            documents_overhead = documents_overhead
                .saturating_add(BTREE_NODE_OVERHEAD)
                .saturating_add(SOURCE_BLOB_INLINE)
                .saturating_add(ARC_HEADER)
                .saturating_add(key.len() as u64);
        }
        // Lot C Phase 1 levier 3 : `document_ids` et `reverse_document_ids`
        // ne portent plus qu'un HANDLE (`Arc<str>` fat pointer, 16 octets)
        // vers le buffer déjà compté ci-dessus au lieu d'une `String`
        // indépendante (24 octets ptr+len+cap) — comme l'ancien
        // `BTREE_NODE_OVERHEAD` ne détaillait déjà pas la taille inline de
        // la clé/valeur (lump-sum approximatif, cf. commentaire plus haut),
        // le fat pointer plus petit reste couvert par ce même lump-sum : on
        // ne rajoute PAS de terme fixe supplémentaire ici. Le point qui
        // compte est la SUPPRESSION du terme `key.len()`/`value.len()` —
        // ces deux maps ne recopient plus les octets UTF-8 de l'UID, d'où
        // la baisse de la gauge. Le coût par entrée est désormais CONSTANT
        // (indépendant de la longueur de l'UID), donc une multiplication
        // par `.len()` remplace la boucle O(n) devenue inutile.
        const ID_MAP_ENTRY_OVERHEAD: u64 = BTREE_NODE_OVERHEAD + 4;
        let id_maps = (data.document_ids.len() as u64)
            .saturating_add(data.reverse_document_ids.len() as u64)
            .saturating_mul(ID_MAP_ENTRY_OVERHEAD);
        Some((documents_overhead, id_maps))
    }

    /// P1 mmap M1 + axe disque #19 : taille on-disk effective du segment
    /// `source.dat` (bytes ecrits, hors reserve `posix_fallocate`).
    /// Permet la mesure disque sans modifier le workflow matchID — la
    /// gauge `surch_index_disk_segment_bytes` est capturee par le scrape
    /// `#20` existant qui filtre `surch_index_*`.
    pub fn index_disk_segment_bytes(&self, index: &str) -> Option<u64> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map(|data| data.source_store.bytes_written())
    }

    /// Pic on-disk depuis la creation du segment (jamais reset par la
    /// compaction). C'est la mesure utile pour le scoreboard axe disque
    /// car le scrape #20 arrive APRES `_refresh` qui truncate le segment.
    pub fn index_disk_segment_peak_bytes(&self, index: &str) -> Option<u64> {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map(|data| data.source_store.peak_bytes_written())
    }

    /// Lot C `C1a-batché` : taille on-disk du segment FoR postings SHADOW
    /// (`surch_index_disk_postings_bytes`), ecrit par
    /// `PostingsBuilder::build()` (batché par champ). Comme
    /// `index_memory_usage`, on materialise d'abord le FST en attente
    /// (`ensure_terms_ready`) pour que la gauge reflete le dernier
    /// `build()`, pas une generation perimee.
    pub fn index_disk_postings_bytes(&self, index: &str) -> Option<u64> {
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map(|data| data.index.postings_segment_bytes())
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
