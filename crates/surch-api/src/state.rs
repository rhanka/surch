use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    fs::OpenOptions,
    io::Write,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use fst::{Map, MapBuilder, Streamer};
use rayon::prelude::*;
use serde_json::Value;
use surch_index::{
    document_index::{DocumentIndex, SegmentPostingsAdvance, SegmentPostingsCursor},
    mapping::{AnalysisSettings, FieldMapping, FieldType, IndexMapping},
    memory::{document_index_memory_usage, MemoryUsage},
    postings::{BlockMeta, PostingsBlockSkipIter, PostingsList},
    roaring::RoaringDocSet,
};
use zstd::bulk::{Compressor as ZstdCompressor, Decompressor as ZstdDecompressor};

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
/// `Raw(Arc<str>)` en RAM : c'est `OnDisk { offset, length, codec }`
/// (`codec` ajoute par la tranche 2b, cf. plus bas), un
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
    /// `_source` matchID / BEIR observes (< 1 MiB) ; depuis tranche 2b
    /// (`docs/paper/contre-expertise-2b-source-2026-07-10.md` §D), c'est
    /// la longueur des bytes ECRITS sur `source.dat`, compresses ou non
    /// selon `codec`.
    ///
    /// `codec` est le tag auto-descriptif de l'amendement 3 : chaque blob
    /// se relit correctement quel que soit son codec, y compris dans un
    /// store a codec MIXTE (flip de `SURCH_SOURCE_COMPRESS` en cours de
    /// vie du process). Tient dans le padding existant de la variante
    /// (`offset: u64` + `length: u32` occupe deja 16 o alignes, `codec: u8`
    /// n'agrandit pas `size_of::<SourceBlob>()`).
    OnDisk { offset: u64, length: u32, codec: u8 },
    /// Bytes deflate-bruts produits par `compact_after_refresh()`.
    /// Decode via [`SourceBlob::decode_compressed`] (Decompress thread-local).
    Compressed(Arc<[u8]>),
}

/// Tag codec `u8` de [`SourceBlob::OnDisk`] — amendement 3 du contrat 2b.
/// `0` = bytes JSON bruts (comportement historique, bit-identique).
const SOURCE_CODEC_RAW: u8 = 0;
/// `1` = bytes zstd (niveau [`ZSTD_SOURCE_LEVEL`]), produits quand
/// `SURCH_SOURCE_COMPRESS=1` (cf. [`source_compress_enabled`]).
const SOURCE_CODEC_ZSTD: u8 = 1;

/// Niveau zstd par defaut pour la compression PAR-DOC du `_source`
/// (contrat 2b, amendement 2 : par-doc d'abord, pas de dictionnaire).
/// Niveau 3 = defaut zstd standard, compromis ratio/vitesse valide par la
/// contre-expertise (~2-3 µs/doc sur ~500 o, <5 % d'un coeur a 21,8k doc/s).
const ZSTD_SOURCE_LEVEL: i32 = 3;

/// Flag `SURCH_SOURCE_COMPRESS` (contrat 2b, gate Q5) : lu UNE FOIS par
/// process (mirroir du pattern `OnceLock` etabli par [`densify_budget_docs`]
/// — un flip mid-run n'est pas supporte, coherent avec tous les autres
/// `SURCH_*` du process). `0`/absent/invalide = OFF (comportement actuel
/// bit-identique, tag [`SOURCE_CODEC_RAW`]) ; `1` = ON (tag
/// [`SOURCE_CODEC_ZSTD`]). La lecture (`parse_source_blob`) gere les DEUX
/// tags dans tous les cas, flag ou pas — un store a codec mixte pendant un
/// flip de flag reste valide.
fn source_compress_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("SURCH_SOURCE_COMPRESS")
            .ok()
            .map(|value| value.trim() == "1")
            .unwrap_or(false)
    })
}

/// Flag `SURCH_SOURCE_FETCH_PARALLEL` : lu UNE FOIS par process, comme
/// [`source_compress_enabled`]. `0`/absent/invalide conserve exactement la
/// boucle sequentielle historique ; `1` hydrate en parallele les seuls
/// gagnants top-K. Le `collect` Rayon preserve l'ordre des `internal_ids`,
/// donc l'ordre de sortie reste independant de l'ordre d'achevement des
/// `pread`.
fn source_fetch_parallel_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("SURCH_SOURCE_FETCH_PARALLEL")
            .ok()
            .map(|value| value.trim() == "1")
            .unwrap_or(false)
    })
}

/// Drapeau benchmark-only `SURCH_SOURCE_FETCH_PROFILE`. Absent, `0` ou une
/// valeur invalide = OFF : le chemin historique ne construit alors aucune
/// structure de profil ni n'emet de metrique. `1` active les compteurs et,
/// si `SURCH_SOURCE_FETCH_PROFILE_FILE` est explicite, l'artefact JSONL borne
/// de [`InMemoryIndex::documents_by_internal_ids`].
fn source_fetch_profile_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        let value = std::env::var("SURCH_SOURCE_FETCH_PROFILE").ok();
        source_fetch_profile_enabled_from(value.as_deref())
    })
}

fn source_fetch_profile_enabled_from(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim() == "1")
}

/// Agregat local a UNE hydratation top-K. Il n'est jamais partage entre les
/// workers Rayon : chaque worker produit son propre echantillon, fusionne
/// sequentiellement apres le `collect` indexe. Ainsi, les compteurs restent
/// exacts sans mutex dans le hot path parallele.
#[derive(Default)]
struct SourceFetchProfile {
    requested_ids: u64,
    hits: u64,
    source_bytes: u64,
    decoded_bytes: u64,
    pread_sum: Duration,
    pread_max: Duration,
    decode_zstd_sum: Duration,
    decode_zstd_max: Duration,
    json_parse_sum: Duration,
    json_parse_max: Duration,
    worker_indices: BTreeSet<usize>,
}

impl SourceFetchProfile {
    fn merge(&mut self, other: Self) {
        self.requested_ids += other.requested_ids;
        self.hits += other.hits;
        self.source_bytes += other.source_bytes;
        self.decoded_bytes += other.decoded_bytes;
        self.pread_sum += other.pread_sum;
        self.pread_max = self.pread_max.max(other.pread_max);
        self.decode_zstd_sum += other.decode_zstd_sum;
        self.decode_zstd_max = self.decode_zstd_max.max(other.decode_zstd_max);
        self.json_parse_sum += other.json_parse_sum;
        self.json_parse_max = self.json_parse_max.max(other.json_parse_max);
        self.worker_indices.extend(other.worker_indices);
    }

    fn record_pread(&mut self, elapsed: Duration) {
        self.pread_sum += elapsed;
        self.pread_max = self.pread_max.max(elapsed);
    }

    fn record_zstd_decode(&mut self, elapsed: Duration) {
        self.decode_zstd_sum += elapsed;
        self.decode_zstd_max = self.decode_zstd_max.max(elapsed);
    }

    fn record_json_parse(&mut self, elapsed: Duration) {
        self.json_parse_sum += elapsed;
        self.json_parse_max = self.json_parse_max.max(elapsed);
    }

    fn record_worker(&mut self, worker_index: usize) {
        self.worker_indices.insert(worker_index);
    }

    fn workers_effectifs(&self) -> u64 {
        self.worker_indices.len() as u64
    }
}

const SOURCE_FETCH_PROFILE_MAX_RECORDS: usize = 4096;

/// Ecriture benchmark-only, bornee et serialisee de l'artefact brut L2.
///
/// Le profil est opt-in et le chemin de sortie doit etre explicite : sans ce
/// chemin, aucune ligne n'est ecrite. Le mutex ne couvre qu'une ligne par
/// requete, apres le `collect` Rayon; les workers ne touchent donc jamais le
/// fichier. La borne protege un serveur mal configure d'un fichier sans fin.
struct SourceFetchProfileSink {
    file: std::fs::File,
    records: usize,
}

fn source_fetch_profile_sink() -> Result<&'static Mutex<SourceFetchProfileSink>, ()> {
    static SINK: std::sync::OnceLock<Result<Mutex<SourceFetchProfileSink>, ()>> =
        std::sync::OnceLock::new();
    SINK.get_or_init(|| {
        let path = std::env::var("SURCH_SOURCE_FETCH_PROFILE_FILE")
            .ok()
            .filter(|path| !path.trim().is_empty())
            .ok_or(())?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .map_err(|_| ())?;
        Ok(Mutex::new(SourceFetchProfileSink { file, records: 0 }))
    })
    .as_ref()
    .map_err(|_| ())
}

/// Emet les compteurs bornes qui servent aux gates et une ligne JSONL par
/// hydratation. Les distributions viennent exclusivement du JSONL exact : les
/// summaries Prometheus ne sont ni utilisees ni soustraites entre phases.
fn record_source_fetch_profile(
    mode: &'static str,
    fetch_wall: Duration,
    profile: SourceFetchProfile,
) {
    // Les labels sont bornes : les phases restent dans les fichiers JSONL
    // exportes par le harnais, jamais dans une serie Prometheus par requete.
    let hits_bucket = source_fetch_hits_bucket(profile.hits);
    metrics::counter!(
        "surch_source_fetch_requests_total",
        "mode" => mode,
        "hits_bucket" => hits_bucket,
    )
    .increment(1);
    metrics::counter!(
        "surch_source_fetch_hits_total",
        "mode" => mode,
        "hits_bucket" => hits_bucket,
    )
    .increment(profile.hits);
    metrics::counter!(
        "surch_source_fetch_bytes_total",
        "mode" => mode,
        "hits_bucket" => hits_bucket,
    )
    .increment(profile.source_bytes);
    metrics::counter!(
        "surch_source_fetch_decoded_bytes_total",
        "mode" => mode,
        "hits_bucket" => hits_bucket,
    )
    .increment(profile.decoded_bytes);
    metrics::counter!(
        "surch_source_fetch_requested_ids_total",
        "mode" => mode,
        "hits_bucket" => hits_bucket,
    )
    .increment(profile.requested_ids);
    metrics::counter!(
        "surch_source_fetch_workers_total",
        "mode" => mode,
        "hits_bucket" => hits_bucket,
    )
    .increment(profile.workers_effectifs());

    let Ok(sink) = source_fetch_profile_sink() else {
        metrics::counter!("surch_source_fetch_profile_write_failures_total").increment(1);
        return;
    };
    let mut sink = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if sink.records >= SOURCE_FETCH_PROFILE_MAX_RECORDS {
        metrics::counter!("surch_source_fetch_profile_dropped_total").increment(1);
        return;
    }
    if writeln!(
        sink.file,
        "{{\"mode\":\"{mode}\",\"requested_ids\":{},\"hydrated_hits\":{},\"bytes\":{},\"fetch_wall_us\":{},\"pread_sum_us\":{},\"pread_max_us\":{},\"decode_sum_us\":{},\"decode_max_us\":{},\"json_sum_us\":{},\"json_max_us\":{},\"workers_effectifs\":{}}}",
        profile.requested_ids,
        profile.hits,
        profile.source_bytes,
        fetch_wall.as_micros(),
        profile.pread_sum.as_micros(),
        profile.pread_max.as_micros(),
        profile.decode_zstd_sum.as_micros(),
        profile.decode_zstd_max.as_micros(),
        profile.json_parse_sum.as_micros(),
        profile.json_parse_max.as_micros(),
        profile.workers_effectifs(),
    )
    .is_err()
    {
        metrics::counter!("surch_source_fetch_profile_write_failures_total").increment(1);
        return;
    }
    sink.records += 1;
    metrics::counter!("surch_source_fetch_profile_records_total").increment(1);
}

/// Trois valeurs constantes bornent la cardinalité Prometheus et suffisent
/// aux gates L2 : absence de fetch, petit fetch, ou fenêtre hydratée >= 8.
fn source_fetch_hits_bucket(hits: u64) -> &'static str {
    match hits {
        0 => "zero",
        1..=7 => "one_to_seven",
        _ => "eight_plus",
    }
}

#[cfg(test)]
mod source_fetch_profile_tests {
    use std::time::Duration;

    use super::{source_fetch_profile_enabled_from, SourceFetchProfile};

    #[test]
    fn source_fetch_profile_flag_is_strictly_off_unless_set_to_one() {
        for value in [None, Some(""), Some("0"), Some("true"), Some(" 2 ")] {
            assert!(
                !source_fetch_profile_enabled_from(value),
                "{value:?} must leave the benchmark profile off"
            );
        }
        assert!(source_fetch_profile_enabled_from(Some(" 1 ")));
    }

    #[test]
    fn source_fetch_profile_merges_worker_samples_without_losing_maxima() {
        let mut sequential = SourceFetchProfile {
            requested_ids: 2,
            hits: 1,
            source_bytes: 12,
            decoded_bytes: 20,
            ..SourceFetchProfile::default()
        };
        sequential.record_worker(0);
        sequential.record_pread(Duration::from_micros(7));
        sequential.record_zstd_decode(Duration::from_micros(3));
        sequential.record_json_parse(Duration::from_micros(5));

        let mut worker = SourceFetchProfile {
            hits: 1,
            source_bytes: 8,
            decoded_bytes: 13,
            ..SourceFetchProfile::default()
        };
        worker.record_worker(3);
        worker.record_pread(Duration::from_micros(11));
        worker.record_zstd_decode(Duration::from_micros(2));
        worker.record_json_parse(Duration::from_micros(9));

        sequential.merge(worker);

        assert_eq!(sequential.requested_ids, 2);
        assert_eq!(sequential.hits, 2);
        assert_eq!(sequential.source_bytes, 20);
        assert_eq!(sequential.decoded_bytes, 33);
        assert_eq!(sequential.pread_sum, Duration::from_micros(18));
        assert_eq!(sequential.pread_max, Duration::from_micros(11));
        assert_eq!(sequential.decode_zstd_sum, Duration::from_micros(5));
        assert_eq!(sequential.decode_zstd_max, Duration::from_micros(3));
        assert_eq!(sequential.json_parse_sum, Duration::from_micros(14));
        assert_eq!(sequential.json_parse_max, Duration::from_micros(9));
        assert_eq!(sequential.workers_effectifs(), 2);
    }

    #[test]
    fn source_fetch_profile_bounds_hit_labels_for_l2_gates() {
        assert_eq!(super::source_fetch_hits_bucket(0), "zero");
        assert_eq!(super::source_fetch_hits_bucket(7), "one_to_seven");
        assert_eq!(super::source_fetch_hits_bucket(8), "eight_plus");
        assert_eq!(super::source_fetch_hits_bucket(u64::MAX), "eight_plus");
    }
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

    /// Compresse `raw` en zstd niveau [`ZSTD_SOURCE_LEVEL`] — tranche 2b,
    /// actif quand [`source_compress_enabled`] retourne `true`. Reutilise
    /// un `zstd::bulk::Compressor` thread-local (meme rationale que
    /// `encode_for_compact` : amortir l'init du contexte `CCtx` sur le hot
    /// path bulk, cf. `docs/paper/contre-expertise-2b-source-2026-07-10.md`
    /// §C Q2). `compress` dimensionne lui-meme le buffer de sortie
    /// (`ZSTD_compressBound`), pas de boucle de croissance necessaire ici
    /// (contrairement au deflate `compress_vec` bas niveau).
    fn encode_zstd(raw: &[u8]) -> Vec<u8> {
        thread_local! {
            static COMPRESSOR: RefCell<ZstdCompressor<'static>> = RefCell::new(
                ZstdCompressor::new(ZSTD_SOURCE_LEVEL)
                    .expect("zstd compressor context initialises at a valid level"),
            );
        }
        COMPRESSOR.with(|cell| {
            cell.borrow_mut()
                .compress(raw)
                .expect("zstd compress of a validated _source blob does not fail")
        })
    }

    /// Decompresse un blob zstd ecrit par [`SourceBlob::encode_zstd`].
    /// Reutilise un `zstd::bulk::Decompressor` thread-local (meme
    /// rationale que `decode_compressed`). `Decompressor::decompress` exige
    /// une capacite de sortie explicite (zstd bulk n'auto-agrandit pas) ;
    /// on part d'une heuristique (×4 la taille compressee, plancher 4 KiB —
    /// meme heuristique que le chemin deflate) et on double jusqu'a ce que
    /// ca tienne, borne a 20 tentatives pour eviter une boucle infinie sur
    /// un store corrompu (le round-trip est garanti lossless par
    /// construction — cf. contrat 2b §A6 — donc ce plafond ne devrait
    /// jamais etre atteint en usage normal).
    fn decode_zstd(bytes: &[u8]) -> Vec<u8> {
        thread_local! {
            static DECOMPRESSOR: RefCell<ZstdDecompressor<'static>> = RefCell::new(
                ZstdDecompressor::new().expect("zstd decompressor context initialises"),
            );
        }
        DECOMPRESSOR.with(|cell| {
            let mut decompressor = cell.borrow_mut();
            let mut capacity = bytes.len().saturating_mul(4).max(4096);
            for _ in 0..20 {
                match decompressor.decompress(bytes, capacity) {
                    Ok(out) => return out,
                    Err(_) => capacity = capacity.saturating_mul(2),
                }
            }
            panic!(
                "stored zstd _source blob did not decode within the capacity growth bound \
                 (store contract violation — round-trip is lossless by construction, cf. \
                 contre-expertise-2b-source-2026-07-10.md §A6)"
            );
        })
    }
}

