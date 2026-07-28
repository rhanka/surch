//! `GET /_surch/stats` — RAM accounting endpoint plus the Prometheus
//! gauges that mirror it.
//!
//! Surch keeps every index in memory; sizing a cluster (notably the
//! matchID INSEE indexer with ~1.3 M docs) depends on knowing how
//! many bytes the postings, the prefix-postings side table, the
//! field-length stats, the term block metas, and the stored `_source`
//! payloads consume.
//!
//! Les tailles sont rafraîchies à l'indexation (`_bulk`, `_doc`, delete,
//! import de snapshot). P3 publie aussi ses compteurs d'attestation après une
//! tentative de chemin à sauts ; cela ne modifie ni le résultat ni le chemin
//! `match` historique.

use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock, TryLockError},
    time::{Duration, Instant},
};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surch_index::{memory::MemoryUsage, postings::P2IntegrityMetrics};

use crate::state::AppState;

/// Prometheus gauge: per-field-component byte counts and total. Refreshed
/// after every write to the relevant index. The `index` label is bounded
/// by the number of physical indices in the cluster; there is no
/// per-document or per-query cardinality blow-up.
pub fn refresh_memory_gauges(state: &AppState, index: &str) {
    let usage = state.index_memory_usage(index).unwrap_or_default();
    let doc_count = state.index_doc_count(index).unwrap_or(0);
    let (state_docs_overhead, state_id_maps) =
        state.index_state_memory_bytes(index).unwrap_or((0, 0));
    let disk_segment_bytes = state.index_disk_segment_bytes(index).unwrap_or(0);
    let disk_segment_peak_bytes = state.index_disk_segment_peak_bytes(index).unwrap_or(0);
    let disk_postings_bytes = state.index_disk_postings_bytes(index).unwrap_or(0);
    let disk_postings_skipped_terms = state.index_disk_postings_skipped_terms(index).unwrap_or(0);
    let disk_subfield_values_bytes = state.index_disk_subfield_values_bytes(index).unwrap_or(0);
    let p2_integrity_metrics = state.index_p2_integrity_metrics(index).unwrap_or_default();
    let segment_count = state.index_segment_count(index);
    set_gauges(
        index,
        doc_count,
        &usage,
        state_docs_overhead,
        state_id_maps,
        disk_segment_bytes,
        disk_segment_peak_bytes,
        disk_postings_bytes,
        disk_postings_skipped_terms,
        disk_subfield_values_bytes,
        segment_count,
    );
    set_p2_integrity_gauges(index, p2_integrity_metrics);
    refresh_runtime_memory_gauges();
}

const RUNTIME_MEMORY_REFRESH_MAX_AGE: Duration = Duration::from_secs(1);

#[derive(Default)]
struct RuntimeMemoryRefreshCache {
    last_attempt: Option<Instant>,
    last_success: Option<Instant>,
    success: bool,
}

static RUNTIME_MEMORY_REFRESH_CACHE: OnceLock<Mutex<RuntimeMemoryRefreshCache>> = OnceLock::new();

fn publish_runtime_memory_refresh(cache: &RuntimeMemoryRefreshCache, now: Instant) {
    let age_seconds = cache
        .last_success
        .map(|success| now.duration_since(success).as_secs_f64())
        .unwrap_or(-1.0);
    metrics::gauge!("surch_runtime_memory_refresh_success").set(if cache.success {
        1.0
    } else {
        0.0
    });
    metrics::gauge!("surch_runtime_memory_refresh_age_seconds").set(age_seconds);
}

