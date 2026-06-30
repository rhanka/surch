# Lot C — Sortir l'index inversé du heap vers un stockage disk-backed (pread, lecture paresseuse)

> 2026-06-29 — plan d'implémentation ancré dans le code réel (suite à la revue user :
> « tout-en-RAM = faille de design, la latence n'est pas bankable »). Objectif : RAM bornée au
> working set, comme Lucene. **pread (`std::os::unix::fs::FileExt::read_exact_at`), jamais mmap**
> (sûr même sous `forbid(unsafe_code)`, et borne réellement le RSS ; mmap recompterait dans le RSS).
> Précédent prouvé : `_source` déjà externalisé via `source_store` pread (`source.dat` 1187 MiB).
> Codec prêt à l'emploi mais inutilisé : `crates/surch-codec/src/postings_block.rs`
> (`encode/decode_postings_doc_id_freq`, `DocIdDeltaCursor`, `BlockSkipList`).

## Cartographie (ancrée)

- **Postings RAM (753 MiB)** = `FieldPostings` `surch-index/src/postings.rs:251-274` :
  `postings: Vec<Vec<Posting{doc_id:u32,freq:u32}>>` (8 B) + `doc_ids: Vec<Vec<u32>>` (DUPLICAT du
  doc_id, ~250 MiB, cache leapfrog) + `block_metas` (compté term_stats) + `roaring` (df>4096).
  Indexé par FST. Gauge : `memory.rs:178-209`.
- **Point de matérialisation read** : `FieldPostings::lookup_with_block_metas` (`postings.rs:287-295`)
  → `PostingsList` (`postings.rs:526-532`) emprunte **zéro-copie** 4 slices RAM, durée de vie = guard
  RwLock. Consommateurs : `state.rs:1166,1226,1329,2677,2842`, `surch-search/execution.rs:131,217`,
  `surch-search/maxscore.rs:59-66`.
- **Infra disque existante** : `source_store` vit dans **surch-api** `state.rs:36-208` (pas surch-index).
  `surch-index` = `#![forbid(unsafe_code)]` ; `surch-api` = `#![deny]` + `posix_fallocate` confiné
  (`state.rs:109`). Les I/O sont **`std` sûres** : `write_all_at` (pwrite), `read_exact_at` (pread).
  `_source` : write `upsert_document_deferred` (`state.rs:707-740`) → `SourceBlob::OnDisk{offset,len}`
  (12 B RAM) ; read `parse_source_blob` (`state.rs:419`) pread top-K. `compact_after_refresh` désactivé
  → `_source` reste OnDisk en steady-state.
- **subfield_values (427 MiB)** = `DocumentIndex.subfield_values: BTreeMap<String,BTreeMap<u32,String>>`
  (`document_index.rs:67`), écrit `:660`, lu UNIQUEMENT par `AppState::subfield_projection`
  (`state.rs:2151-2175`) qui **clone déjà toute la map** par requête (pas de zéro-copie, pas de
  scoring). Les tokens `.raw` sont AUSSI dans les postings normaux → `term`/`match` indépendant.

## Plan incrémental

- **C0 (1er mover)** — externaliser `subfield_values` vers un segment pread. 2e plus gros poste
  (427 MiB), mécanisme prouvé (`source_store`), zéro scoring/parité de score. Gain ~380-400 MiB.
  Risque : `sort`/`agg .raw` peut balayer N docs → coalescer les pread, mesurer cold/warm.
- **C1a** — écrire `postings.dat` (codec `encode_postings_doc_id_freq`) + voie décode *shadow* +
  `debug_assert` décodé == RAM. 0 gain RAM, valide round-trip + surcoût décode. Risque ~nul.
- **C1b** — basculer read path sur disque, DROP `postings`+`doc_ids`. Garder fst+block_metas+roaring
  en hotcache L0 (~188 MiB). Décodage **bloc-à-bloc** piloté par `BlockMeta`+`DocIdDeltaCursor`
  (ne jamais décoder la liste entière sur conjonction terme rare). Gain ~560-750 MiB. Risque latence
  le plus élevé (warm = +décode ; cold = pread disque). Buffer décodé **possédé** à portée de requête.
- **C2 (opt, après C1b)** — supprimer le canal `doc_ids` dupliqué (~250 MiB).

## Validation (pas de cargo local — CI only)

- Gauges : C0 `subfield_values_bytes` 427→~30 MiB ; C1b `postings_bytes` chute, `process_rss_bytes`
  baisse (la cible). `stats.rs:74,109-156`.
- Bench `surch-eval-perf` (épinglé `sha-<HEAD>`) + criterion `search_hot_path` (surcoût décode warm).
- **Warm vs cold séparés (impératif)** : cold via `posix_fadvise(POSIX_FADV_DONTNEED)` (libc, surch-api,
  pattern `posix_fallocate`) avant 1re requête ; reporter p50/p95 cold ET warm distinctement.
- Parité : `opensearch-oracle` + `snapshot_es` verts (décodage bit-identique → recall/scoring invariant).

## Pièges

1. Latence warm : décoder bloc-à-bloc, jamais la liste entière.
2. Durée de vie : buffer décodé possédé à portée de requête + cache `(field,term)` anti-double-décode.
3. `block_metas`/`roaring` restent en RAM (accès aléatoire WAND/skip).
4. subfield sort/agg = pas top-K → coalescer pread, borner par `size`.
5. Rester pread (pas mmap : recompte RSS + unsafe hors surch-index).
6. Ne pas coupler `postings.dat` (codec surch-codec) avec la sérialisation Lucene `segment_manifest.rs` (snapshot_es) — chemins disjoints.