/// Lit et decode les bytes bruts (JSON) d'un blob [`SourceBlob::OnDisk`],
/// quel que soit son `codec` — amendement 3 du contrat 2b : un store a
/// codec MIXTE (flip de `SURCH_SOURCE_COMPRESS` en cours de vie du
/// process) reste toujours relisible, blob par blob.
fn read_on_disk_bytes(store: &SourceStore, offset: u64, length: u32, codec: u8) -> Vec<u8> {
    let bytes = store.read(offset, length);
    match codec {
        SOURCE_CODEC_RAW => bytes,
        SOURCE_CODEC_ZSTD => SourceBlob::decode_zstd(&bytes),
        other => panic!(
            "unknown _source codec tag {other} on OnDisk blob (contrat 2b : raw={SOURCE_CODEC_RAW}, zstd={SOURCE_CODEC_ZSTD})"
        ),
    }
}

/// Variante benchmark-only de [`read_on_disk_bytes`]. Le chronometre encadre
/// exactement [`SourceStore::read`], donc `pread_sum` ne comprend ni decode ni
/// JSON. Elle n'est jamais atteinte quand `SURCH_SOURCE_FETCH_PROFILE` est OFF.
fn read_on_disk_bytes_profiled(
    store: &SourceStore,
    offset: u64,
    length: u32,
    codec: u8,
    profile: &mut SourceFetchProfile,
) -> Vec<u8> {
    let pread_started = Instant::now();
    let bytes = store.read(offset, length);
    profile.record_pread(pread_started.elapsed());
    profile.source_bytes += u64::from(length);

    match codec {
        SOURCE_CODEC_RAW => bytes,
        SOURCE_CODEC_ZSTD => {
            let decode_started = Instant::now();
            let decoded = SourceBlob::decode_zstd(&bytes);
            profile.record_zstd_decode(decode_started.elapsed());
            decoded
        }
        other => panic!(
            "unknown _source codec tag {other} on OnDisk blob (contrat 2b : raw={SOURCE_CODEC_RAW}, zstd={SOURCE_CODEC_ZSTD})"
        ),
    }
}

/// Helper pour parser un [`SourceBlob`] en `Value`. Reutilise par
/// `parsed_source`, `rebuild_index` et `documents_paginated`.
///
/// `mmap M1` : le `store` est lu pour les blobs `OnDisk` (pread sur le
/// segment file-backed, decode selon `codec` via [`read_on_disk_bytes`]) ;
/// les `Compressed` decodent via le `Decompress` thread-local (inchange
/// option B).
fn parse_source_blob(blob: &SourceBlob, store: &SourceStore) -> Value {
    match blob {
        SourceBlob::OnDisk {
            offset,
            length,
            codec,
        } => {
            let bytes = read_on_disk_bytes(store, *offset, *length, *codec);
            serde_json::from_slice(&bytes).expect("stored OnDisk _source is valid JSON")
        }
        SourceBlob::Compressed(bytes) => {
            let decoded = SourceBlob::decode_compressed(bytes.as_ref());
            serde_json::from_slice(&decoded)
                .expect("stored Compressed _source decodes to valid JSON")
        }
    }
}

/// Variante benchmark-only de [`parse_source_blob`]. Elle garde les mesures
/// dans une valeur locale retournable par un worker Rayon; aucune metrique ni
/// synchronisation n'est executee par hit.
fn parse_source_blob_profiled(
    blob: &SourceBlob,
    store: &SourceStore,
) -> (Value, SourceFetchProfile) {
    let mut profile = SourceFetchProfile::default();
    let bytes = match blob {
        SourceBlob::OnDisk {
            offset,
            length,
            codec,
        } => read_on_disk_bytes_profiled(store, *offset, *length, *codec, &mut profile),
        SourceBlob::Compressed(bytes) => {
            profile.source_bytes += bytes.len() as u64;
            let decode_started = Instant::now();
            let decoded = SourceBlob::decode_compressed(bytes.as_ref());
            profile.record_zstd_decode(decode_started.elapsed());
            decoded
        }
    };
    profile.decoded_bytes += bytes.len() as u64;

    let json_started = Instant::now();
    let source = serde_json::from_slice(&bytes).expect("stored _source is valid JSON");
    profile.record_json_parse(json_started.elapsed());
    profile.hits = 1;
    (source, profile)
}

/// C1 fix (design doc `docs/paper/design-segments-pic-borne-2026-07-05.md`
/// §C1): merge-joins `old` (an already-built `fst::Map`, sorted by
/// construction) with `new_entries` (sorted ascending by uid bytes by the
/// caller) into ONE fresh FST, without re-collecting or re-inserting any
/// key `old` already carries. Used by
/// [`InMemoryIndex::densify_append_only`] so a budget-triggered
/// incremental densify does not have to re-materialize every HISTORICAL
/// uid on every call (which [`InMemoryIndex::densify_full`]'s
/// from-scratch rebuild does — an `Arc::from(uid)` heap allocation PER
/// already-densified doc, PER call, and the cost this incremental path
/// exists to avoid).
///
/// The two key sets are DISJOINT by construction, which is what makes a
/// plain merge-join (as opposed to `fst::map::OpBuilder::union`, built
/// for possibly-overlapping sources) correct here:
/// `upsert_document_deferred` only mints a fresh `doc_id` — and therefore
/// only ever adds an uid to `new_entries` — when `resolve_uid` found
/// NOTHING live for that uid. The one case where the SAME uid text could
/// legitimately reappear (delete-then-reinsert) always tombstones the old
/// `doc_id` into `deleted_since_dense` first (see
/// `delete_document_deferred`), which is exactly the condition
/// [`InMemoryIndex::densify`]'s interference check routes to
/// `densify_full` instead — so by the time this function runs, `old`
/// cannot contain any uid also present in `new_entries`.
///
/// `fst::MapBuilder::memory()` is used here (not the streaming-to-file
/// trick applied to `merge_term_dictionaries` in `surch-index`): the
/// id-maps FST covers far fewer distinct bytes than a multi-field term
/// dictionary — one key per doc, heavily prefix-shared on the INSEE
/// corpus's sequential-looking uids (tens of MiB at 28,9M, see the design
/// doc's §C1 diagnostic table) — and this can run far more often (once
/// per budget tranche) than the rare large tiered merge. A future pass
/// can stream this one too if a concrete corpus proves the memory
/// transient matters.
fn merge_forward_fst(
    old: Option<Map<Vec<u8>>>,
    new_entries: &[(Arc<str>, u32)],
) -> Option<Map<Vec<u8>>> {
    if new_entries.is_empty() {
        // Nothing new to fold in — whatever `old` already is (`Some` or
        // `None`) is still the correct answer, no rebuild needed.
        return old;
    }
    let mut builder = MapBuilder::memory();
    let mut next_new = 0usize;
    if let Some(old_map) = &old {
        let mut stream = old_map.stream();
        while let Some((key, value)) = stream.next() {
            while next_new < new_entries.len() && new_entries[next_new].0.as_bytes() < key {
                let (uid, doc_id) = &new_entries[next_new];
                builder.insert(uid.as_bytes(), u64::from(*doc_id)).expect(
                    "new_entries sorted ascending and disjoint from `old` — \
                         this key sorts strictly before the old FST's next key",
                );
                next_new += 1;
            }
            builder
                .insert(key, value)
                .expect("fst::Map::stream re-emits its own keys in strictly ascending order");
        }
    }
    for (uid, doc_id) in &new_entries[next_new..] {
        builder
            .insert(uid.as_bytes(), u64::from(*doc_id))
            .expect("new_entries sorted ascending by the caller, disjoint from `old`");
    }
    let bytes = builder
        .into_inner()
        .expect("fst::MapBuilder memory writer never fails I/O");
    Some(Map::new(bytes).expect("fst::Map from valid MapBuilder bytes"))
}

use surch_search::scoring::{bm25_score, Bm25Config};

use crate::scroll::ScrollTable;
use crate::stats::{clear_memory_gauges, refresh_jemalloc_purge, refresh_memory_gauges};

#[derive(Clone, Copy, PartialEq, Eq)]
enum FusedConjunctionScoreMode {
    Should,
    Must,
}

fn fused_bm25_contribution(score: Option<f64>, mode: FusedConjunctionScoreMode) -> f64 {
    match mode {
        FusedConjunctionScoreMode::Should => score.filter(|score| *score != 1.0).unwrap_or(0.0),
        FusedConjunctionScoreMode::Must => score.filter(|score| *score > 0.0).unwrap_or(1.0),
    }
}

/// Accumule les blocs FoR observés pendant UNE tentative P2. Les compteurs
/// sont aussi publiés sur les replis : une erreur ou un segment sans toutes
/// les clauses ne doit pas faire disparaître le travail disque déjà effectué.
#[derive(Default)]
struct P2DiskBlockMetrics {
    blocks_read: u64,
    blocks_total: u64,
}

impl P2DiskBlockMetrics {
    fn observe(&mut self, cursors: &[&SegmentPostingsCursor<'_>]) {
        for cursor in cursors {
            if let Some((read, total)) = cursor.disk_block_counts() {
                self.blocks_read = self.blocks_read.saturating_add(read);
                self.blocks_total = self.blocks_total.saturating_add(total);
            }
        }
    }

    fn publish(&self) {
        if self.blocks_total > 0 {
            metrics::counter!("surch_postings_disk_blocks_read_total").increment(self.blocks_read);
            metrics::counter!("surch_postings_disk_blocks_total").increment(self.blocks_total);
        }
    }
}

#[cfg(test)]
mod p1a_fused_must_tests {
    use super::{fused_bm25_contribution, FusedConjunctionScoreMode};

    #[test]
    fn must_conserve_un_score_bm25_unitaire() {
        assert_eq!(
            fused_bm25_contribution(Some(1.0), FusedConjunctionScoreMode::Must),
            1.0
        );
        assert_eq!(
            fused_bm25_contribution(Some(1.0), FusedConjunctionScoreMode::Should),
            0.0
        );
    }
}

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
    entries: HashMap<u64, SearchCacheEntry>,
    order: VecDeque<u64>,
}

/// Réponse mémorisée avec sa provenance P1a. Le cache ne relance jamais le
/// scorer : cette donnée conserve la sémantique du compteur lorsqu'une
/// réponse RAM directe est servie depuis le cache.
#[derive(Clone)]
struct SearchCacheEntry {
    bytes: Vec<u8>,
    direct_must_fused: bool,
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
/// Largeur en bits de chaque champ du `u64` packe de
/// [`DenseIdMaps::documents`] (plan 2c,
/// `docs/paper/plan-packing-sidetable-2026-07-12.md`) : `[codec:1]
/// [length:23][offset:40]`, MSB -> LSB.
/// - `offset` (40 bits) = 1 Tio adressables dans `source.dat` — a 28,9M
///   docs on mesure ~15 Go orphelins compris, marge 64×.
/// - `length` (23 bits) = 8 Mio/doc max — les `_source` observes
///   (matchID/BEIR) font < 1 Mio ; l'ancien contrat `u32` (4 Gio) etait
///   deja theorique.
/// - `codec` (1 bit) = les 2 valeurs actuelles ([`SOURCE_CODEC_RAW`] /
///   [`SOURCE_CODEC_ZSTD`]). Un 3e codec grignotera un bit de `length`
///   ou d'`offset` — a trancher au moment du dictionnaire zstd, pas
///   avant (cf. le plan).
const PACKED_OFFSET_BITS: u32 = 40;
const PACKED_LENGTH_BITS: u32 = 23;
const PACKED_CODEC_BITS: u32 = 1;
const _: () = assert!(PACKED_OFFSET_BITS + PACKED_LENGTH_BITS + PACKED_CODEC_BITS == 64);

const PACKED_OFFSET_SHIFT: u32 = 0;
const PACKED_LENGTH_SHIFT: u32 = PACKED_OFFSET_BITS;
const PACKED_CODEC_SHIFT: u32 = PACKED_OFFSET_BITS + PACKED_LENGTH_BITS;

/// Offset maximal representable (2^40 − 1, soit 1 Tio − 1 o).
const PACKED_OFFSET_MAX: u64 = (1u64 << PACKED_OFFSET_BITS) - 1;
/// Length maximale representable (2^23 − 1, soit 8 Mio − 1 o).
const PACKED_LENGTH_MAX: u32 = (1u32 << PACKED_LENGTH_BITS) - 1;

/// Empaquete un pointeur `OnDisk { offset, length, codec }` en un `u64`
/// pour un slot de [`DenseIdMaps::documents`] (plan 2c). `length == 0`
/// est reserve au trou ([`unpack_document_slot`]) — jamais atteint pour
/// un `_source` reel (meme `{}` serialise deja sur 2 octets).
///
/// Panique plutot que de tronquer silencieusement : un doc dont le
/// `_source` (compresse ou non) depasse 8 Mio, ou un `source.dat` qui
/// depasse 1 Tio, sortent du contrat de cette side-table packee. Le plan
/// prevoit une `HashMap<u32, SourceBlob>` d'exceptions comme escape-hatch
/// si ce cas se presente un jour — non implementee ici, pas encore
/// necessaire (aucun `_source` deces/matchID/BEIR n'approche 8 Mio).
fn pack_document_slot(offset: u64, length: u32, codec: u8) -> u64 {
    assert!(
        length <= PACKED_LENGTH_MAX,
        "doc _source > 8 MiB non supporté par la side-table packée"
    );
    assert!(
        offset <= PACKED_OFFSET_MAX,
        "offset _source > 1 TiB non supporté par la side-table packée"
    );
    debug_assert!(
        codec <= 1,
        "codec _source doit tenir sur 1 bit (0=raw, 1=zstd) — voir \
         SOURCE_CODEC_RAW/SOURCE_CODEC_ZSTD"
    );
    ((u64::from(codec) & 0x1) << PACKED_CODEC_SHIFT)
        | (u64::from(length) << PACKED_LENGTH_SHIFT)
        | (offset << PACKED_OFFSET_SHIFT)
}

/// Deballe un slot packe en `(offset, length, codec)`, ou `None` si c'est
/// un trou (`length == 0` — doc supprime avant ce densify, cf. le champ
/// `documents` de [`DenseIdMaps`]).
fn unpack_document_slot(packed: u64) -> Option<(u64, u32, u8)> {
    let length = ((packed >> PACKED_LENGTH_SHIFT) & u64::from(PACKED_LENGTH_MAX)) as u32;
    if length == 0 {
        return None;
    }
    let offset = (packed >> PACKED_OFFSET_SHIFT) & PACKED_OFFSET_MAX;
    let codec = (packed >> PACKED_CODEC_SHIFT) as u8;
    Some((offset, length, codec))
}

/// Empaquete un [`SourceBlob`] vivant du chemin dense en `u64` (plan 2c).
/// Seul `OnDisk` peut apparaitre ici : depuis `mmap M1`, aucun chemin de
/// densify ne peut produire de `Compressed` — `compact_after_refresh`,
/// seul producteur de `Compressed`, n'est plus appele nulle part (voir le
/// "Constat structurel" #1 du plan). `unreachable!` documente ce contrat
/// plutot que de tronquer silencieusement un cas cense impossible.
fn pack_source_blob(blob: &SourceBlob) -> u64 {
    match blob {
        SourceBlob::OnDisk {
            offset,
            length,
            codec,
        } => pack_document_slot(*offset, *length, *codec),
        SourceBlob::Compressed(_) => unreachable!(
            "densify ne doit jamais voir de SourceBlob::Compressed sur le chemin dense \
             (mmap M1 : compact_after_refresh n'est plus appele — plan 2c, \
             docs/paper/plan-packing-sidetable-2026-07-12.md, Constat structurel #1)"
        ),
    }
}

/// Lot C `C2` : snapshot immuable et dense des 3 anciennes `BTreeMap`
/// (`documents`, `document_ids`, `reverse_document_ids`), materialise a
/// chaque `_refresh` par [`InMemoryIndex::densify`]. Remplace jusqu'a
/// ~1,36 M allocations `Arc<str>` + nœuds `BTreeMap` individuelles (la
/// source de fragmentation interne mesuree — cf. commit "gauges internes
/// jemalloc pour expliquer le gap heap anon") par des buffers contigus :
/// - `forward` : FST `uid -> doc_id`. Zero allocation par UID — les
///   octets sont encodes dans le graphe FST partage-prefixe (meme
///   crate/pattern que `TermDictionary`, cf. `surch-index::postings`).
/// - `reverse_uids` / `reverse_offsets` : `doc_id -> uid` empaquetes en
///   UN buffer + une table d'offsets CSR (`len = doc_count + 1`), indexee
///   directement par `doc_id`. Un "trou" (doc_id supprime avant ce
///   densify) est encode par `offsets[i] == offsets[i+1]` (span vide) —
///   sans collision possible avec un UID legitime car `document_handler`
///   rejette tout id vide en amont (`document.rs::document_handler`:
///   "document id must not be empty").
/// - `documents` : `_source` indexe par `doc_id`, empaquete en `u64` (plan
///   2c, `docs/paper/plan-packing-sidetable-2026-07-12.md` — remplace
///   l'ancien `Vec<Option<SourceBlob>>` ~24 o/slot par 8 o/slot, −433 Mio
///   anon a 28,9M docs). Encodage `[codec:1][length:23][offset:40]`
///   (MSB -> LSB), trou = `length == 0` : voir [`pack_document_slot`] /
///   [`unpack_document_slot`] ci-dessus.
///
/// `doc_id` n'est **jamais reutilise** (`InMemoryIndex::next_doc_id` ne
/// fait qu'incrementer ; un uid absent — y compris un uid precedemment
/// supprime — reçoit toujours un id neuf, voir
/// [`InMemoryIndex::upsert_document_deferred`]) : un trou reste un trou a
/// vie, il n'est jamais "re-rempli" en place, seulement omis du PROCHAIN
/// snapshot dense.
///
/// C1 fix (revue independante, design doc §C1) : `reverse_uids`/
/// `reverse_offsets`/`documents` etaient des `Box<[T]>` exact-fit (zero
/// slack, optimal en steady-state) — mais `Box<[T]>::into_vec()` PUIS
/// `Vec::into_boxed_slice()` (le cycle que `InMemoryIndex::densify_full`/
/// `densify_append_only` faisait a CHAQUE appel) shrink-to-fit
/// SYSTEMATIQUEMENT la capacite a la longueur exacte a la fin de chaque
/// appel, ce qui annule tout le benefice de la croissance amortie d'un
/// `Vec` : la tranche SUIVANTE re-declenche une reallocation+copie de la
/// TOTALITE du buffer courant (pas seulement la tranche neuve), rendant
/// `densify_append_only` cumulativement O(N²/tranche) au lieu de O(N)
/// amorti — feedback direct de la revue Opus sur ce fix (voir le doc
/// design §C1 fix). `Vec<T>` (sans `into_boxed_slice()` final) laisse la
/// capacite geometrique-croissante de `Vec` survivre d'un appel au
/// suivant : la MAJORITE des tranches n'ont alors PLUS besoin de
/// reallouer du tout (la capacite residuelle suffit), au prix d'un peu de
/// slack resident en steady-state (borne, pas illimite) — echange
/// clairement favorable pour borner le PIC transitoire, l'objectif de ce
/// fix. Les accesseurs `uid`/`blob`/`doc_count` ci-dessous sont
/// INCHANGES : `Vec<T>` deref vers `&[T]`, `.len()`/`.get()`/l'indexation
/// se comportent identiquement (bornes sur la longueur LOGIQUE, jamais la
/// capacite).
#[derive(Debug, Default)]
struct DenseIdMaps {
    forward: Option<Map<Vec<u8>>>,
    reverse_uids: Vec<u8>,
    reverse_offsets: Vec<u32>,
    documents: Vec<u64>,
}

impl DenseIdMaps {
    /// Nombre de `doc_id` couverts par ce snapshot (vivants ou trous).
    fn doc_count(&self) -> u32 {
        self.reverse_offsets.len().saturating_sub(1) as u32
    }