/// Rafraîchit au plus une fois par seconde les jauges mémoire propres au
/// processus sans relire un index.
///
/// Un scrape concurrent ne bloque pas derrière une lecture de `/proc` ou un
/// `mallctl` : il sert le dernier état publié. Chaque échec est visible via
/// `surch_runtime_memory_refresh_success=0` et
/// `surch_runtime_memory_refresh_errors_total`, de sorte qu'une valeur
/// précédente ne puisse jamais être confondue avec une mesure fraîche.
pub(crate) fn refresh_runtime_memory_gauges() {
    let now = Instant::now();
    let cache = RUNTIME_MEMORY_REFRESH_CACHE
        .get_or_init(|| Mutex::new(RuntimeMemoryRefreshCache::default()));
    let mut cache = match cache.try_lock() {
        Ok(cache) => cache,
        Err(TryLockError::WouldBlock) => return,
        Err(TryLockError::Poisoned(_)) => {
            metrics::gauge!("surch_runtime_memory_refresh_success").set(0.0);
            metrics::counter!("surch_runtime_memory_refresh_errors_total").increment(1);
            return;
        }
    };
    if cache
        .last_attempt
        .is_some_and(|attempt| now.duration_since(attempt) < RUNTIME_MEMORY_REFRESH_MAX_AGE)
    {
        publish_runtime_memory_refresh(&cache, now);
        return;
    }

    cache.last_attempt = Some(now);
    match refresh_process_memory_gauges().and_then(|()| refresh_jemalloc_gauges()) {
        Ok(()) => {
            cache.last_success = Some(now);
            cache.success = true;
        }
        Err(_) => {
            cache.success = false;
            metrics::counter!("surch_runtime_memory_refresh_errors_total").increment(1);
        }
    }
    publish_runtime_memory_refresh(&cache, now);
}

/// Jauges P3 mises à jour à l'indexation et après chaque tentative P2. Les
/// tailles sont instantanées ; les compteurs de vérification et de déclin sont
/// cumulatifs pour la génération de segments courante.
pub(crate) fn set_p2_integrity_gauges(index: &str, integrity: P2IntegrityMetrics) {
    let label = [("index", index.to_owned())];
    metrics::gauge!("surch_postings_p2_integrity_bytes", &label)
        .set(integrity.integrity_bytes as f64);
    metrics::gauge!("surch_postings_p2_integrity_pages", &label)
        .set(integrity.integrity_pages as f64);
    metrics::gauge!("surch_postings_p2_verified_bytes", &label)
        .set(integrity.verified_bytes as f64);
    metrics::gauge!("surch_postings_p2_hash_failures", &label).set(integrity.hash_failures as f64);
    metrics::gauge!("surch_postings_p2_fallbacks", &label).set(integrity.fallbacks as f64);
    metrics::gauge!("surch_postings_p2_fallback_fields", &label)
        .set(integrity.fallback_fields as f64);
    metrics::gauge!("surch_postings_p2_term_occurrences", &label)
        .set(integrity.term_occurrences as f64);
    metrics::gauge!("surch_postings_p2_blocks", &label).set(integrity.blocks as f64);
    metrics::gauge!("surch_postings_p2_fields", &label).set(integrity.fields as f64);
    metrics::gauge!("surch_postings_p2_term_payload_bytes", &label)
        .set(integrity.term_payload_bytes as f64);
    metrics::gauge!("surch_postings_p2_csr_bytes", &label).set(integrity.csr_bytes as f64);
    metrics::gauge!("surch_postings_p2_directory_bytes", &label)
        .set(integrity.directory_bytes as f64);
}

/// Process-wide RSS accounting (#17b). Reads `/proc/self/status` to
/// surface VmRSS / VmAnon / VmData / VmHWM as Prometheus gauges. Combined
/// with the per-index `surch_index_*` gauges this isolates the gap
/// between "what Surch thinks it stores" (~2.8 GiB after inc1) and the
/// kernel's resident view (~8.0 GiB). Linux-only.
#[cfg(target_os = "linux")]
fn refresh_process_memory_gauges() -> Result<(), String> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("lecture /proc/self/status impossible: {error}"))?;
    let mut rss = false;
    let mut hwm = false;
    let mut anon = false;
    for line in status.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let wanted = matches!(
            key,
            "VmRSS" | "VmHWM" | "VmData" | "RssAnon" | "RssFile" | "VmSize"
        );
        if !wanted {
            continue;
        }
        // `rest` looks like "    123456 kB"; jemalloc and the kernel both
        // expose this field in kibibytes. `split_whitespace` already skips
        // the leading run of whitespace, no trim needed.
        let kib_str = rest.split_whitespace().next().unwrap_or("0");
        let kib = kib_str
            .parse::<u64>()
            .map_err(|error| format!("valeur mémoire /proc invalide pour {key}: {error}"))?;
        let bytes = kib.saturating_mul(1024) as f64;
        match key {
            "VmRSS" => {
                metrics::gauge!("surch_process_rss_bytes").set(bytes);
                rss = true;
            }
            "VmHWM" => {
                metrics::gauge!("surch_process_rss_peak_bytes").set(bytes);
                hwm = true;
            }
            "VmData" => metrics::gauge!("surch_process_data_bytes").set(bytes),
            "RssAnon" => {
                metrics::gauge!("surch_process_rss_anon_bytes").set(bytes);
                anon = true;
            }
            "RssFile" => metrics::gauge!("surch_process_rss_file_bytes").set(bytes),
            "VmSize" => metrics::gauge!("surch_process_vsize_bytes").set(bytes),
            _ => unreachable!("filtre wanted incohérent"),
        }
    }
    (rss && hwm && anon)
        .then_some(())
        .ok_or_else(|| "champs VmRSS, VmHWM ou RssAnon absents de /proc/self/status".to_owned())
}