    /// UID pour `doc_id` ; `None` si hors bornes ou trou (doc supprime
    /// avant ce densify).
    fn uid(&self, doc_id: u32) -> Option<&str> {
        let idx = doc_id as usize;
        let start = *self.reverse_offsets.get(idx)?;
        let end = *self.reverse_offsets.get(idx + 1)?;
        if start == end {
            return None;
        }
        std::str::from_utf8(&self.reverse_uids[start as usize..end as usize]).ok()
    }

    /// `_source` stocke pour `doc_id` ; `None` si hors bornes ou trou.
    /// Retourne PAR VALEUR depuis le plan 2c : `OnDisk` est reconstruit a
    /// partir du slot packe ([`unpack_document_slot`]), copie triviale de
    /// scalaires, aucun cout au-dela du deballage lui-meme.
    fn blob(&self, doc_id: u32) -> Option<SourceBlob> {
        let packed = *self.documents.get(doc_id as usize)?;
        let (offset, length, codec) = unpack_document_slot(packed)?;
        Some(SourceBlob::OnDisk {
            offset,
            length,
            codec,
        })
    }

    /// `doc_id` pour `uid` dans CE snapshot, sans tenir compte des
    /// suppressions posterieures — voir [`InMemoryIndex::resolve_uid`]
    /// pour la resolution complete (dirty + tombstones + dense).
    fn forward_get(&self, uid: &str) -> Option<u32> {
        self.forward
            .as_ref()?
            .get(uid.as_bytes())
            .map(|value| value as u32)
    }
}

#[cfg(test)]
mod packed_document_slot_tests {
    use super::{
        pack_document_slot, unpack_document_slot, PACKED_LENGTH_MAX, PACKED_OFFSET_MAX,
        SOURCE_CODEC_RAW, SOURCE_CODEC_ZSTD,
    };

    /// Round-trip : ce que `pack` encode, `unpack` le retrouve a
    /// l'identique, pour les deux codecs.
    #[test]
    fn round_trip_raw_and_zstd() {
        for (offset, length, codec) in [
            (0u64, 2u32, SOURCE_CODEC_RAW),
            (1_234_567u64, 512u32, SOURCE_CODEC_ZSTD),
            (PACKED_OFFSET_MAX, PACKED_LENGTH_MAX, SOURCE_CODEC_ZSTD),
        ] {
            let packed = pack_document_slot(offset, length, codec);
            assert_eq!(
                unpack_document_slot(packed),
                Some((offset, length, codec)),
                "round-trip failed for offset={offset} length={length} codec={codec}"
            );
        }
    }

    /// Un trou (`length == 0`, quel que soit `offset`) deballe en `None` —
    /// c'est la convention qui remplace l'ancien `Option::None` du
    /// `Vec<Option<SourceBlob>>`.
    #[test]
    fn hole_is_length_zero() {
        assert_eq!(unpack_document_slot(0), None);
        // `offset` non nul mais `length == 0` reste un trou — seul
        // `length` porte la semantique de trou, l'`offset` est ignore.
        let packed = pack_document_slot(999, 0, SOURCE_CODEC_RAW);
        assert_eq!(unpack_document_slot(packed), None);
    }

    /// Bornes maximales : `offset`/`length` a leur plafond exact
    /// (2^40 − 1 / 2^23 − 1) round-trippent sans troncature ni panic.
    #[test]
    fn max_bounds_round_trip() {
        let packed = pack_document_slot(PACKED_OFFSET_MAX, PACKED_LENGTH_MAX, SOURCE_CODEC_ZSTD);
        assert_eq!(
            unpack_document_slot(packed),
            Some((PACKED_OFFSET_MAX, PACKED_LENGTH_MAX, SOURCE_CODEC_ZSTD))
        );
    }

    /// `length` au-dela de 2^23 − 1 (8 Mio) doit paniquer explicitement
    /// plutot que tronquer silencieusement — le garde-fou du plan 2c.
    #[test]
    #[should_panic(expected = "doc _source > 8 MiB non supporté par la side-table packée")]
    fn length_overflow_panics_with_explicit_message() {
        pack_document_slot(0, PACKED_LENGTH_MAX + 1, SOURCE_CODEC_RAW);
    }