#[cfg(not(target_os = "linux"))]
fn refresh_process_memory_gauges() -> Result<(), String> {
    Err("rafraîchissement mémoire runtime non pris en charge hors Linux".to_owned())
}

/// Lot C Phase 0b : stats INTERNES de l'allocateur jemalloc, exposées en
/// gauges Prometheus pour EXPLIQUER le gap heap anonyme (~1,5-2 GiB que
/// `set_gauges` n'attribue à aucune structure d'index comptée).
///
/// Lecture à l'état steady POST-`_refresh` : `refresh_memory_gauges` est
/// invoqué par `refresh_index` APRÈS `finalize_terms_for_refresh` (le
/// `PostingsBuilder` est déjà droppé), donc `allocated` reflète les
/// allocations vivantes résiduelles, pas le builder transitoire.
/// Interprétation visée :
///   - `resident − allocated ≈ 1,5-2 GiB` => fragmentation / pages dirty
///     retenues par l'allocateur (levier : decay / compaction) ;
///   - `allocated ≈ 3,8 GiB` => allocations VIVANTES non comptées
///     (levier : structures d'index encore attachées au heap).
///
/// Les compteurs jemalloc sont cachés : ils ne se rafraîchissent qu'en
/// avançant l'`epoch`. Sans cet advance on relirait des valeurs figées.
/// Nécessite la feature `stats` sur `tikv-jemalloc-ctl` (sinon le module
/// n'est pas compilé) ET sur le `tikv-jemalloc-sys` sous-jacent (sinon
/// jemalloc est `--disable-stats` et chaque `read()` renvoie une erreur),
/// laquelle remonte au rafraîchisseur runtime et est publiée par ses jauges
/// de succès, d'âge et d'erreurs. Zéro `unwrap`.
/// Linux-only : jemalloc n'est le `#[global_allocator]` que sur Linux.
#[cfg(target_os = "linux")]
fn refresh_jemalloc_gauges() -> Result<(), String> {
    use tikv_jemalloc_ctl::{epoch, stats};

    // Rafraîchit le cache de stats. `advance()` renvoie l'ancienne valeur
    // d'epoch (`Result<u64>`) ; on l'ignore, seul l'effet de bord compte.
    epoch::advance().map_err(|error| format!("jemalloc epoch::advance impossible: {error}"))?;

    // `read()` renvoie `Result<usize>` (`libc::size_t`). `usize as f64`
    // est exact jusqu'à 2^53 octets, très au-delà des tailles heap ici.
    let allocated = stats::allocated::read()
        .map_err(|error| format!("lecture jemalloc allocated impossible: {error}"))?;
    let active = stats::active::read()
        .map_err(|error| format!("lecture jemalloc active impossible: {error}"))?;
    let resident = stats::resident::read()
        .map_err(|error| format!("lecture jemalloc resident impossible: {error}"))?;
    let retained = stats::retained::read()
        .map_err(|error| format!("lecture jemalloc retained impossible: {error}"))?;
    let mapped = stats::mapped::read()
        .map_err(|error| format!("lecture jemalloc mapped impossible: {error}"))?;
    metrics::gauge!("surch_jemalloc_allocated_bytes").set(allocated as f64);
    metrics::gauge!("surch_jemalloc_active_bytes").set(active as f64);
    metrics::gauge!("surch_jemalloc_resident_bytes").set(resident as f64);
    metrics::gauge!("surch_jemalloc_retained_bytes").set(retained as f64);
    metrics::gauge!("surch_jemalloc_mapped_bytes").set(mapped as f64);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn refresh_jemalloc_gauges() -> Result<(), String> {
    Err("rafraîchissement jemalloc non pris en charge hors Linux".to_owned())
}

/// Lot C Phase 1 : purge EXPLICITE et SYNCHRONE des extents jemalloc
/// libérés par le free-storm du build aplati
/// (`materialize_terms_and_finalize_postings`, appelé juste avant ceci
/// par `finalize_terms_for_refresh` sur le chemin `_refresh`).
///
/// CONTEXTE MESURÉ : au `_refresh`, le build aplati libère ~745 MiB de
/// heap alloué (des millions de petites `Vec` par-terme remplacées par
/// quelques gros `Box<[T]>`), mais le RSS ne baisse que de ~376 MiB —
/// `resident − allocated` passe de 855 à 1225 MiB et `retained`
/// augmente de 141 MiB : jemalloc a bien commencé à rendre des pages,
/// mais n'a pas fini de purger tout ce que le free-storm vient de
/// libérer au moment où on lit les gauges.
///
/// POURQUOI UNE PURGE EXPLICITE AIDE MALGRÉ `dirty_decay_ms:0,
/// muzzy_decay_ms:0` (cf. `Dockerfile`) : ces réglages mettent le
/// DÉLAI de décroissance à zéro, mais la décroissance elle-même reste
/// OPPORTUNISTE — elle s'exécute au prochain tick du thread
/// `background_thread:true`, ou à la prochaine allocation/désallocation
/// qui retraverse l'arène concernée. Juste après un `_refresh`, le
/// process peut rester inactif un moment (pas de `_bulk`/recherche
/// immédiats) : les extents fraîchement libérés par LE free-storm du
/// build restent donc « dirty » (mappés, comptant dans le RSS) le temps
/// que l'évènement opportuniste suivant survienne. Un appel SYNCHRONE
/// au mallctl de purge force jemalloc à rendre ces pages au système
/// immédiatement, sans dépendre d'une activité allocateur ultérieure.
///
/// API TROUVÉE (inspection de la source du crate, PAS de mémoire) :
/// `tikv-jemalloc-ctl` 0.6.1 N'EXPOSE AUCUN binding — sûr ou même
/// unsafe — capable d'appeler `arena.<i>.purge`. Preuve :
/// - `~/.cargo/registry/.../tikv-jemalloc-sys-0.6.1+.../jemalloc/src/ctl.c`
///   définit `arena_i_purge_ctl` avec la macro `NEITHER_READ_NOR_WRITE()`
///   (même fichier, macro autour de la ligne 1797), qui fait
///   `if (oldp != NULL || oldlenp != NULL || newp != NULL || newlen != 0)
///   { ret = EPERM; ... }` : ce mallctl EST un pur déclencheur d'effet
///   de bord, sans lecture ni écriture — les QUATRE paramètres doivent
///   être NULL/0.
/// - `~/.cargo/registry/.../tikv-jemalloc-sys-.../jemalloc/include/jemalloc/jemalloc_macros.h.in`
///   documente l'usage exact :
///   `mallctl("arena." STRINGIFY(MALLCTL_ARENAS_ALL) ".purge", NULL,
///   NULL, NULL, 0)` avec `MALLCTL_ARENAS_ALL = 4096` — d'où le nom
///   `"arena.4096.purge"` utilisé ci-dessous pour purger TOUTES les
///   arènes en un seul appel (équivalent à `arenas.purge` sur des
///   jemalloc plus anciens, qui n'existe plus sous ce nom en 5.3.0).
/// - `~/.cargo/registry/.../tikv-jemalloc-ctl-0.6.1/src/raw.rs` : TOUTES
///   les primitives (`read`, `read_mib`, `write`, `write_mib`,
///   `update`, `update_mib`) prennent l'adresse d'une variable LOCALE
///   pour `oldp` et/ou `newp` — jamais NULL, même pour `T = ()` (une
///   référence Rust vers un zero-sized type reste un pointeur non-NULL
///   bien formé). Utiliser `raw::write_mib::<()>` échouerait donc
///   TOUJOURS avec `Err(EPERM)`, silencieusement avalé par une gestion
///   d'erreur sans `unwrap` — un piège qui aurait fait croire à une
///   purge fonctionnelle sans jamais purger quoi que ce soit.
/// - `~/.cargo/registry/.../tikv-jemalloc-ctl-0.6.1/src/arenas.rs` ne
///   modélise que `arenas::narenas` (lecture seule) ; aucun module du
///   crate n'expose de mallctl « call pur ».
///
/// VOIE CHOISIE : appel direct à `tikv_jemalloc_sys::mallctl` (le
/// binding FFI généré par le build script du crate `tikv-jemalloc-sys`,
/// qui résout déjà le bon nom de symbole selon que jemalloc est
/// compilé préfixé ou non — voir `#[cfg_attr(prefixed, link_name =
/// ...)]` dans `tikv-jemalloc-sys/src/lib.rs`). Ce crate est déjà
/// résolu de façon transitive (dépendance commune de
/// `tikv-jemallocator` et `tikv-jemalloc-ctl`, même version 0.6.1) ; il
/// est déclaré en dépendance directe Linux-only dans `Cargo.toml` pour
/// y accéder sans réécrire l'`extern "C"` à la main (fragile : le nom
/// de symbole change sous `#[cfg(prefixed)]`).
///
/// Appelée UNIQUEMENT sur le chemin `_refresh`
/// (`AppState::refresh_index`), jamais sur le hot path de requête :
/// c'est un évènement rare (un `_refresh` par batch d'indexation, pas
/// par requête de recherche).
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub fn refresh_jemalloc_purge() {
    // `arena.4096.purge` : 4096 = `MALLCTL_ARENAS_ALL`, purge toutes
    // les arènes en un seul appel. Nom null-terminé requis par
    // `mallctl`.
    const PURGE_ALL_ARENAS: &[u8] = b"arena.4096.purge\0";

    // SAFETY: `mallctl` est le point d'entrée de contrôle standard de
    // jemalloc — ce n'est pas un déréférencement de pointeur
    // utilisateur, juste un appel FFI vers l'allocateur déjà lié au
    // process (jemalloc est le `#[global_allocator]` sur Linux, cf.
    // `main.rs`). `PURGE_ALL_ARENAS` est un littéral `&'static [u8]`
    // valide et null-terminé, converti en `*const c_char` (cast
    // `u8` -> `i8`, même représentation mémoire). Les quatre autres
    // arguments sont NULL/0 exactement comme documenté par jemalloc
    // pour ce mallctl "call pur" (voir commentaire de fonction
    // ci-dessus) : jemalloc ne lit ni n'écrit aucune valeur à travers
    // ces pointeurs pour `arena.<i>.purge`, il déclenche seulement un
    // effet de bord synchrone (purge des extents dirty/muzzy de
    // chaque arène). Le retour est un `c_int` (0 = succès), vérifié
    // ci-dessous — pas de valeur produite à faire fuiter hors de ce
    // bloc `unsafe`.
    let ret = unsafe {
        tikv_jemalloc_sys::mallctl(
            PURGE_ALL_ARENAS.as_ptr() as *const libc::c_char,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    if ret != 0 {
        // Purge indisponible (jemalloc compilé sans les arènes
        // attendues, ou toute autre erreur mallctl) : skip silencieux,
        // comme le reste de ce module. Un `_refresh` ne doit jamais
        // échouer à cause d'une optimisation RSS best-effort.
        return;
    }
    // Purge synchrone terminée : relit les gauges jemalloc pour que
    // `surch_jemalloc_resident_bytes` (et les autres) reflètent l'état
    // POST-purge au prochain scrape, plutôt que le pic pré-purge
    // (sinon la gauge resterait gonflée jusqu'à la prochaine activité
    // allocateur qui avance l'epoch).
    if refresh_jemalloc_gauges().is_err() {
        metrics::gauge!("surch_runtime_memory_refresh_success").set(0.0);
        metrics::counter!("surch_runtime_memory_refresh_errors_total").increment(1);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn refresh_jemalloc_purge() {}

/// Drop the gauges for `index`. Called when an index is deleted so the
/// scrape body does not keep advertising stale totals.
pub fn clear_memory_gauges(index: &str) {
    set_gauges(index, 0, &MemoryUsage::default(), 0, 0, 0, 0, 0, 0, 0, 0);
    set_p2_integrity_gauges(index, P2IntegrityMetrics::default());
}

// Internal gauge-fan-out helper: one positional arg per Prometheus gauge
// family. The list grows by one each time a new disk/RAM accounting gauge
// is added (C1a added `disk_postings_bytes`, the C1a-batché hardening pass
// added `disk_postings_skipped_terms`); bundling into a struct would just
// move the same fields around without improving the single call site.
#[allow(clippy::too_many_arguments)]
fn set_gauges(
    index: &str,
    doc_count: u64,
    usage: &MemoryUsage,
    state_documents_overhead: u64,
    state_id_maps: u64,
    disk_segment_bytes: u64,
    disk_segment_peak_bytes: u64,
    disk_postings_bytes: u64,
    disk_postings_skipped_terms: u64,
    disk_subfield_values_bytes: u64,
    segment_count: usize,
) {
    let label = [("index", index.to_owned())];
    // P1 mmap M1 + axe disque #19 : taille on-disk effective du segment
    // `source.dat`. `_bytes` est la mesure instantanée (0 après refresh
    // car `compact_after_refresh` truncate). `_peak_bytes` retient le
    // pic depuis la creation — c'est la vraie mesure pour le scoreboard
    // axe disque (puisque le scrape arrive APRES `_refresh`).
    metrics::gauge!("surch_index_disk_segment_bytes", &label).set(disk_segment_bytes as f64);
    metrics::gauge!("surch_index_disk_segment_peak_bytes", &label)
        .set(disk_segment_peak_bytes as f64);
    // Lot C `C1a-batché` : segment FoR postings SHADOW (écrit, batché par
    // champ, mais aucun read path de prod ne le consomme — voir
    // `surch_index::postings::TermDictionary::decode_from_segment`).
    // Volontairement HORS `usage.total_bytes()` : ce sont des octets sur
    // disque (page-cache), pas de la RAM, et les mêmes postings sont
    // aujourd'hui AUSSI comptés dans `surch_index_postings_bytes` (RAM
    // reste la source de vérité tant que C1b n'a pas basculé le read
    // path) — les sommer doublerait le compte.
    metrics::gauge!("surch_index_disk_postings_bytes", &label).set(disk_postings_bytes as f64);
    // C1a-batché hardening : nombre de termes sans couverture disque suite
    // a un echec d'encode FoR (typiquement doc_id dupliques issus d'un
    // champ `Value::Array` eclate sans dedup en amont). Diagnostic couple
    // avec la gauge ci-dessus : `skipped_terms > 0` => doc_id dupliques ;
    // `skipped_terms == 0` mais `disk_postings_bytes == 0` => IO/tmpfs.
    metrics::gauge!("surch_index_disk_postings_skipped_terms", &label)
        .set(disk_postings_skipped_terms as f64);
    metrics::gauge!("surch_index_postings_bytes", &label).set(usage.postings_bytes as f64);
    metrics::gauge!("surch_index_prefix_postings_bytes", &label)
        .set(usage.prefix_postings_bytes as f64);
    metrics::gauge!("surch_index_stored_fields_bytes", &label)
        .set(usage.stored_fields_bytes as f64);
    metrics::gauge!("surch_index_field_stats_bytes", &label).set(usage.field_stats_bytes as f64);
    metrics::gauge!("surch_index_term_stats_bytes", &label).set(usage.term_stats_bytes as f64);
    // #17: break out the previously-unaccounted RSS portion.
    metrics::gauge!("surch_index_fst_bytes", &label).set(usage.fst_bytes as f64);
    metrics::gauge!("surch_index_roaring_bytes", &label).set(usage.roaring_bytes as f64);
    metrics::gauge!("surch_index_block_metas_bytes", &label).set(usage.block_metas_bytes as f64);
    // #17c: capacity slack on per-term Vec<Posting>/Vec<u32> — bytes allocated
    // but unused after the FST build (size-class rounding).
    metrics::gauge!("surch_index_postings_capacity_slack_bytes", &label)
        .set(usage.postings_capacity_slack_bytes as f64);
    // #17c walker complet : PostingsBuilder retenu Lot 1.5. Premier suspect
    // du gap heap ~4 GiB inexpliqué sur deces 1.36M (cf scoreboard
    // 2026-06-10-mesured.md). Walk les BTreeMap imbriqués + Vec capacity.
    metrics::gauge!("surch_index_postings_builder_bytes", &label)
        .set(usage.postings_builder_bytes as f64);
    // #17c walker complet — A10 multi-field side-table déjà calculée
    // dans `MemoryUsage` mais jamais exposée en gauge (suspect #2 confirmé
    // sur deces : 426 MiB = ~10 % du RSS). Inclut tous les `.raw`
    // subfields (PRENOM.raw + NOM.raw côté matchID).
    // Plan segments S5c : `dict`+`codes` de chaque `SubfieldColumn` sont
    // spillés dans un segment pread partagé par segment sous
    // SURCH_POSTINGS_DISK (même flag que S5a/S5b, pas de nouveau knob) —
    // cette gauge doit lire ~0 flag on (colonnes vidées, voir
    // `SubfieldColumn::spill`) ; les octets déplacés apparaissent dans
    // `surch_index_disk_subfield_values_bytes` ci-dessous (footprint
    // disque/page-cache, PAS de la RAM).
    metrics::gauge!("surch_index_subfield_values_bytes", &label)
        .set(usage.subfield_values_bytes as f64);
    metrics::gauge!("surch_index_disk_subfield_values_bytes", &label)
        .set(disk_subfield_values_bytes as f64);
    // #17c walker complet : live_docs BTreeSet<u32>.
    metrics::gauge!("surch_index_live_docs_bytes", &label).set(usage.live_docs_bytes as f64);
    // Plan segments S5 : per-term CSR/directory metadata RÉSIDENTE
    // (offsets, block_offsets, segment_descriptors, block_directory,
    // block_dir_offsets) — le poste identifié comme le plus fort candidat
    // pour le gap ~295 MiB non gaugé mesuré à 1,36 M (cf. design doc §S5).
    // Scale avec le nombre de termes DISTINCTS, pas le nombre de docs.
    // S5b : les 5 tableaux sont spillés dans le segment pread sous
    // SURCH_POSTINGS_DISK (table TermEntry unifiée 28 o/terme). P3 ne
    // réintroduit pas leurs copies : cette gauge ne garde plus que les
    // digests BLAKE3 et descripteurs de pages ; les octets de service restent
    // dans surch_index_disk_postings_bytes (footprint disque, page cache).
    metrics::gauge!("surch_index_postings_directory_bytes", &label)
        .set(usage.postings_directory_bytes as f64);
    metrics::gauge!("surch_index_total_bytes", &label).set(usage.total_bytes() as f64);
    // #17c walker complet : exposer en clair les gaps qui restent à traquer.
    // `unaccounted` = ce qui est dans le heap mais pas dans nos gauges
    // structurelles. C'est le levier RAM restant.
    let total_accounted = usage
        .total_bytes()
        .saturating_add(state_documents_overhead)
        .saturating_add(state_id_maps);
    metrics::gauge!("surch_index_total_accounted_bytes", &label).set(total_accounted as f64);
    metrics::gauge!("surch_index_doc_count", &label).set(doc_count as f64);
    // Plan segments S2/S3 : nombre de segments (scellés + actif) tenus par
    // l'index. `1` en mono-segment (budget flush non configuré). Post-S3
    // c'est la jauge de santé du merge tiered : elle doit rester bornée
    // (~fan-in × paliers) quand `SURCH_MERGE_FANIN > 0`, au lieu de
    // croître en O(corpus / budget) segments. `0` après suppression de
    // l'index (via `clear_memory_gauges`).
    metrics::gauge!("surch_index_segment_count", &label).set(segment_count as f64);
    // #17b: api-side state overhead — the `documents` BTreeMap node + Arc
    // header + key strings, and the id maps. The _source payload is already
    // reported via `surch_index_stored_fields_bytes`, so this gauge is the
    // ON-TOP-OF cost.
    metrics::gauge!("surch_state_documents_overhead_bytes", &label)
        .set(state_documents_overhead as f64);
    metrics::gauge!("surch_state_id_maps_bytes", &label).set(state_id_maps as f64);
}

/// `?index=<name>` query string for `GET /_surch/stats`.
#[derive(Debug, Deserialize, Default)]
pub struct StatsParams {
    /// Optional filter; when supplied, the response only contains the
    /// named index. An unknown name returns an empty `indices` object.
    pub index: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IndexStats {
    pub doc_count: u64,
    pub memory: MemoryReport,
}

#[derive(Debug, Serialize, Default)]
pub struct MemoryReport {
    pub postings_bytes: u64,
    pub prefix_postings_bytes: u64,
    pub stored_fields_bytes: u64,
    pub field_stats_bytes: u64,
    pub term_stats_bytes: u64,
    // #17: previously-unaccounted RSS components.
    pub fst_bytes: u64,
    pub roaring_bytes: u64,
    pub block_metas_bytes: u64,
    // #17c: capacity slack on Vec<Posting>/Vec<u32> per-term channels.
    pub postings_capacity_slack_bytes: u64,
    // #17c walker complet: PostingsBuilder retenu Lot 1.5.
    pub postings_builder_bytes: u64,
    // #17c walker complet: A10 multi-field side-table (PRENOM.raw etc.).
    pub subfield_values_bytes: u64,
    // Plan segments S5: per-term CSR/directory metadata (offsets,
    // block_offsets, segment_descriptors, block_directory,
    // block_dir_offsets) — see `surch_index::memory::MemoryUsage`'s
    // `postings_directory_bytes` doc comment.
    pub postings_directory_bytes: u64,
    pub total_bytes: u64,
}

impl From<MemoryUsage> for MemoryReport {
    fn from(value: MemoryUsage) -> Self {
        Self {
            postings_bytes: value.postings_bytes,
            prefix_postings_bytes: value.prefix_postings_bytes,
            stored_fields_bytes: value.stored_fields_bytes,
            field_stats_bytes: value.field_stats_bytes,
            term_stats_bytes: value.term_stats_bytes,
            fst_bytes: value.fst_bytes,
            roaring_bytes: value.roaring_bytes,
            block_metas_bytes: value.block_metas_bytes,
            postings_capacity_slack_bytes: value.postings_capacity_slack_bytes,
            postings_builder_bytes: value.postings_builder_bytes,
            subfield_values_bytes: value.subfield_values_bytes,
            postings_directory_bytes: value.postings_directory_bytes,
            total_bytes: value.total_bytes(),
        }
    }
}

/// Axum handler for `GET /_surch/stats[?index=<name>]`.
pub async fn stats_handler(
    State(state): State<AppState>,
    Query(params): Query<StatsParams>,
) -> impl IntoResponse {
    let names: Vec<String> = match params.index {
        Some(name) if state.index_exists(&name) => vec![name],
        Some(_) => Vec::new(),
        None => state.index_names(),
    };

    let mut indices: BTreeMap<String, Value> = BTreeMap::new();
    let mut grand_total: u64 = 0;

    for name in names {
        let usage = state.index_memory_usage(&name).unwrap_or_default();
        let doc_count = state.index_doc_count(&name).unwrap_or(0);
        let report = MemoryReport::from(usage);
        grand_total = grand_total.saturating_add(report.total_bytes);
        indices.insert(
            name,
            json!({
                "doc_count": doc_count,
                "memory": report,
            }),
        );
    }

    let body = json!({
        "indices": indices,
        "total_bytes": grand_total,
    });

    (StatusCode::OK, Json(body))
}