    /// `offset` au-dela de 2^40 − 1 (1 Tio) doit paniquer explicitement
    /// plutot que tronquer silencieusement (meme garde-fou, second champ).
    #[test]
    #[should_panic(expected = "offset _source > 1 TiB non supporté par la side-table packée")]
    fn offset_overflow_panics_with_explicit_message() {
        pack_document_slot(PACKED_OFFSET_MAX + 1, 2, SOURCE_CODEC_RAW);
    }
}

#[derive(Debug, Default)]
struct InMemoryIndex {
    /// Lot C `C2` : snapshot immuable, dense, materialise au dernier
    /// `_refresh` — voir [`DenseIdMaps`]. Les ecritures posterieures a ce
    /// snapshot vivent dans les 4 champs `*_dirty` / `deleted_since_dense`
    /// ci-dessous jusqu'au PROCHAIN `_refresh` ([`InMemoryIndex::densify`],
    /// appele par [`InMemoryIndex::finalize_terms_for_refresh`]). Toute
    /// lecture (`_id` GET, `_source`, scoring, `_count`) doit passer par
    /// [`InMemoryIndex::resolve_uid`] / [`InMemoryIndex::uid_for_doc_id`] /
    /// [`InMemoryIndex::blob_for_doc_id`], qui fusionnent les deux
    /// couches — ne JAMAIS lire `dense` directement hors de ces helpers,
    /// une lecture directe verrait un etat perime des l'instant ou une
    /// ecriture arrive apres le dernier `_refresh` (le cas courant sur le
    /// hot path bulk : `ensure_terms_ready` materialise les postings
    /// SANS `_refresh`, donc une recherche peut voir des docs encore
    /// uniquement `*_dirty`).
    dense: DenseIdMaps,
    /// UID -> doc_id pour les documents INSERES depuis le dernier
    /// densify (absents de `dense.forward`). Une mise a jour d'un
    /// document DEJA densifie ne touche PAS cette map (le doc_id ne
    /// change jamais) — seul `documents_dirty` change alors. Retire
    /// l'entree d'un uid frais supprime (voir
    /// `delete_document_deferred`). Cle `Arc<str>` partagee avec
    /// `reverse_dirty` (meme trick "levier 3" que l'ancien design : un
    /// seul alloc par UID frais, pas deux).
    forward_dirty: HashMap<Arc<str>, u32>,
    /// doc_id -> UID, miroir de `forward_dirty` (memes instances
    /// `Arc<str>`, `Arc::clone`). Ne couvre QUE les doc_id frais (jamais
    /// densifies) — un doc deja densifie garde son UID dans `dense`,
    /// jamais duplique ici meme apres update.
    reverse_dirty: HashMap<u32, Arc<str>>,
    /// doc_id -> `_source`, pour les inserts frais ET les updates d'un
    /// doc deja densifie (le blob change, le doc_id jamais). Prioritaire
    /// sur `dense.documents` a la lecture (voir `blob_for_doc_id`).
    documents_dirty: HashMap<u32, SourceBlob>,
    /// doc_id du snapshot `dense` (`< dense.doc_count()`) supprimes
    /// depuis ce densify. Verifie EN PREMIER par toute lecture
    /// doc_id-keyed (`uid_for_doc_id` / `blob_for_doc_id`) avant de
    /// consulter `*_dirty` / `dense` : un doc_id tombstonne ne peut
    /// jamais redevenir vivant (voir la garantie "jamais reutilise" sur
    /// [`DenseIdMaps`]), donc cet ordre est correct par construction,
    /// pas seulement grace au nettoyage best-effort fait par
    /// `delete_document_deferred`. Vide a chaque densify (les trous sont
    /// alors codes en dur dans le nouveau `dense.reverse_offsets`).
    deleted_since_dense: HashSet<u32>,
    /// Nombre de documents vivants, maintenu de façon incrementale (+1
    /// sur un insert frais, -1 sur un delete reussi, inchange sur un
    /// update) pour eviter un scan O(doc_count) a chaque `_count` /
    /// `document_count` (contrat deja documente sur
    /// [`AppState::document_count`] : "Avoids the O(N) clone…").
    live_count: u32,
    /// `mmap M1` — segment `source.dat` file-backed sous `TMPDIR`,
    /// pre-alloue par `posix_fallocate(64 MiB)`. Append-only pendant le
    /// bulk. Le store est cree paresseusement via `Default` (un tempfile
    /// par index) ; il est supprime au `Drop` de `InMemoryIndex`.
    source_store: SourceStore,
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
    /// C1 fix (design doc `docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §C1) test/ops hook: per-index override of
    /// [`Self::maybe_densify_by_budget`]'s budget, independently of the
    /// process-wide `SURCH_DENSIFY_BUDGET_DOCS` env var — same
    /// `OnceLock`-cannot-flip-mid-run rationale as `DocumentIndex`'s
    /// `FlushBudgetOverride`/`MergeFaninOverride` (`surch_index::document_index`).
    densify_budget_override: DensifyBudgetOverride,
}

/// See `InMemoryIndex`'s `densify_budget_override` field. `UseEnv` (the
/// `Default`) reads [`densify_budget_docs`] (the process-wide
/// `SURCH_DENSIFY_BUDGET_DOCS` `OnceLock`) — production default.
/// `Forced(_)` is set via [`AppState::set_densify_budget_docs_override`]
/// so a test can pin an exact budget (or force "no budget") on ONE index
/// instance, independently of the env var and of any other test in the
/// same process.
#[derive(Debug, Clone, Copy, Default)]
enum DensifyBudgetOverride {
    #[default]
    UseEnv,
    Forced(Option<u64>),
}

/// C1 fix: env-configured doc-count threshold for
/// [`InMemoryIndex::maybe_densify_by_budget`], read ONCE per process
/// (mirrors `surch_index::document_index::flush_budget_bytes`'s
/// established `OnceLock` pattern). `None` when unset, empty, `"0"`, or
/// unparseable — the reversibility flag: with no budget configured, the
/// id-maps overlay (`forward_dirty`/`reverse_dirty`/`documents_dirty`/
/// `deleted_since_dense`) is only ever drained by an explicit `_refresh`
/// (`InMemoryIndex::densify`, called from `finalize_terms_for_refresh`),
/// exactly like before this feature existed — this flag ONLY controls
/// whether `_bulk` chunk boundaries ALSO proactively trigger a densify
/// mid-bulk.
///
/// Precision (independent review, design doc §C1 fix): this flag does
/// NOT gate which internal algorithm `InMemoryIndex::densify` picks —
/// that dispatch (`densify_append_only` vs `densify_full`) is decided
/// SOLELY by interference detection, in every call, flag or no flag. So
/// even with this env var unset, a SECOND (and later) explicit `_refresh`
/// on an append-only sequence already takes the incremental path, not
/// just the historical from-scratch one — proven output-equivalent (not
/// literally the same code path) to what ran before this fix. What THIS
/// flag changes is purely the CADENCE: whether densify also runs
/// mid-bulk, before an explicit `_refresh` ever happens.
///
/// When set, every `_bulk` chunk boundary
/// (`InMemoryIndex::append_to_index`) ALSO checks the overlay's size and
/// drains it early via the incremental append-only fast path
/// (`InMemoryIndex::densify_append_only`) once it crosses the budget —
/// bounding the OVERLAY's peak to O(budget) instead of O(corpus). This is
/// what the design doc's §C1 diagnostic identified as the DOMINANT
/// transient of the 28,9M refresh OOM: with a single final `_refresh`
/// (the fair-ab bench's default), the overlay held the WHOLE corpus
/// (measured ~5+ GiB at 28,9M with the codebase's own
/// `index_state_memory_bytes` accounting formula), dwarfing the
/// old+new-`DenseIdMaps` double-detention (~2,4 GiB re-measured by
/// independent review, see `InMemoryIndex::densify`'s doc comment)
/// `densify_full` alone pays.
fn densify_budget_docs() -> Option<u64> {
    static BUDGET: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("SURCH_DENSIFY_BUDGET_DOCS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|&docs| docs > 0)
    })
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

/// Plan segments S2 (`docs/paper/design-segments-pic-borne-2026-07-05.md`):
/// the dense `doc_len` byte slice(s) backing [`FieldScoringStats::doc_len`].
/// `doc_id`s are GLOBAL but each sealed segment's own `doc_len_dense` is
/// indexed LOCALLY (`doc_id - doc_base`, see `surch_index`'s `Segment`
/// design note), so resolving a global `doc_id` needs to know which
/// segment it falls into. `Single` is the fast path used whenever there is
/// exactly one segment (forever true while `SURCH_FLUSH_BUDGET_BYTES` is
/// unset — the S1 reversibility flag): it borrows the SAME slice
/// `doc_len_dense` used to be, with NO extra allocation and NO
/// `partition_point` — bit-identical to before this enum existed.
/// `Segments` is the genuine multi-segment case (`Vec` allocated once per
/// query per field, bounded by segment count).
#[derive(Clone, Debug, PartialEq)]
enum DocLenDense<'a> {
    None,
    Single(&'a [u8]),
    /// `(doc_base, doc_len_dense)` pairs, ascending by `doc_base`.
    Segments(Vec<(u32, &'a [u8])>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldScoringStats<'a> {
    pub doc_count: u64,
    pub avg_doc_len: f64,
    pub norms_enabled: bool,
    /// See [`DocLenDense`]. Not `pub`: `Self::doc_len` is the only
    /// sanctioned way to read a doc's length (mirrors the pre-S2 API,
    /// which never exposed `doc_len_dense` for direct indexing either —
    /// grep-audited, every consumer already went through `.doc_len()`).
    doc_len_dense: DocLenDense<'a>,
    /// Precomputed smallest reconstructed `doc_len` (`0` = none),
    /// threaded from the index's incrementally-maintained
    /// `FieldLengthStats::min_doc_len` so the WAND upper bound no
    /// longer re-scans the dense slice per query. Already in the
    /// reconstructed-length domain (same as [`Self::doc_len`]).
    pub min_doc_len: u64,
}

impl<'a> FieldScoringStats<'a> {
    /// Lucene-quantized `doc_len` for the GLOBAL `doc_id`, or `None` when
    /// no length was recorded. Each byte is decoded via
    /// [`surch_index::decode_doc_len_byte`] — the encoding is the same one
    /// Lucene's `BM25Similarity` uses, so the reconstructed value is the
    /// value the scorer must consume (see
    /// `docs/paper/ndcg-trec-covid-rootcause-22.md`).
    pub fn doc_len(&self, doc_id: u32) -> Option<u64> {
        match &self.doc_len_dense {
            DocLenDense::None => None,
            DocLenDense::Single(dense) => dense
                .get(doc_id as usize)
                .copied()
                .filter(|&byte| byte > 0)
                .map(surch_index::decode_doc_len_byte),
            DocLenDense::Segments(segments) => {
                // `partition_point` over ascending `doc_base`: the owning
                // segment is the last one whose `doc_base <= doc_id`.
                let idx = segments.partition_point(|&(base, _)| base <= doc_id);
                if idx == 0 {
                    return None;
                }
                let (base, dense) = segments[idx - 1];
                let local = doc_id - base;
                dense
                    .get(local as usize)
                    .copied()
                    .filter(|&byte| byte > 0)
                    .map(surch_index::decode_doc_len_byte)
            }
        }
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

    /// Lot C `C2` : resout `id` vers son `doc_id` VIVANT courant, en
    /// fusionnant la couche mutable (`forward_dirty`) et le snapshot
    /// dense (`dense.forward`, filtre par `deleted_since_dense`). Source
    /// de verite UNIQUE pour la decision insert-vs-update de
    /// [`Self::upsert_document_deferred`] ET pour toute lecture publique
    /// (`has_document`, `parsed_source`, `internal_doc_ids`) — ne JAMAIS
    /// interroger `dense.forward`/`forward_dirty` directement ailleurs.
    fn resolve_uid(&self, id: &str) -> Option<u32> {
        if let Some(&doc_id) = self.forward_dirty.get(id) {
            return Some(doc_id);
        }
        let doc_id = self.dense.forward_get(id)?;
        if self.deleted_since_dense.contains(&doc_id) {
            None
        } else {
            Some(doc_id)
        }
    }

    /// Lot C `C2` : UID public pour `doc_id`, fusionnant les deux
    /// couches. Le tombstone est verifie EN PREMIER (voir le commentaire
    /// sur `deleted_since_dense`) : un `doc_id` retire ne peut jamais
    /// redevenir vivant, donc court-circuiter ici est correct meme si un
    /// override `*_dirty` obsolete traine encore.
    fn uid_for_doc_id(&self, doc_id: u32) -> Option<&str> {
        if self.deleted_since_dense.contains(&doc_id) {
            return None;
        }
        if let Some(uid) = self.reverse_dirty.get(&doc_id) {
            return Some(uid.as_ref());
        }
        self.dense.uid(doc_id)
    }

    /// Lot C `C2` : `_source` stocke pour `doc_id`, fusionnant les deux
    /// couches (meme ordre de priorite que `uid_for_doc_id`). Retourne
    /// PAR VALEUR depuis le plan 2c (`DenseIdMaps::blob` deballe le cote
    /// dense ; le cote dirty clone un `SourceBlob` existant — `OnDisk`
    /// copie des scalaires, `Compressed` bump un `Arc` refcount, les deux
    /// pas chers).
    fn blob_for_doc_id(&self, doc_id: u32) -> Option<SourceBlob> {
        if self.deleted_since_dense.contains(&doc_id) {
            return None;
        }
        if let Some(blob) = self.documents_dirty.get(&doc_id) {
            return Some(blob.clone());
        }
        self.dense.blob(doc_id)
    }

    /// Lot C `C2` : `doc_id`s vivants en ordre ascendant (`0..next_doc_id`,
    /// trous sautes). C'est l'ordre d'INSERTION (le seul ordre naturel
    /// qu'un `doc_id` dense supporte), PAS l'ancien ordre lexicographique
    /// sur l'uid public que la `BTreeMap<Arc<str>, _>` produisait — voir
    /// le changement de contrat documente sur `AppState::documents`/
    /// `AppState::documents_paginated`.
    fn live_doc_ids(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.next_doc_id).filter(move |&doc_id| !self.deleted_since_dense.contains(&doc_id))
    }

    fn upsert_document_deferred(&mut self, id: &str, source: Value) {
        // `mmap M1` + option B : on serialise puis on append au segment
        // `source.dat` file-backed (cf. module `source_store`). Le slot
        // stocke `OnDisk { offset, length, codec }` — 12 octets utiles
        // (+ le tag `codec`) en RAM au lieu des bytes JSON eux-memes.
        //
        // Coût bulk : 1× pwrite (~5 µs amorti grace au
        // `posix_fallocate` qui evite l'extension ext4 par bloc 4 KiB).
        // Gate indexation >= 14 000 docs/s preserve.
        //
        // Tranche 2b (`docs/paper/contre-expertise-2b-source-2026-07-10.md`
        // §D) : compression zstd PAR-DOC inline, juste avant l'append,
        // gatee par `SURCH_SOURCE_COMPRESS` (cf. [`source_compress_enabled`]).
        // OFF (defaut) = bytes bruts ecrits tels quels, tag
        // [`SOURCE_CODEC_RAW`] — comportement bit-identique a avant 2b. ON
        // = bytes zstd niveau [`ZSTD_SOURCE_LEVEL`], tag
        // [`SOURCE_CODEC_ZSTD`]. Zero buffering supplementaire : toujours
        // 1 seul `pwrite`/doc, juste plus petit quand compresse.
        //
        // `ensure_fields` a besoin du `Value` parse, donc analyse AVANT
        // serialisation. Updates (meme `id`) ecrasent le slot dirty — les
        // bytes precedents dans `source.dat` sont orphelins (acceptable
        // pour P1 ; P2/P3 ajoutent une compaction segments). En pratique
        // le bulk matchID est append-only par construction, donc zero
        // orphelin.
        self.mapping.ensure_fields(&source);
        let serialized =
            serde_json::to_vec(&source).expect("a validated _source serialises to JSON");
        let (stored_bytes, codec) = if source_compress_enabled() {
            (SourceBlob::encode_zstd(&serialized), SOURCE_CODEC_ZSTD)
        } else {
            (serialized, SOURCE_CODEC_RAW)
        };
        let (offset, length) = self.source_store.append(&stored_bytes);
        let blob = SourceBlob::OnDisk {
            offset,
            length,
            codec,
        };

        if let Some(doc_id) = self.resolve_uid(id) {
            // Update : le `doc_id` ne change JAMAIS, seul le blob
            // stocke change. Va dans l'overlay dirty que le doc soit deja
            // densifie (`doc_id < dense.doc_count()`) ou encore frais —
            // `blob_for_doc_id` regarde `documents_dirty` avant `dense`
            // dans les deux cas.
            self.documents_dirty.insert(doc_id, blob);
            return;
        }

        // Insert frais — un uid jamais vu, OU un uid deja vu mais
        // supprime depuis (voir `resolve_uid` : un `doc_id` tombstonne
        // reste tombstonne pour toujours, donc ceci mint TOUJOURS un
        // `doc_id` neuf, jamais une resurrection de l'ancien).
        //
        // Lot C `C2` (mirroir de l'ancien "levier 3") : un SEUL
        // `Arc<str>` porte l'UID, partage entre `forward_dirty` (clé) et
        // `reverse_dirty` (valeur) via `Arc::clone` — un seul alloc pour
        // les deux handles.
        let doc_id = self.next_doc_id;
        self.next_doc_id += 1;
        let uid: Arc<str> = Arc::from(id);
        self.reverse_dirty.insert(doc_id, Arc::clone(&uid));
        self.forward_dirty.insert(uid, doc_id);
        self.documents_dirty.insert(doc_id, blob);
        self.live_count += 1;
    }

    fn delete_document(&mut self, id: &str) {
        if self.delete_document_deferred(id) {
            self.rebuild_index();
        }
    }

    fn delete_document_deferred(&mut self, id: &str) -> bool {
        let Some(doc_id) = self.resolve_uid(id) else {
            return false;
        };
        // Retire tout override dirty pour ce doc_id/uid — couvre a la
        // fois un insert frais jamais densifie (retraction complete,
        // aucun tombstone requis puisqu'il n'a jamais existe dans
        // `dense`) ET un doc deja densifie mis a jour puis supprime dans
        // la meme fenetre (no-op si aucun override n'existait).
        self.forward_dirty.remove(id);
        self.reverse_dirty.remove(&doc_id);
        self.documents_dirty.remove(&doc_id);
        // Un `doc_id` du snapshot dense doit etre tombstonne
        // explicitement (le buffer `dense` est immuable jusqu'au
        // prochain densify) ; un `doc_id` frais (jamais densifie) n'a
        // besoin de rien de plus, la retraction ci-dessus suffit.
        if doc_id < self.dense.doc_count() {
            self.deleted_since_dense.insert(doc_id);
        }
        self.live_count -= 1;
        true
    }

    fn mapping_value(&self) -> Value {
        self.mapping.as_value()
    }

    fn settings_value(&self) -> Value {
        self.settings.clone()
    }

    fn has_document(&self, id: &str) -> bool {
        self.resolve_uid(id).is_some()
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        let mut documents: Vec<(u32, Vec<(String, String)>)> = Vec::new();
        for doc_id in 0..self.next_doc_id {
            let Some(blob) = self.blob_for_doc_id(doc_id) else {
                continue;
            };
            // #15 + `mmap M1` : on parse le `_source` stocke (OnDisk ->
            // pread sur le segment file-backed, Compressed -> decode
            // thread-local). Seul chemin d'INDEXATION qui touche le
            // decode/pread ; pas le hot path bulk steady-state.
            let parsed = parse_source_blob(&blob, &self.source_store);
            documents.push((doc_id, indexed_fields_for_document(&parsed, &self.mapping)));
        }
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
        // stable pour la suite. `blob_for_doc_id` et
        // `indexed_fields_for_document` ne touchent que des structures
        // lues par reference (`dense`, `documents_dirty`, `mapping`),
        // donc l'itération `rayon::par_iter` est thread-safe.
        //
        // `mmap M1` : sur le hot path bulk steady-state, le blob resolu
        // est `OnDisk` → `pread` sur le segment `source.dat` file-backed
        // (les bytes viennent juste d'etre ecrits par
        // `upsert_document_deferred`, page-cache OS chaud). Aucun decode
        // codec ici. Chaque worker thread partage `Arc<File>` (Sync), le
        // `pread` syscall est concurrent-safe par construction. Cout
        // per-doc dominant ≈ 50 %, gain attendu ~1.4× sur 2 cores W=2.
        //
        // Lot C `C2` : resolution DIRECTE par `doc_id` (plus de detour
        // par l'uid public) — `blob_for_doc_id` est deja un lookup
        // doc_id-keyed, ce que `reverse_document_ids.get` +
        // `parsed_source(uid)` faisait en deux temps auparavant.
        let self_ref = &*self;
        let documents = new_doc_ids
            .par_iter()
            .filter_map(|&doc_id| {
                let blob = self_ref.blob_for_doc_id(doc_id)?;
                let source = parse_source_blob(&blob, &self_ref.source_store);
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
        // Plan segments S2: check the flush-by-budget threshold ONCE per
        // bulk chunk — this function IS the chunk boundary (one call per
        // `_bulk` POST / `apply_document_writes` batch of fresh inserts).
        // No-op when `SURCH_FLUSH_BUDGET_BYTES` is unset (the S1
        // reversibility flag). Deliberately NOT called from
        // `rebuild_index()`, which must always reproduce a mono-segment
        // index (see `DocumentIndex::clear`'s doc).
        self.index.maybe_flush_by_budget();
        // C1 fix: same chunk-boundary hook, for the id-maps overlay this
        // time — see `maybe_densify_by_budget`'s doc comment. No-op when
        // `SURCH_DENSIFY_BUDGET_DOCS` is unset, and (like the postings
        // budget above) deliberately not called from `rebuild_index()`.
        self.maybe_densify_by_budget();
    }

    /// C1 fix (design doc §C1): resolve this index's densify budget,
    /// honoring the `densify_budget_override` field when forced (test/ops
    /// hook, see [`AppState::set_densify_budget_docs_override`]) or
    /// falling back to the process-wide [`densify_budget_docs`] env var.
    fn resolved_densify_budget_docs(&self) -> Option<u64> {
        match self.densify_budget_override {
            DensifyBudgetOverride::UseEnv => densify_budget_docs(),
            DensifyBudgetOverride::Forced(budget) => budget,
        }
    }

    /// C1 fix: if the id-maps overlay (everything inserted since the
    /// last [`Self::densify`]) has grown past the configured budget,
    /// drain it NOW via the incremental append-only fast path instead of
    /// waiting for the next explicit `_refresh`. Call this ONCE PER BULK
    /// CHUNK (mirrors `DocumentIndex::maybe_flush_by_budget`'s own
    /// contract) — never from `rebuild_index()`'s replay path.
    ///
    /// `next_doc_id - dense.doc_count()` is used as the overlay-size
    /// proxy rather than re-deriving the byte estimate
    /// `AppState::index_state_memory_bytes` computes: for the append-only
    /// hot path this function targets, every doc_id in that range holds
    /// exactly one `forward_dirty`/`reverse_dirty`/`documents_dirty`
    /// entry, so a doc-count budget is a simple, cheap, and accurate
    /// enough trigger. At ~140-150 bytes/doc across the three overlays, a
    /// budget of a few hundred thousand to ~1-2M docs keeps the OVERLAY
    /// itself in the tens to low hundreds of MiB — nowhere near the ~5+
    /// GiB measured for the whole 28,9M corpus with no periodic trigger
    /// at all (the design doc's §C1 diagnostic, the DOMINANT transient).
    /// The dense buffers `densify_append_only` extends can still spike
    /// higher on the rare tranche that crosses one of `Vec`'s own growth
    /// thresholds (an amortized, not per-tranche, cost — see that
    /// method's doc comment) — smaller and far less frequent than the
    /// overlay problem this budget exists to bound, but not literally
    /// zero.
    fn maybe_densify_by_budget(&mut self) {
        let Some(budget) = self.resolved_densify_budget_docs() else {
            return;
        };
        let overlay_docs = u64::from(self.next_doc_id - self.dense.doc_count());
        if overlay_docs < budget {
            return;
        }
        self.densify();
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
            // Invariant : `terms_finalized == true` only happens right
            // after this method ran (and every write path resets it to
            // `false` again via `rebuild_index`/`append_to_index` — see
            // their doc comments), so the 4 `densify` overlays below are
            // necessarily already empty here; nothing pending to fold in.
            debug_assert!(
                self.forward_dirty.is_empty()
                    && self.reverse_dirty.is_empty()
                    && self.documents_dirty.is_empty()
                    && self.deleted_since_dense.is_empty(),
                "terms_finalized=true but id-map overlays are non-empty — \
                 a write path bypassed rebuild_index/append_to_index's \
                 terms_finalized reset"
            );
            return;
        }
        // Lot C `C2` : densifie les id maps dans le meme mouvement que la
        // finalisation des postings ci-dessous — les deux "builders"
        // (postings_builder ici, les 4 overlays dirty pour `densify`)
        // sont draines ensemble a chaque refresh.
        self.densify();
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

    /// Lot C `C2` : fusionne les 4 overlays mutables
    /// (`forward_dirty`/`reverse_dirty`/`documents_dirty`/
    /// `deleted_since_dense`) dans un `DenseIdMaps` frais, puis vide les
    /// overlays. Appelee par [`Self::finalize_terms_for_refresh`] (le
    /// `_refresh` explicite) ET, si `SURCH_DENSIFY_BUDGET_DOCS` est
    /// configure, par [`Self::maybe_densify_by_budget`] (mi-bulk, par
    /// tranche) — le "builder" des id maps est ces 4 champs, exactement
    /// comme `postings_builder` est le builder du dictionnaire de termes.
    ///
    /// C1 fix (design doc `docs/paper/design-segments-pic-borne-2026-07-05.md`
    /// §C1) : diagnostic chiffre — dans le flux bench "gros bulk puis UN
    /// SEUL `_refresh` final" (le cas 28,9M@8g mesure), cette fonction ne
    /// tournait qu'UNE fois, avec un overlay ayant accumule TOUT le
    /// corpus (aucun autre point ne draine `forward_dirty`/
    /// `reverse_dirty`/`documents_dirty` avant un `_refresh` explicite).
    /// A 28,9M docs, la formule deja utilisee par
    /// `AppState::index_state_memory_bytes` (`HASH_ENTRY_OVERHEAD` = 48
    /// o + en-tete `Arc` 16 o + octets uid ~7,6 o/doc pour
    /// `forward_dirty` ; 48+4 o pour `reverse_dirty` ; 48+4+
    /// `size_of::<Option<SourceBlob>>()`~24 o pour `documents_dirty`)
    /// chiffre l'overlay a lui seul a **~5,4 GiB** (1,93 `forward_dirty`
    /// plus 1,40 `reverse_dirty` plus 2,05 `documents_dirty`), CONTRE
    /// (revue independante, cf. design doc §C1 fix) **~2,4 GiB** pour le
    /// pic reel de `densify_full` (le doublement ancien+nouveau `dense` —
    /// quasi vide au tout premier refresh — PLUS le scratch `live_uids`
    /// ~700 Mo PLUS ~28,9M reallocations `Arc::from(uid)` neuves ~680 Mo
    /// PLUS le nouveau `documents` ~700 Mo ; la premiere estimation de ce
    /// commentaire, ~1-1,3 GiB, ne comptait que le doublement de `dense`
    /// seul et etait optimiste). **L'overlay domine toujours** (~2x, pas
    /// ~4x comme initialement estime) : c'est lui qui transforme un
    /// refresh en pic O(corpus) au lieu de O(tranche), pas la
    /// double-detention seule — mais l'un ET l'autre restent bornes par
    /// ce fix.
    ///
    /// Fix retenu : densifier PAR TRANCHES plutot qu'une seule fois —
    /// [`Self::maybe_densify_by_budget`] declenche cette methode des que
    /// l'overlay depasse un budget de doc-count configurable. Le chemin
    /// rapide [`Self::densify_append_only`] ne retraite QUE la tranche
    /// neuve (append aux buffers `reverse_uids`/`reverse_offsets`/
    /// `documents`, fusion streaming du FST via [`merge_forward_fst`]) —
    /// cout AMORTI O(tranche) sur l'ensemble du bulk (voir son commentaire
    /// pour la nuance : une reallocation `Vec` occasionnelle, rare, coute
    /// encore O(taille courante), mais PAS a chaque tranche comme un
    /// rescan complet le ferait — un rescan complet a chaque tranche
    /// re-allouerait un `Arc<str>` par UID DEJA densifie a CHAQUE appel,
    /// un cout cumulatif O(N²/tranche) inacceptable, c'est pour ca que ce
    /// chemin existe). Valide uniquement quand rien dans l'overlay ne
    /// touche un `doc_id` DEJA densifie (voir le detecteur d'interference
    /// ci-dessous). Sinon (update/delete contre un vieux doc — plus rare,
    /// plus petit en pratique), on retombe sur [`Self::densify_full`],
    /// l'algorithme EXISTANT (from-scratch, correct et inchange pour ce
    /// cas).
    fn densify(&mut self) {
        if self.forward_dirty.is_empty()
            && self.deleted_since_dense.is_empty()
            && self.documents_dirty.is_empty()
        {
            // Rien de neuf depuis le dernier densify (ex: deux
            // `_refresh` consecutifs sans ecriture) — `self.dense` est
            // deja a jour, evite un rescan O(doc_count) inutile.
            return;
        }

        let old_boundary = self.dense.doc_count();
        // Interference = un update OU un delete touchant un doc_id DEJA
        // densifie (`doc_id < old_boundary`). `deleted_since_dense` ne
        // contient JAMAIS que de tels doc_id par construction
        // (`delete_document_deferred` n'y insere que si `doc_id <
        // self.dense.doc_count()` — un doc frais jamais densifie n'a pas
        // besoin de tombstone), donc le verifier vide suffit sans avoir
        // a comparer chaque entree a `old_boundary`. `documents_dirty`,
        // en revanche, recoit AUSSI les updates de docs frais (pas
        // encore densifies) — d'ou le scan explicite par seuil.
        let has_old_interference = !self.deleted_since_dense.is_empty()
            || self
                .documents_dirty
                .keys()
                .any(|&doc_id| doc_id < old_boundary);

        if has_old_interference {
            self.densify_full();
        } else {
            self.densify_append_only(old_boundary);
        }
    }

    /// Chemin complet (from-scratch) — l'algorithme `densify` historique,
    /// utilise en fallback quand [`Self::densify`] detecte une interference
    /// avec un doc_id deja densifie (un update ou un delete contre un vieux
    /// doc_id).
    ///
    /// Precision (revue independante) : CE fallback n'est PAS conditionne
    /// par `SURCH_DENSIFY_BUDGET_DOCS` — le dispatch rapide/complet dans
    /// [`Self::densify`] ne regarde QUE l'interference, jamais le flag ;
    /// celui-ci ne controle QUE la CADENCE de declenchement mi-bulk
    /// (`Self::maybe_densify_by_budget`). Consequence assumee : meme flag
    /// absent, un `_refresh` explicite APRES un `_refresh` precedent sans
    /// interference entre les deux (insertions pures) emprunte deja
    /// [`Self::densify_append_only`], pas ce chemin complet — ce n'est PAS
    /// une regression de comportement OBSERVABLE (sortie prouvee
    /// equivalente, voir le commentaire de `densify_append_only`), mais ce
    /// n'est pas non plus "l'ancien chemin de code au mot pres" ; seul le
    /// TOUT PREMIER appel jamais interference-libre partage exactement le
    /// meme calcul que l'ancien `densify()` faisait a chaque fois.
    ///
    /// O(doc courant vivant + trous) : meme ordre de grandeur que le
    /// rescan complet deja paye par `rebuild_index()` sur un delete/update
    /// et que la reconstruction FST des postings a chaque refresh — pas
    /// un cout nouveau de nature, juste un cout supplementaire du meme
    /// ordre.
    ///
    /// Cout transitoire assume : le nouveau snapshot est construit
    /// PENDANT que l'ancien `self.dense.reverse_uids`/`reverse_offsets`/
    /// `documents` restent alloues (ils sont lus tout du long — `.blob`/
    /// `.uid` ci-dessous), donc le pic memoire pendant cette methode est
    /// `ancien dense + nouveau dense` pour CES TROIS buffers — un
    /// doublement transitoire, borne et libere des la sortie de cette
    /// methode (`self.dense = ...` droppe l'ancien).
    ///
    /// C1 fix : `self.dense.forward` (le FST), lui, n'est JAMAIS lu par
    /// la boucle ci-dessous (`dense.blob`/`dense.uid` ne touchent que
    /// `documents`/`reverse_uids`/`reverse_offsets` — `forward` ne sert
    /// qu'a `resolve_uid`, hors de cette methode) : on peut donc le
    /// liberer des MAINTENANT plutot qu'a l'affectation finale, sans
    /// attendre. Sans risque pour un lecteur concurrent : le
    /// verrou d'ecriture pris par `AppState::refresh_index` (ou
    /// `maybe_densify_by_budget`, appele sous le meme verrou que
    /// `append_to_index`) couvre TOUTE la duree de cet appel — aucune
    /// recherche ne peut observer `forward` absent entre-temps.
    fn densify_full(&mut self) {
        self.dense.forward = None;

        let doc_count = self.next_doc_id;
        // `live_uids[i]` correspond a `documents[i]` par construction (la
        // meme condition `Some(blob) && Some(uid)` remplit les deux au
        // meme rang) — voir le `debug_assert!` plus bas.
        let mut live_uids: Vec<(u32, Arc<str>)> = Vec::new();
        // Plan 2c : `documents` est desormais un `Vec<u64>` packe — un
        // slot vaut `0` (trou, `length == 0`) ou le pack de l'`OnDisk`
        // vivant ([`pack_source_blob`], jamais de `Compressed` sur ce
        // chemin, voir son doc comment).
        let mut documents: Vec<u64> = Vec::with_capacity(doc_count as usize);
        for doc_id in 0..doc_count {
            if self.deleted_since_dense.contains(&doc_id) {
                documents.push(0);
                continue;
            }
            let blob = self
                .documents_dirty
                .get(&doc_id)
                .cloned()
                .or_else(|| self.dense.blob(doc_id));
            let uid: Option<Arc<str>> = match self.reverse_dirty.get(&doc_id) {
                Some(uid) => Some(Arc::clone(uid)),
                None => match self.dense.uid(doc_id) {
                    Some(uid) => {
                        let owned: Arc<str> = Arc::from(uid);
                        Some(owned)
                    }
                    None => None,
                },
            };
            match (blob, uid) {
                (Some(blob), Some(uid)) => {
                    live_uids.push((doc_id, uid));
                    documents.push(pack_source_blob(&blob));
                }
                // Defensif : un `doc_id < doc_count` non tombstonne doit
                // toujours avoir un blob ET un uid (invariant maintenu
                // par `upsert_document_deferred`/`delete_document_deferred`).
                // Ne devrait jamais arriver ; on reste sur le trou plutot
                // que de paniquer sur un etat interne incoherent.
                _ => documents.push(0),
            }
        }
        debug_assert_eq!(
            documents.len(),
            doc_count as usize,
            "densify_full must produce exactly one documents slot per doc_id in 0..next_doc_id"
        );

        // Reverse (doc_id -> uid), empaquete, positionnel par
        // construction : `live_uids` est deja en ordre de doc_id
        // croissant (boucle ci-dessus), donc un simple parcours en
        // parallele avec `0..doc_count` place chaque uid au bon offset,
        // un "trou" (doc_id absent de `live_uids`) obtenant un span vide.
        let mut reverse_uids: Vec<u8> = Vec::new();
        let mut reverse_offsets: Vec<u32> = Vec::with_capacity(doc_count as usize + 1);
        reverse_offsets.push(0);
        let mut live_iter = live_uids.iter().peekable();
        for doc_id in 0..doc_count {
            if let Some((next_id, uid)) = live_iter.peek() {
                if *next_id == doc_id {
                    reverse_uids.extend_from_slice(uid.as_bytes());
                    reverse_offsets.push(reverse_uids.len() as u32);
                    live_iter.next();
                    continue;
                }
            }
            reverse_offsets.push(reverse_uids.len() as u32);
        }

        // Forward (uid -> doc_id) : le `fst::MapBuilder` exige des cles
        // strictement croissantes, donc on trie `live_uids` par octets
        // d'uid (il est actuellement en ordre de doc_id).
        let mut by_uid = live_uids;
        by_uid.sort_unstable_by(|a, b| a.1.as_bytes().cmp(b.1.as_bytes()));
        debug_assert!(
            by_uid
                .windows(2)
                .all(|pair| pair[0].1.as_bytes() < pair[1].1.as_bytes()),
            "two live doc_ids resolved to the same uid — forward resolution invariant violated"
        );
        let forward = if by_uid.is_empty() {
            None
        } else {
            let mut builder = MapBuilder::memory();
            for (doc_id, uid) in &by_uid {
                builder
                    .insert(uid.as_bytes(), u64::from(*doc_id))
                    .expect("live uids are unique and were just sorted ascending");
            }
            let bytes = builder
                .into_inner()
                .expect("fst::MapBuilder memory writer never fails I/O");
            Some(Map::new(bytes).expect("fst::Map from valid MapBuilder bytes"))
        };

        self.dense = DenseIdMaps {
            forward,
            reverse_uids,
            reverse_offsets,
            documents,
        };
        self.forward_dirty.clear();
        self.forward_dirty.shrink_to_fit();
        self.reverse_dirty.clear();
        self.reverse_dirty.shrink_to_fit();
        self.documents_dirty.clear();
        self.documents_dirty.shrink_to_fit();
        self.deleted_since_dense.clear();
        self.deleted_since_dense.shrink_to_fit();
    }

    /// C1 fix: chemin rapide de [`Self::densify`], valide UNIQUEMENT
    /// quand rien dans l'overlay ne touche un `doc_id` deja densifie
    /// (`old_boundary == self.dense.doc_count()`, garanti par l'appelant
    /// — voir son detecteur d'interference). C'est le cas courant d'un
    /// bulk append-only (le corpus INSEE deces du gate 28,9M : aucun
    /// update/delete).
    ///
    /// `reverse_uids`/`reverse_offsets`/`documents` sont ETENDUS via
    /// `Vec::reserve`/`push` (voir `DenseIdMaps`'s doc comment : ces
    /// champs sont des `Vec<T>`, PAS des `Box<[T]>`, precisement pour que
    /// la capacite geometrique accumulee survive d'un appel au suivant —
    /// sans ca, `.reserve()` re-copierait la TOTALITE du buffer courant a
    /// CHAQUE tranche, un piege repere en revue independante). Cout
    /// AMORTI sur l'ensemble du bulk : O(N) total (comme n'importe quel
    /// `Vec` qui grossit par `push`), la plupart des tranches ne
    /// reallouant PAS du tout (capacite deja suffisante) ; seules les
    /// tranches qui franchissent un palier de croissance de `Vec`
    /// (rare, ~O(log N) fois sur tout le bulk) paient une copie de la
    /// taille courante — un pic ponctuel, PAS le pic systematique a
    /// CHAQUE tranche que `densify_full` paierait s'il etait appele en
    /// boucle (cout cumulatif O(N²/tranche) inacceptable, un
    /// `Arc::from(uid)` neuf par uid DEJA densifie a chaque appel).
    /// `forward` est reconstruit via [`merge_forward_fst`], une fusion
    /// streaming (merge-join) de l'ANCIEN FST avec la tranche neuve
    /// triee, O(ancien FST + tranche) — voir le commentaire de cette
    /// fonction pour l'argument de disjonction des cles.
    fn densify_append_only(&mut self, old_boundary: u32) {
        let new_upper = self.next_doc_id;
        let tranche_len = (new_upper - old_boundary) as usize;

        // C1 fix (revue independante) : `mem::take` recupere directement le
        // `Vec<T>` du champ (plus de `Box<[T]>`/`.into_vec()`/
        // `.into_boxed_slice()` intermediaires) — la capacite geometrique
        // eventuellement deja presente sur ces buffers SURVIT d'un appel
        // au suivant, ce qui est le point : sans ca, `.reserve()`
        // ci-dessous re-declencherait une reallocation+copie de la
        // TOTALITE du buffer courant a CHAQUE tranche (voir le
        // commentaire de `DenseIdMaps`).
        //
        // Plan 2c : `documents` est un `Vec<u64>` packe — cette copie de
        // prefixe est desormais un simple memcpy de `u64` (8 o/slot),
        // plus rapide que l'ancien `Vec<Option<SourceBlob>>` (24 o/slot).
        let mut documents = std::mem::take(&mut self.dense.documents);
        documents.reserve(tranche_len);

        let mut reverse_uids = std::mem::take(&mut self.dense.reverse_uids);
        let mut reverse_offsets = std::mem::take(&mut self.dense.reverse_offsets);
        if reverse_offsets.is_empty() {
            // Bootstrap: le tout premier densify (dense encore `Default`)
            // n'a pas encore le `0` sentinelle de tete — tout appel
            // suivant en herite via l'append ci-dessous.
            reverse_offsets.push(0);
        }
        reverse_offsets.reserve(tranche_len);

        let mut new_entries: Vec<(Arc<str>, u32)> = Vec::with_capacity(tranche_len);
        for doc_id in old_boundary..new_upper {
            // Plan 2c : pack direct depuis `documents_dirty` — `0` (trou)
            // quand ce doc_id a ete cree PUIS supprime dans cette meme
            // fenetre (voir le commentaire sur `reverse_offsets` juste en
            // dessous, meme cas).
            let packed = self
                .documents_dirty
                .get(&doc_id)
                .map(pack_source_blob)
                .unwrap_or(0);
            documents.push(packed);
            if let Some(uid) = self.reverse_dirty.get(&doc_id) {
                reverse_uids.extend_from_slice(uid.as_bytes());
                new_entries.push((Arc::clone(uid), doc_id));
            }
            // Un doc_id sans entree `reverse_dirty` a ete cree PUIS
            // supprime DANS cette meme fenetre (jamais entre dans
            // `dense`, donc jamais besoin de tombstone) — span vide,
            // meme convention de trou que `densify_full`.
            reverse_offsets.push(reverse_uids.len() as u32);
        }
        debug_assert_eq!(documents.len(), new_upper as usize);
        debug_assert_eq!(reverse_offsets.len(), new_upper as usize + 1);

        // `merge_forward_fst` fait un merge-join : `new_entries` doit
        // etre trie par octets d'uid (l'ordre d'insertion ci-dessus est
        // par doc_id croissant, pas par uid) — meme tri que
        // `densify_full`.
        new_entries.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        debug_assert!(
            new_entries
                .windows(2)
                .all(|pair| pair[0].0.as_bytes() < pair[1].0.as_bytes()),
            "two live doc_ids resolved to the same uid within one tranche — forward resolution invariant violated"
        );

        let forward = merge_forward_fst(self.dense.forward.take(), &new_entries);

        self.dense = DenseIdMaps {
            forward,
            reverse_uids,
            reverse_offsets,
            documents,
        };
        self.forward_dirty.clear();
        self.forward_dirty.shrink_to_fit();
        self.reverse_dirty.clear();
        self.reverse_dirty.shrink_to_fit();
        self.documents_dirty.clear();
        self.documents_dirty.shrink_to_fit();
        debug_assert!(
            self.deleted_since_dense.is_empty(),
            "densify_append_only must only run when the caller's interference check found no old tombstone"
        );
    }

    /// Convertit en bloc tous les `SourceBlob::OnDisk` vivants en
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
    /// recommencera a offset 0.
    ///
    /// NOTE: Plus appelée depuis `finalize_terms_for_refresh` — avec P1
    /// mmap M1 actif les blobs OnDisk restent sur disque (pread O(1)) et
    /// le heap baisse plus que via la compression option B. Conservée pour
    /// usage manuel/test et resurrection éventuelle (`P2 manifest` notamment).
    /// Lot C `C2` : adaptee aux structures dense/dirty — parcourt les
    /// deux (le dense snapshot ET l'overlay dirty), en ignorant les trous
    /// (`None`).
    ///
    /// Plan 2c (`docs/paper/plan-packing-sidetable-2026-07-12.md`) :
    /// `dense.documents` est desormais un `Vec<u64>` packe qui ne peut
    /// encoder QUE des pointeurs `OnDisk { offset, length, codec }` —
    /// aucune representation possible pour des bytes `Compressed` tenus
    /// en RAM (voir [`pack_document_slot`]). Le cote dense de cette
    /// fonction est donc structurellement irrealisable depuis ce packing
    /// (deja fonctionnellement mort depuis `mmap M1`, cf. la note
    /// ci-dessus) : on ne compacte plus QUE `documents_dirty`, seul cote
    /// qui peut encore porter un `Compressed`.
    #[allow(dead_code)]
    fn compact_after_refresh(&mut self) {
        for blob in self.documents_dirty.values_mut() {
            let SourceBlob::OnDisk {
                offset,
                length,
                codec,
            } = *blob
            else {
                continue;
            };
            // `read_on_disk_bytes` decode selon `codec` (raw ou zstd,
            // tranche 2b) : les bytes obtenus ici sont TOUJOURS le JSON
            // brut, quel que soit le codec du blob source — condition
            // necessaire pour que `encode_for_compact` (deflate) reçoive
            // du JSON et non un flux zstd deja compresse.
            let raw_bytes = read_on_disk_bytes(&self.source_store, offset, length, codec);
            let compressed = SourceBlob::encode_for_compact(&raw_bytes);
            *blob = SourceBlob::Compressed(Arc::from(compressed.into_boxed_slice()));
        }
        // Un `source.dat` ne peut etre tronque en securite que si AUCUN
        // pointeur `OnDisk` n'y fait plus reference. Cote dense, un slot
        // packe vivant EST TOUJOURS un `OnDisk` par construction (jamais
        // de `Compressed` possible depuis ce packing) — toute entree
        // dense vivante interdit donc le reset tant qu'elle existe.
        let dense_still_on_disk =
            (0..self.dense.doc_count()).any(|doc_id| self.dense.blob(doc_id).is_some());
        let dirty_still_on_disk = self
            .documents_dirty
            .values()
            .any(|blob| matches!(blob, SourceBlob::OnDisk { .. }));
        if !dense_still_on_disk && !dirty_still_on_disk {
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

        // Lot C `C1b` sous-pas 2: disk-backed path — `self.index.postings`
        // reads the RAM `doc_ids_flat` channel, intentionally empty when
        // the disk flag is on (see `FieldPostings`'s doc comment); decode
        // the term from the disk segment instead. `term_hits` is not a
        // hot conjunction path (it collects into an owned `Vec<String>`
        // regardless), so a full-term decode here is the design's
        // sanctioned "correct first" fallback, not a block-addressing gap.
        if self.index.postings_disk_backed() {
            return self
                .index
                .decode_from_segment(field, &token)
                .map(|(doc_ids, _freqs)| doc_ids)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|doc_id| self.uid_for_doc_id(doc_id).map(str::to_string))
                .collect();
        }

        self.index
            .postings(field, &token)
            .into_iter()
            .flat_map(|postings| postings.map(|posting| posting.doc_id))
            // Lot C `C2`: `uid_for_doc_id` fuses the dense snapshot and the
            // dirty overlay; `term_hits` keeps its public `Vec<String>`
            // contract, so the public id is materialised here (bounded by
            // the matched-doc set, not the full corpus).
            .filter_map(|doc_id| self.uid_for_doc_id(doc_id).map(str::to_string))
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
        // Lot C `C1b` sous-pas 2: see `term_hits`'s disk branch above —
        // same rationale, this method already returns owned `Vec<u32>`.
        if self.index.postings_disk_backed() {
            return self
                .index
                .decode_from_segment(field, &token)
                .map(|(doc_ids, _freqs)| doc_ids)
                .unwrap_or_default();
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
                        .filter_map(|&doc_id| self.uid_for_doc_id(doc_id).map(str::to_string))
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
                    .filter_map(|&doc_id| self.uid_for_doc_id(doc_id).map(str::to_string))
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
        // Plan segments S1/S2 (docs/paper/design-segments-pic-borne-2026-07-05.md):
        // BM25 `doc_count`/`avg_doc_len`/`min_doc_len` are the GLOBAL
        // aggregate across every sealed segment (Σ doc_count, Σ
        // total_terms / Σ doc_count) — non-negotiable for oracle parity
        // once `segments.len() > 1`. With exactly one segment (S1, the
        // default while `SURCH_FLUSH_BUDGET_BYTES` is unset) this reduces
        // to the same single division `FieldLengthStats::avg_doc_len()`
        // performed directly, so the value is bit-for-bit identical to
        // the pre-segment code path.
        let aggregated = self.index.field_stats_aggregated(field)?;
        let norms_enabled = self.mapping.norms_enabled(field);
        let avg_doc_len = if norms_enabled {
            aggregated.avg_doc_len()?
        } else {
            1.0
        };
        // Plan segments S2: `doc_len` needs to resolve a GLOBAL doc_id to
        // the right segment's LOCALLY-indexed dense byte slice (see
        // `DocLenDense`'s doc). Fast path: with exactly one segment
        // (`segment_count() == 1`, forever true while budget flush is
        // unset), borrow the SAME zero-copy slice as before this
        // refactor — no allocation, bit-identical to S1. Only the
        // genuinely multi-segment case pays a small per-query `Vec`
        // allocation (bounded by segment count).
        let doc_len_dense = if !norms_enabled {
            DocLenDense::None
        } else if self.index.segment_count() == 1 {
            match self.index.field_stats(field) {
                Some(stats) => DocLenDense::Single(stats.doc_len_dense()),
                None => DocLenDense::None,
            }
        } else {
            DocLenDense::Segments(
                self.index
                    .field_stats_segments(field)
                    .into_iter()
                    .map(|(base, stats)| (base, stats.doc_len_dense()))
                    .collect(),
            )
        };

        let min_doc_len = if norms_enabled {
            aggregated.min_doc_len().unwrap_or(0)
        } else {
            0
        };

        Some(FieldScoringStats {
            doc_count: aggregated.doc_count,
            avg_doc_len,
            norms_enabled,
            doc_len_dense,
            min_doc_len,
        })
    }

    fn term_scoring_stats(&self, field: &str, term: &str) -> TermScoringStats {
        // Postings come from `TermDictionary` in ascending `doc_id` order
        // (see `PostingsBuilder::build`), so a single pass produces a sorted
        // accumulator without re-sorting. The same-doc merge below (tail
        // check) is now a pure defensive no-op: `PostingsBuilder::build()`'s
        // `dedup_merge_postings` already guarantees at most one posting per
        // `(field, term, doc_id)` — a multi-valued source field used to be
        // able to violate that (several array values sharing a doc_id, each
        // `add`-ed separately) before that fix. Kept here as
        // belt-and-suspenders in case a future caller bypasses the builder.
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
        // relies on the `PostingsBuilder::build()` invariant: a given
        // `(doc_id, field, term)` triple produces exactly one posting
        // (`dedup_merge_postings` merges any multi-valued-field run before
        // the block metas are computed), so the merge branch above is a
        // defensive no-op and both Vecs have the same length. The
        // `debug_assert_eq!` below catches a regression as soon as it
        // happens.
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
            .filter_map(|doc_id| self.uid_for_doc_id(doc_id).map(str::to_string))
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

        // Lot C `C1b` sous-pas 2: disk-backed path — `self.index.postings`
        // reads the RAM `doc_ids_flat` channel below, intentionally empty
        // when the disk flag is on (see `FieldPostings`'s doc comment).
        if self.index.postings_disk_backed() {
            return Self::match_hits_disk(&self.index, field, &terms, require_all_terms);
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

    /// Lot C `C1b` sous-pas 2: disk-backed counterpart of
    /// [`Self::match_hits_internal`]'s RAM path, byte-for-byte the same
    /// shape (single-token fast path, then OR/AND accumulation), sourced
    /// from [`DocumentIndex::decode_from_segment`] instead of
    /// [`DocumentIndex::postings`].
    fn match_hits_disk(
        index: &DocumentIndex,
        field: &str,
        terms: &[String],
        require_all_terms: bool,
    ) -> Vec<u32> {
        if terms.len() == 1 {
            return index
                .decode_from_segment(field, &terms[0])
                .map(|(doc_ids, _freqs)| doc_ids)
                .unwrap_or_default();
        }

        let mut matches: Option<BTreeSet<u32>> = None;
        for term in terms {
            let current: BTreeSet<u32> = index
                .decode_from_segment(field, term)
                .map(|(doc_ids, _freqs)| doc_ids.into_iter().collect())
                .unwrap_or_default();

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
        // Lot C `C1b` sous-pas 2: disk-backed path — MUST stay
        // block-addressed (a full per-term materialisation here would
        // reintroduce the exact regression this design avoids on the
        // deces bool/full common-term tail). See `Self::conjunction_hits_disk`.
        if self.index.postings_disk_backed() {
            return Self::conjunction_hits_disk(&self.index, terms);
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

    /// Lot P2 : conjonction checked segment par segment. Les segments sont
    /// disjoints et ordonnés par doc id, donc leurs résultats s'ajoutent sans
    /// k-way merge. Une erreur détruit l'accumulateur temporaire et revient au
    /// chemin générique ; l'absence réelle d'un terme ne saute que le segment.
    fn conjunction_hits_disk(index: &DocumentIndex, terms: &[(String, String)]) -> Vec<u32> {
        let mut segmented_terms = Vec::with_capacity(terms.len());
        let mut block_metrics = P2DiskBlockMetrics::default();
        for (field, term) in terms {
            match index.segmented_postings_checked(field, term) {
                Ok(Some(postings)) => segmented_terms.push(postings),
                // Terme absent de tous les segments : l'AND entier est vide.
                Ok(None) => return Vec::new(),
                // Ne jamais publier le préfixe des segments déjà lus.
                Err(_) => return Self::conjunction_hits_merged(index, terms),
            }
        }

        let mut out = Vec::new();
        for segment_idx in 0..index.segment_count() {
            let mut cursors = Vec::with_capacity(segmented_terms.len());
            let mut missing_clause = false;
            for postings in &mut segmented_terms {
                match postings.cursor_at_mut(segment_idx) {
                    Some(cursor) => cursors.push(cursor),
                    // L'absence est locale : les autres segments restent
                    // candidats et conservent leur `df` global au scoring.
                    None => {
                        missing_clause = true;
                        break;
                    }
                }
            }
            if missing_clause {
                let metric_cursors: Vec<_> = cursors.iter().map(|cursor| &**cursor).collect();
                block_metrics.observe(&metric_cursors);
                continue;
            }
            match Self::conjunction_hits_segment(&mut cursors) {
                Ok(segment_hits) => {
                    let metric_cursors: Vec<_> = cursors.iter().map(|cursor| &**cursor).collect();
                    block_metrics.observe(&metric_cursors);
                    out.extend(segment_hits);
                }
                Err(()) => {
                    let metric_cursors: Vec<_> = cursors.iter().map(|cursor| &**cursor).collect();
                    block_metrics.observe(&metric_cursors);
                    block_metrics.publish();
                    return Self::conjunction_hits_merged(index, terms);
                }
            }
        }
        block_metrics.publish();
        out
    }

    /// Leapfrog d'une conjonction dans un seul segment. Le driver est le
    /// terme localement le plus rare, tandis que l'erreur reste observable
    /// pour laisser l'appelant abandonner tous ses temporaires.
    fn conjunction_hits_segment(
        cursors: &mut [&mut SegmentPostingsCursor<'_>],
    ) -> Result<Vec<u32>, ()> {
        if cursors.is_empty() {
            return Ok(Vec::new());
        }
        cursors.sort_by_key(|cursor| cursor.local_df());
        let (driver, followers) = cursors.split_at_mut(1);
        let driver = &mut driver[0];
        let mut cur = Vec::with_capacity(followers.len());
        for follower in followers.iter_mut() {
            cur.push(match follower.advance_to_with_status(0) {
                SegmentPostingsAdvance::Found(doc_id) => Some(doc_id),
                SegmentPostingsAdvance::Exhausted => None,
                SegmentPostingsAdvance::Error => return Err(()),
            });
        }
        let mut out = Vec::new();
        let mut next_probe = 0u32;
        'docs: loop {
            let target = match driver.advance_to_with_status(next_probe) {
                SegmentPostingsAdvance::Found(doc_id) => doc_id,
                SegmentPostingsAdvance::Exhausted => break,
                SegmentPostingsAdvance::Error => return Err(()),
            };
            next_probe = target.saturating_add(1);
            for (i, it) in followers.iter_mut().enumerate() {
                if cur[i].is_some_and(|c| c < target) {
                    cur[i] = match it.advance_to_with_status(target) {
                        SegmentPostingsAdvance::Found(doc_id) => Some(doc_id),
                        SegmentPostingsAdvance::Exhausted => None,
                        SegmentPostingsAdvance::Error => return Err(()),
                    };
                }
                if cur[i] != Some(target) {
                    continue 'docs;
                }
            }
            out.push(target);
        }
        Ok(out)
    }

    /// Référence matérialisée de repli pour [`Self::conjunction_hits_disk`].
    /// P2 n'y arrive plus pour le multi-segment nominal : elle ne sert qu'une
    /// fois qu'une lecture checked refuse de publier un résultat partiel.
    fn conjunction_hits_merged(index: &DocumentIndex, terms: &[(String, String)]) -> Vec<u32> {
        let mut acc: Option<BTreeSet<u32>> = None;
        for (field, term) in terms {
            let current: BTreeSet<u32> = match index.decode_from_segment(field, term) {
                Some((doc_ids, _freqs)) => doc_ids.into_iter().collect(),
                None => return Vec::new(),
            };
            acc = Some(match acc {
                None => current,
                Some(prev) => prev.intersection(&current).copied().collect(),
            });
            if acc.as_ref().is_some_and(BTreeSet::is_empty) {
                return Vec::new();
            }
        }
        acc.unwrap_or_default().into_iter().collect()
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
        let doc_id = self.resolve_uid(id)?;
        let blob = self.blob_for_doc_id(doc_id)?;
        Some(Arc::new(parse_source_blob(&blob, &self.source_store)))
    }

    fn documents_by_internal_ids(&self, index: &str, internal_ids: &[u32]) -> Vec<StoredDocument> {
        let parallel = source_fetch_parallel_enabled();
        if !source_fetch_profile_enabled() {
            return if parallel {
                // Les `winner_ids` restent ordonnes par score avant cette etape.
                // Le premier `collect` garde un slot par id ; le filtrage reste
                // sequentiel apres lui, donc les pread disperses ne peuvent pas
                // changer l'ordre de sortie.
                internal_ids
                    .par_iter()
                    .map(|&doc_id| {
                        let id = self.uid_for_doc_id(doc_id)?;
                        let blob = self.blob_for_doc_id(doc_id)?;
                        let source = Arc::new(parse_source_blob(&blob, &self.source_store));
                        Some(StoredDocument {
                            index: index.to_owned(),
                            id: id.to_string(),
                            source,
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .flatten()
                    .collect()
            } else {
                internal_ids
                    .iter()
                    .filter_map(|&doc_id| {
                        // Lot C `C2`: doc_id-keyed lookups on both sides — no more
                        // detour through the public uid to re-derive a doc_id that
                        // was already known.
                        let id = self.uid_for_doc_id(doc_id)?;
                        let blob = self.blob_for_doc_id(doc_id)?;
                        let source = Arc::new(parse_source_blob(&blob, &self.source_store));
                        Some(StoredDocument {
                            index: index.to_owned(),
                            id: id.to_string(),
                            source,
                        })
                    })
                    .collect()
            };
        }

        // Profil L2 : une seule emission Prometheus par hydratation top-K,
        // sans log par requete. Chaque worker retourne son agregat local;
        // la fusion reste sequentielle et preserve l'ordre des gagnants.
        let mode = if parallel { "parallel" } else { "sequential" };
        let fetch_started = Instant::now();
        let mut profile = SourceFetchProfile {
            requested_ids: internal_ids.len() as u64,
            ..SourceFetchProfile::default()
        };
        let documents = if parallel {
            // Les `winner_ids` restent ordonnes par score avant cette etape.
            // Le premier `collect` garde un slot par id ; le filtrage reste
            // sequentiel apres lui, donc les pread disperses ne peuvent pas
            // changer l'ordre de sortie.
            internal_ids
                .par_iter()
                .map(|&doc_id| {
                    let mut task_profile = SourceFetchProfile::default();
                    if let Some(worker_index) = rayon::current_thread_index() {
                        task_profile.record_worker(worker_index);
                    }
                    let document = (|| {
                        let id = self.uid_for_doc_id(doc_id)?;
                        let blob = self.blob_for_doc_id(doc_id)?;
                        let (source, hit_profile) =
                            parse_source_blob_profiled(&blob, &self.source_store);
                        task_profile.merge(hit_profile);
                        Some(StoredDocument {
                            index: index.to_owned(),
                            id: id.to_string(),
                            source: Arc::new(source),
                        })
                    })();
                    (document, task_profile)
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|(document, task_profile)| {
                    profile.merge(task_profile);
                    document
                })
                .collect()
        } else {
            if !internal_ids.is_empty() {
                profile.record_worker(0);
            }
            internal_ids
                .iter()
                .filter_map(|&doc_id| {
                    // Lot C `C2`: doc_id-keyed lookups on both sides — no more
                    // detour through the public uid to re-derive a doc_id that
                    // was already known.
                    let id = self.uid_for_doc_id(doc_id)?;
                    let blob = self.blob_for_doc_id(doc_id)?;
                    let (source, hit_profile) =
                        parse_source_blob_profiled(&blob, &self.source_store);
                    profile.merge(hit_profile);
                    Some(StoredDocument {
                        index: index.to_owned(),
                        id: id.to_string(),
                        source: Arc::new(source),
                    })
                })
                .collect()
        };
        record_source_fetch_profile(mode, fetch_started.elapsed(), profile);
        documents
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
                let id = self.uid_for_doc_id(doc_id)?;
                let blob = self.blob_for_doc_id(doc_id)?;
                let source = Arc::new(parse_source_blob(&blob, &self.source_store));
                Some((
                    doc_id,
                    StoredDocument {
                        index: index.to_owned(),
                        id: id.to_string(),
                        source,
                    },
                ))
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

    /// Lot C `C1b` sous-pas 2: whether this index's term dictionary was
    /// built with disk-backed postings. `SearchScoringContext`
    /// (surch-api `search.rs`) branches on this instead of
    /// [`Self::term_scoring_view`] (which reads the RAM channels,
    /// intentionally empty under disk mode) when deciding how to
    /// populate its OR-match scoring arena.
    pub fn postings_disk_backed(&self) -> bool {
        self.data.index.postings_disk_backed()
    }

    /// Lot C `C1b` sous-pas 2: decode `(field, term)`'s full postings
    /// from the disk segment — the OR-match scoring arena's data source
    /// when [`Self::postings_disk_backed`] is `true`. `None` when the
    /// field/term is unknown or carries no disk coverage.
    pub fn decode_term_for_scoring(&self, field: &str, term: &str) -> Option<(Vec<u32>, Vec<u32>)> {
        self.data.index.decode_from_segment(field, term)
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
            .map(|id| self.data.resolve_uid(id))
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
    pub fn search_cache_get(&self, index: &str, key: u64) -> Option<(Vec<u8>, bool)> {
        let cache = self
            .search_cache
            .read()
            .expect("search cache lock should not be poisoned");
        cache
            .get(index)
            .and_then(|entry| entry.entries.get(&key))
            .map(|entry| (entry.bytes.clone(), entry.direct_must_fused))
    }

    pub fn search_cache_put(&self, index: &str, key: u64, bytes: Vec<u8>, direct_must_fused: bool) {
        let mut cache = self
            .search_cache
            .write()
            .expect("search cache lock should not be poisoned");
        let entry = cache.entry(index.to_owned()).or_default();
        let value = SearchCacheEntry {
            bytes,
            direct_must_fused,
        };
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

    /// Lot C `C1b` sous-pas 2 test/ops hook: pin `index`'s disk-backed
    /// postings read path independently of the process-wide
    /// `SURCH_POSTINGS_DISK` env flag (`postings_disk_enabled` in
    /// `surch_index::postings`, latched for the process's lifetime by its
    /// `OnceLock` — a single test binary cannot flip it mid-run). MUST be
    /// called before any document is indexed into `index`: the override
    /// only takes effect at the next `PostingsBuilder::build_with_disk_flag`
    /// call (the next materialize/`_refresh`), and does not retroactively
    /// convert an already-built `TermDictionary`'s RAM/disk layout. See
    /// `crates/surch-api/tests/postings_disk_parity.rs` for the flag-ON ==
    /// flag-OFF parity gate this unlocks. No-op if `index` does not exist.
    pub fn set_postings_disk_enabled(&self, index: &str, enabled: bool) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            data.index.set_postings_disk_enabled(enabled);
        }
    }

    /// Plan segments S2 test/ops hook: pin `index`'s flush-by-budget
    /// threshold independently of the process-wide `SURCH_FLUSH_BUDGET_BYTES`
    /// env var (same `OnceLock`-cannot-flip-mid-run rationale as
    /// [`Self::set_postings_disk_enabled`]). `Some(bytes)` forces that
    /// exact budget (used by parity tests to force real multi-segment
    /// indexing deterministically in-process); `None` forces "no budget"
    /// (mono-segment) regardless of the env var. No-op if `index` does
    /// not exist.
    pub fn set_flush_budget_bytes_override(&self, index: &str, budget: Option<u64>) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            data.index.set_flush_budget_bytes_override(budget);
        }
    }

    /// Plan segments S3 test/ops hook: pin `index`'s tiered-merge fan-in
    /// independently of the process-wide `SURCH_MERGE_FANIN` env var (same
    /// `OnceLock`-cannot-flip-mid-run rationale as
    /// [`Self::set_flush_budget_bytes_override`]). `0` forces "merge
    /// disabled" (used by parity tests to prove the un-merged multi-segment
    /// engine is unaffected by this feature); any other value forces that
    /// exact fan-in. No-op if `index` does not exist.
    pub fn set_merge_fanin_override(&self, index: &str, fanin: usize) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            data.index.set_merge_fanin_override(fanin);
        }
    }

    /// Plan segments S3b (design doc §S3b, "cap de taille de tier") test/ops
    /// hook: pin `index`'s tiered-merge doc-count cap independently of the
    /// process-wide `SURCH_MERGE_MAX_DOCS` env var (same
    /// `OnceLock`-cannot-flip-mid-run rationale as
    /// [`Self::set_flush_budget_bytes_override`]). `Some(docs)` forces that
    /// exact cap (used by parity tests to force real, smaller-than-corpus
    /// merged segments deterministically); `None` forces "no cap"
    /// regardless of the env var. No-op if `index` does not exist.
    pub fn set_merge_max_docs_override(&self, index: &str, max_docs: Option<u64>) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            data.index.set_merge_max_docs_override(max_docs);
        }
    }

    /// C1 fix (design doc §C1) test/ops hook: pin `index`'s id-maps
    /// densify-by-budget threshold independently of the process-wide
    /// `SURCH_DENSIFY_BUDGET_DOCS` env var (same
    /// `OnceLock`-cannot-flip-mid-run rationale as
    /// [`Self::set_flush_budget_bytes_override`]). `Some(docs)` forces
    /// that exact doc-count budget (used by parity tests to force the
    /// incremental fast path deterministically, in-process, across
    /// several `_bulk` chunks); `None` forces "no budget" (overlay only
    /// drained at an explicit `_refresh`, today's behaviour) regardless
    /// of the env var. No-op if `index` does not exist.
    pub fn set_densify_budget_docs_override(&self, index: &str, budget: Option<u64>) {
        let mut store = self
            .store
            .write()
            .expect("in-memory API state lock should not be poisoned");
        if let Some(data) = store.indices.get_mut(index) {
            data.densify_budget_override = DensifyBudgetOverride::Forced(budget);
        }
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

    /// Plan segments S2: number of sealed/active segments currently held
    /// by the named index (`1` for an unknown index or one that has
    /// never been written, matching a fresh mono-segment
    /// `DocumentIndex`). Used by the flush-by-budget parity test to
    /// assert a forced tiny budget actually produced real multi-segment
    /// indexing, and available as a diagnostic hook more generally.
    pub fn index_segment_count(&self, index: &str) -> usize {
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map_or(1, |data| data.index.segment_count())
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
                    } else if let Some(doc_id) = data.resolve_uid(&id) {
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
                        if let Some(doc_id) = data.resolve_uid(&id) {
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
            .map_or(0, |index| u64::from(index.live_count))
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
        // Plan segments S5c (`docs/paper/design-segments-pic-borne-2026-07-05.md`
        // §S5c): `DocumentIndex::subfield_projection` now owns the
        // "walk every sealed segment, materialize a disk-spilled column,
        // translate its LOCAL doc_ids back to GLOBAL" plumbing (it needs
        // the owning segment's `subfield_segment` file handle, which
        // `subfield_values_maps()` never exposed cross-crate) — this
        // layer only maps the GLOBAL doc_id to the public `_id`, exactly
        // as before.
        let pairs = data.index.subfield_projection(field_path)?;
        let mut projection = BTreeMap::new();
        for (doc_id, value) in pairs {
            if let Some(public_id) = data.uid_for_doc_id(doc_id) {
                projection.insert(public_id.to_string(), value);
            }
        }
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

    /// Lot C `C2` : ordre d'iteration change de l'ordre lexicographique
    /// sur l'uid public (ancienne `BTreeMap<Arc<str>, _>` key order) vers
    /// l'ordre d'INSERTION (`doc_id` ascendant, trous sautes) — le seul
    /// ordre qu'une structure dense indexee par `doc_id` supporte sans
    /// re-trier. Aucun test n'epinglait l'ordre lexicographique (verifie
    /// par grep avant ce refactor) ; cet ordre est d'ailleurs plus proche
    /// du comportement reel d'OpenSearch pour `match_all` sans `sort`
    /// explicite (ordre Lucene interne par segment, PAS un tri sur
    /// `_id`) — a reverifier neanmoins via le gate oracle.
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
                data.live_doc_ids().filter_map(move |doc_id| {
                    let id = data.uid_for_doc_id(doc_id)?;
                    let blob = data.blob_for_doc_id(doc_id)?;
                    Some(StoredDocument {
                        index: index.to_owned(),
                        id: id.to_string(),
                        source: Arc::new(parse_source_blob(&blob, &data.source_store)),
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
            .map_or(0, |data| u64::from(data.live_count))
    }

    /// Returns documents at positions `[from, from + size)` in the
    /// index's stable iteration order — Lot C `C2` : `doc_id` ascendant
    /// (insertion order), see [`Self::documents`]'s doc comment for the
    /// ordering-contract change from the previous `_id`-lexicographic
    /// order. Only the requested window is cloned, so the `match_all`
    /// top-K shortcut clones K sources instead of N. Returns an empty vec
    /// when `index` does not exist or when `from` lands past the last
    /// document.
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
        data.live_doc_ids()
            .skip(from)
            .take(size)
            .filter_map(|doc_id| {
                let id = data.uid_for_doc_id(doc_id)?;
                let blob = data.blob_for_doc_id(doc_id)?;
                Some(StoredDocument {
                    index: index.to_owned(),
                    id: id.to_string(),
                    // #15 + option B + `mmap M1` : parse le `_source`
                    // stocke (OnDisk -> pread, Compressed -> decode
                    // thread-local) en `Value` ; cette voie ne traite que
                    // la fenetre `[from..from+size)`.
                    source: Arc::new(parse_source_blob(&blob, &data.source_store)),
                })
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
        self.fused_conjunction_scores_with_mode(index, clauses, FusedConjunctionScoreMode::Should)
    }

    /// Calcule deux feuilles `bool.must` mono-token sans perdre une
    /// contribution BM25 égale à `1.0`; toute autre forme décline.
    pub fn fused_must_scores(
        &self,
        index: &str,
        clauses: &[(&str, &str)],
    ) -> Option<Vec<(f64, u32)>> {
        if clauses.len() != 2 {
            return None;
        }
        self.fused_conjunction_scores_with_mode(index, clauses, FusedConjunctionScoreMode::Must)
    }

    fn fused_conjunction_scores_with_mode(
        &self,
        index: &str,
        clauses: &[(&str, &str)],
        mode: FusedConjunctionScoreMode,
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

        // P2 couvre maintenant le disque et le multi-segment avec une lecture
        // checked : un `bool.must` direct peut donc emprunter ce chemin sans
        // confondre l'absence locale d'un terme avec une erreur de lecture.
        if data.index.postings_disk_backed() {
            return Self::fused_conjunction_scores_disk(data, clauses, mode);
        }

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
        // (same `bm25_score` primitive + guards) but with the captured `freq`.
        let term_contrib = |t: &TermCtx<'_>, doc_id: u32, freq: u32| -> f64 {
            if freq == 0 || t.doc_freq == 0 || t.doc_freq > t.field_stats.doc_count {
                return fused_bm25_contribution(None, mode);
            }
            let doc_len = if t.field_stats.norms_enabled {
                match t.field_stats.doc_len(doc_id) {
                    Some(len) => len,
                    None => return fused_bm25_contribution(None, mode),
                }
            } else {
                1
            };
            let score = bm25_score(
                config,
                t.field_stats.doc_count,
                t.doc_freq,
                u64::from(freq),
                doc_len,
                t.field_stats.avg_doc_len,
            )
            .ok();
            fused_bm25_contribution(score, mode)
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

    /// Lot P2 : scoring fusionné checked, segment par segment. Chaque terme
    /// porte son `df` global pour l'IDF, même lorsqu'il est absent d'un autre
    /// segment ; seul le choix du driver utilise le `df` local. Une erreur de
    /// lecture abandonne les scores temporaires et décline vers la référence.
    fn fused_conjunction_scores_disk(
        data: &InMemoryIndex,
        clauses: &[(&str, &str)],
        mode: FusedConjunctionScoreMode,
    ) -> Option<Vec<(f64, u32)>> {
        let mut segmented_terms = Vec::with_capacity(clauses.len());
        let mut scoring_terms = Vec::with_capacity(clauses.len());
        for &(field, value) in clauses {
            let recall = normalized_terms_for_field(value, field, &data.mapping);
            if recall.len() != 1 || recall != data.mapping.analyzer(field).terms(value) {
                return None;
            }
            let token = recall.into_iter().next().expect("len checked == 1");
            let postings = match data.index.segmented_postings_checked(field, &token) {
                Ok(Some(postings)) => postings,
                // Le terme est absent de tous les segments : l'AND est vide.
                Ok(None) => return Some(Vec::new()),
                // La distinction checked impose un repli, jamais un préfixe.
                Err(_) => return Self::fused_conjunction_scores_merged(data, clauses, mode),
            };
            let field_stats = data.field_scoring_stats(field)?;
            scoring_terms.push((field_stats, postings.global_df()));
            segmented_terms.push(postings);
        }

        let config = Bm25Config::default();

        let term_contrib =
            |field_stats: &FieldScoringStats<'_>, doc_freq: u64, doc_id: u32, freq: u32| -> f64 {
                if freq == 0 || doc_freq == 0 || doc_freq > field_stats.doc_count {
                    return fused_bm25_contribution(None, mode);
                }
                let doc_len = if field_stats.norms_enabled {
                    match field_stats.doc_len(doc_id) {
                        Some(len) => len,
                        None => return fused_bm25_contribution(None, mode),
                    }
                } else {
                    1
                };
                let score = bm25_score(
                    config,
                    field_stats.doc_count,
                    doc_freq,
                    u64::from(freq),
                    doc_len,
                    field_stats.avg_doc_len,
                )
                .ok();
                fused_bm25_contribution(score, mode)
            };

        let mut scored: Vec<(f64, u32)> = Vec::new();
        let mut block_metrics = P2DiskBlockMetrics::default();
        for segment_idx in 0..data.index.segment_count() {
            let mut cursors = Vec::with_capacity(segmented_terms.len());
            let mut missing_clause = false;
            for (clause_idx, postings) in segmented_terms.iter_mut().enumerate() {
                match postings.cursor_at_mut(segment_idx) {
                    Some(cursor) => cursors.push((clause_idx, cursor)),
                    // Une clause absente rend seulement ce segment vide.
                    None => {
                        missing_clause = true;
                        break;
                    }
                }
            }
            if missing_clause {
                let metric_cursors: Vec<_> = cursors.iter().map(|(_, cursor)| &**cursor).collect();
                block_metrics.observe(&metric_cursors);
                continue;
            }
            cursors.sort_by_key(|(_, cursor)| cursor.local_df());
            let segment_result = (|| -> Result<(), ()> {
                let (driver, followers) = cursors.split_at_mut(1);
                let (driver_clause, driver_cursor) = &mut driver[0];
                let mut current = Vec::with_capacity(followers.len());
                for (_, follower) in followers.iter_mut() {
                    current.push(match follower.advance_to_with_status(0) {
                        SegmentPostingsAdvance::Found(doc_id) => Some(doc_id),
                        SegmentPostingsAdvance::Exhausted => None,
                        SegmentPostingsAdvance::Error => return Err(()),
                    });
                }

                let mut next_probe = 0u32;
                let mut frequencies = vec![0u32; scoring_terms.len()];
                'docs: loop {
                    let doc_id = match driver_cursor.advance_to_with_status(next_probe) {
                        SegmentPostingsAdvance::Found(doc_id) => doc_id,
                        SegmentPostingsAdvance::Exhausted => break,
                        SegmentPostingsAdvance::Error => return Err(()),
                    };
                    next_probe = doc_id.saturating_add(1);
                    frequencies[*driver_clause] = driver_cursor.freq();
                    for (follower_idx, (clause_idx, follower)) in followers.iter_mut().enumerate() {
                        if current[follower_idx].is_some_and(|found| found < doc_id) {
                            current[follower_idx] = match follower.advance_to_with_status(doc_id) {
                                SegmentPostingsAdvance::Found(found) => Some(found),
                                SegmentPostingsAdvance::Exhausted => None,
                                SegmentPostingsAdvance::Error => return Err(()),
                            };
                        }
                        if current[follower_idx] != Some(doc_id) {
                            continue 'docs;
                        }
                        frequencies[*clause_idx] = follower.freq();
                    }
                    // L'ordre de sommation suit les clauses de la requête,
                    // jamais le tri local des drivers : les bits restent stables.
                    let mut sum = 0.0;
                    for (clause_idx, (field_stats, doc_freq)) in scoring_terms.iter().enumerate() {
                        sum +=
                            term_contrib(field_stats, *doc_freq, doc_id, frequencies[clause_idx]);
                    }
                    scored.push((if sum > 0.0 { sum } else { 1.0 }, doc_id));
                }
                Ok(())
            })();
            let metric_cursors: Vec<_> = cursors.iter().map(|(_, cursor)| &**cursor).collect();
            block_metrics.observe(&metric_cursors);
            if segment_result.is_err() {
                block_metrics.publish();
                return Self::fused_conjunction_scores_merged(data, clauses, mode);
            }
        }
        block_metrics.publish();
        Some(scored)
    }

    /// Référence matérialisée de repli pour
    /// [`Self::fused_conjunction_scores_disk`]. P2 la conserve uniquement
    /// lorsqu'une lecture checked échoue, jamais comme parcours multi-segment
    /// nominal.
    fn fused_conjunction_scores_merged(
        data: &InMemoryIndex,
        clauses: &[(&str, &str)],
        mode: FusedConjunctionScoreMode,
    ) -> Option<Vec<(f64, u32)>> {
        struct MergedTermCtx<'a> {
            field_stats: FieldScoringStats<'a>,
            doc_freq: u64,
            doc_ids: Vec<u32>,
            freqs: Vec<u32>,
        }
        let mut terms: Vec<MergedTermCtx<'_>> = Vec::with_capacity(clauses.len());
        for &(field, value) in clauses {
            let recall = normalized_terms_for_field(value, field, &data.mapping);
            if recall.len() != 1 || recall != data.mapping.analyzer(field).terms(value) {
                return None;
            }
            let token = recall.into_iter().next().expect("len checked == 1");
            let Some((doc_ids, freqs)) = data.index.decode_from_segment(field, &token) else {
                // A required term with no postings ⇒ the intersection is empty.
                return Some(Vec::new());
            };
            if doc_ids.is_empty() {
                return Some(Vec::new());
            }
            let field_stats = data.field_scoring_stats(field)?;
            let doc_freq = doc_ids.len() as u64;
            terms.push(MergedTermCtx {
                field_stats,
                doc_freq,
                doc_ids,
                freqs,
            });
        }

        // Drive the rarest term; same ordering rationale as the RAM/disk
        // paths.
        terms.sort_by_key(|t| t.doc_ids.len());
        let config = Bm25Config::default();

        // Same `term_contrib` kernel as the RAM/disk paths.
        let term_contrib =
            |field_stats: &FieldScoringStats<'_>, doc_freq: u64, doc_id: u32, freq: u32| -> f64 {
                if freq == 0 || doc_freq == 0 || doc_freq > field_stats.doc_count {
                    return fused_bm25_contribution(None, mode);
                }
                let doc_len = if field_stats.norms_enabled {
                    match field_stats.doc_len(doc_id) {
                        Some(len) => len,
                        None => return fused_bm25_contribution(None, mode),
                    }
                } else {
                    1
                };
                let score = bm25_score(
                    config,
                    field_stats.doc_count,
                    doc_freq,
                    u64::from(freq),
                    doc_len,
                    field_stats.avg_doc_len,
                )
                .ok();
                fused_bm25_contribution(score, mode)
            };

        let mut scored: Vec<(f64, u32)> = Vec::new();
        'docs: for (idx, &doc_id) in terms[0].doc_ids.iter().enumerate() {
            let mut sum = term_contrib(
                &terms[0].field_stats,
                terms[0].doc_freq,
                doc_id,
                terms[0].freqs[idx],
            );
            for follower in &terms[1..] {
                match follower.doc_ids.binary_search(&doc_id) {
                    Ok(pos) => {
                        sum += term_contrib(
                            &follower.field_stats,
                            follower.doc_freq,
                            doc_id,
                            follower.freqs[pos],
                        );
                    }
                    Err(_) => continue 'docs,
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

        // Lot C `C1b` sous-pas 2: disk-backed path. See
        // `Self::conjunction_of_matches_disk`'s doc comment for why this
        // one (unlike `conjunction_hits_internal`/`fused_conjunction_scores`)
        // takes the design's sanctioned "decode owned, correct first"
        // fallback rather than a pure block-addressed multi-cursor merge.
        if data.index.postings_disk_backed() {
            return Self::conjunction_of_matches_disk(data, clauses);
        }

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

    /// Lot C `C1b` sous-pas 2: disk-backed counterpart of
    /// [`Self::conjunction_of_matches`]. Unlike the pure leapfrog
    /// conjunctions (`InMemoryIndex::conjunction_hits_disk`,
    /// `Self::fused_conjunction_scores_disk`), this function's per-clause
    /// OR-of-tokens shape does not reduce to a single monotonic cursor as
    /// directly: a clause can match via ANY of several tokens, so
    /// testing "does clause C contain doc_id X" is not one cursor's
    /// `advance_to` but a small union/intersection over several. Rather
    /// than hand-roll a new multi-cursor merge for this one candidate-
    /// resolution helper, every needed token is decoded ONCE, in full,
    /// via [`DocumentIndex::decode_from_segment`] — the design's
    /// sanctioned "decode owned, correct first" fallback — and the REST
    /// of the algorithm below is byte-for-byte [`Self::conjunction_of_matches`]'s,
    /// just sourced from owned `Vec<u32>` instead of
    /// `PostingsList::doc_ids()`. A future sous-pas can revisit this with
    /// a block-addressed multi-cursor merge if it proves hot under disk
    /// mode.
    fn conjunction_of_matches_disk(
        data: &InMemoryIndex,
        clauses: &[(&str, &str, bool)],
    ) -> Option<Vec<u32>> {
        struct ClauseTokensOwned {
            require_all: bool,
            token_doc_ids: Vec<Vec<u32>>,
        }
        let mut clause_tokens: Vec<ClauseTokensOwned> = Vec::with_capacity(clauses.len());
        for &(field, value, require_all) in clauses {
            let tokens = normalized_terms_for_field(value, field, &data.mapping);
            if tokens.is_empty() {
                return Some(Vec::new());
            }
            let mut token_doc_ids: Vec<Vec<u32>> = Vec::with_capacity(tokens.len());
            for token in &tokens {
                match data.index.decode_from_segment(field, token) {
                    Some((doc_ids, _freqs)) => token_doc_ids.push(doc_ids),
                    None if require_all => return Some(Vec::new()),
                    None => {}
                }
            }
            if token_doc_ids.is_empty() {
                return Some(Vec::new());
            }
            clause_tokens.push(ClauseTokensOwned {
                require_all,
                token_doc_ids,
            });
        }

        let estimate = |c: &ClauseTokensOwned| -> usize {
            if c.require_all {
                c.token_doc_ids.iter().map(Vec::len).min().unwrap_or(0)
            } else {
                c.token_doc_ids.iter().map(Vec::len).sum()
            }
        };
        let driver_idx = (0..clause_tokens.len())
            .min_by_key(|&i| estimate(&clause_tokens[i]))
            .expect("clause_tokens is non-empty");

        let clause_contains = |c: &ClauseTokensOwned, doc_id: u32| -> bool {
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

        let driver = &clause_tokens[driver_idx];
        let driver_docs: Vec<u32> = if driver.token_doc_ids.len() == 1 {
            driver.token_doc_ids[0].clone()
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
        public_ids.iter().map(|id| data.resolve_uid(id)).collect()
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
        // Lot C `C2` : plus de `documents: BTreeMap<Arc<str>, SourceBlob>`
        // a `.values()`-scanner — `live_doc_ids()` fusionne le snapshot
        // dense (trous exclus) et l'overlay dirty (inserts/updates depuis
        // le dernier `_refresh`), meme resolution que toute lecture
        // (`blob_for_doc_id`) donc pas de double-comptage d'un doc mis a
        // jour (le blob perime en `dense` est ignore au profit du blob
        // dirty courant).
        usage.stored_fields_bytes = data
            .live_doc_ids()
            .filter_map(|doc_id| data.blob_for_doc_id(doc_id))
            .map(|blob| blob.payload_len() as u64)
            .sum();
        Some(usage)
    }

    /// API-side state overhead for `index` — the bytes NOT already counted by
    /// [`index_memory_usage`]. Returns `(documents_overhead, id_maps)`.
    ///
    /// Lot C `C2` : les 3 anciennes `BTreeMap` ont disparu. `documents_overhead`
    /// couvre maintenant `dense.documents` (un slot par `doc_id`, ZERO
    /// overhead par-entree au-dela du slot lui-meme — plus de nœud
    /// `BTreeMap`, plus de cle `Arc<str>` dupliquee) plus l'overlay
    /// `documents_dirty: HashMap<u32, SourceBlob>` (borne par la taille
    /// d'un batch bulk, pas par le corpus — sauf si
    /// `SURCH_DENSIFY_BUDGET_DOCS` est absent ET qu'aucun `_refresh`
    /// n'est jamais appele, voir le design doc §C1). `id_maps` couvre le
    /// snapshot dense (`reverse_uids` + `reverse_offsets` empaquetes,
    /// `forward` FST) plus les 3 overlays UID
    /// (`forward_dirty`/`reverse_dirty`/`deleted_since_dense`).
    ///
    /// C1 fix : `dense.documents`/`.reverse_uids`/`.reverse_offsets` sont
    /// des `Vec<T>` (pas des `Box<[T]>`) depuis ce fix — voir
    /// `DenseIdMaps`'s doc comment — donc `.capacity()` (pas `.len()`)
    /// est la mesure honnete de leur empreinte RAM reelle : un `Vec`
    /// etendu par tranches (`InMemoryIndex::densify_append_only`) peut
    /// porter du slack de croissance non utilise entre deux appels.
    ///
    /// Le FST `forward` et le buffer `reverse_uids` ENCODENT chacun les
    /// octets UTF-8 des UID separement (pas de partage entre les deux
    /// representations, contrairement a l'ancien "levier 3" qui partageait
    /// un seul `Arc<str>` entre 3 `BTreeMap`) — mais chacun le fait dans
    /// UN buffer contigu par index au lieu d'~1,36 M allocations
    /// individuelles : c'est l'echange qui tue la fragmentation interne
    /// mesuree (~671 MiB, cf. commit "gauges internes jemalloc"), au prix
    /// d'un doublement logique des octets UID bruts (~60 MiB sur deces,
    /// negligeable face au gain de fragmentation).
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
        // Overhead approximatif par entree `HashMap` (hashbrown : 1 octet
        // de controle + slack de bucket) — meme ordre de grandeur que
        // l'ancien `BTREE_NODE_OVERHEAD`, reutilise ici pour les 4
        // overlays dirty (bulk-batch sized, PAS O(corpus)).
        const HASH_ENTRY_OVERHEAD: u64 = 48;
        // Lot C Phase 1 levier 3 (reutilise) : en-tête `ArcInner` (strong +
        // weak `AtomicUsize`, 16 octets) d'un buffer `Arc<str>` — seuls
        // les UID encore dans un overlay dirty (pas densifies) en paient
        // le cout ici.
        const ARC_HEADER: u64 = 16;
        // `documents_dirty: HashMap<u32, SourceBlob>` reste tel quel (pas
        // touche par le plan 2c) — `source_blob_slot` continue de mesurer
        // SON cout par entree.
        let source_blob_slot = std::mem::size_of::<Option<SourceBlob>>() as u64;
        // Plan 2c : `dense.documents` est un `Vec<u64>` packe, 8 o/slot
        // (contre `source_blob_slot` ~24 o avant ce plan) — c'est le gain
        // −433 Mio anon a 28,9M docs que le plan vise.
        let packed_document_slot = std::mem::size_of::<u64>() as u64;
        let u32_size = std::mem::size_of::<u32>() as u64;

        // `.capacity()`, not `.len()`: these three buffers are `Vec<T>`
        // (C1 fix) and can carry unused growth slack between two
        // `densify_append_only` tranches — see `DenseIdMaps`'s doc
        // comment. `.capacity()` is the honest resident-bytes measure.
        let dense_documents_bytes =
            (data.dense.documents.capacity() as u64).saturating_mul(packed_document_slot);
        let dirty_documents_bytes = (data.documents_dirty.len() as u64)
            .saturating_mul(HASH_ENTRY_OVERHEAD + u32_size + source_blob_slot);
        let documents_overhead = dense_documents_bytes.saturating_add(dirty_documents_bytes);

        let dense_reverse_bytes = (data.dense.reverse_uids.capacity() as u64).saturating_add(
            (data.dense.reverse_offsets.capacity() as u64).saturating_mul(u32_size),
        );
        let dense_forward_bytes = data
            .dense
            .forward
            .as_ref()
            .map_or(0, |map| map.as_fst().as_bytes().len() as u64);
        // `forward_dirty` porte les octets UTF-8 de l'UID (cle) ; `reverse_dirty`
        // partage le MEME `Arc<str>` (voir `upsert_document_deferred`) donc ne
        // recompte que son en-tête `Arc`/overhead de bucket, pas les octets.
        let forward_dirty_bytes: u64 = data
            .forward_dirty
            .keys()
            .map(|uid| HASH_ENTRY_OVERHEAD + ARC_HEADER + uid.len() as u64)
            .sum();
        let reverse_dirty_bytes =
            (data.reverse_dirty.len() as u64).saturating_mul(HASH_ENTRY_OVERHEAD + u32_size);
        let deleted_since_dense_bytes =
            (data.deleted_since_dense.len() as u64).saturating_mul(HASH_ENTRY_OVERHEAD + u32_size);

        let id_maps = dense_reverse_bytes
            .saturating_add(dense_forward_bytes)
            .saturating_add(forward_dirty_bytes)
            .saturating_add(reverse_dirty_bytes)
            .saturating_add(deleted_since_dense_bytes);
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

    /// Lot C `C1a-batché` hardening : nombre de termes sans couverture
    /// disque suite a un echec d'encode FoR au build
    /// (`surch_index_disk_postings_skipped_terms`). Diagnostic : couple
    /// avec `index_disk_postings_bytes` ci-dessus pour distinguer un
    /// crash cause par des doc_id dupliques (`skipped_terms > 0`) d'un
    /// echec IO/tmpfs (`skipped_terms == 0` mais `bytes == 0`). Meme
    /// pattern que `index_disk_postings_bytes` (materialise le FST en
    /// attente avant de lire).
    pub fn index_disk_postings_skipped_terms(&self, index: &str) -> Option<u64> {
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map(|data| data.index.postings_segment_skipped_terms())
    }

    /// Plan segments S5c : taille on-disk des segments de spill sub-field
    /// (`surch_index_disk_subfield_values_bytes`), ecrite par
    /// `Segment::seal_subfield_columns` (batché par colonne). Meme
    /// pattern que `index_disk_postings_bytes` (materialise le FST en
    /// attente avant de lire, pour refleter le dernier scellement).
    pub fn index_disk_subfield_values_bytes(&self, index: &str) -> Option<u64> {
        self.ensure_terms_ready(index);
        let store = self
            .store
            .read()
            .expect("in-memory API state lock should not be poisoned");
        store
            .indices
            .get(index)
            .map(|data| data.index.subfield_segment_bytes())
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
            .map(|data| u64::from(data.live_count))
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
