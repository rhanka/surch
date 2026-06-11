# Persistance disque Surch — architecture segments + manifest atomique

Date : 2026-06-11
État : design doc, pas d'implémentation. Pilier débloquant axes RAM +
disque + comparaison indexation honnête vs ES.

## 0. Reality check

Surch est RAM-only (`SourceBlob::Compressed(Arc<[u8]>)`, `BTreeMap<String,
SourceBlob>` state.rs L240). Conséquences :

- **1.16× ES indexation n'a pas de sens** : ES écrit ~1.2 GiB durables ;
  Surch n'écrit rien. Pommes vs oranges.
- **Axe disque #19 ouvert** : cible ≤ 0.5× OS (≤ 600 MiB) impossible à
  mesurer.
- **Axe RAM bloqué** : RSS 6 621 MiB = 3.78× pire. Le gap 3.8 GiB ne se
  ferme PAS par compression à la marge — il faut sortir `_source` et
  `postings` vers le disque (page-cache OS).
- **mmap M1 (3c864af) reverté à tort 2026-06-09** : timeout 58 min
  attribué au mmap était un bug bulk-search stall pré-existant (Artillery
  hang) ; `posix_fallocate` aurait par ailleurs évité l'expansion fichier
  ext4 répétée. À cherry-pick.

Le design : un dérivé de **Lucene segments** (immutables append-only,
refresh = nouveau segment) piloté par un **manifest atomique style
Iceberg** (single source of truth + rename POSIX). Débloque mémoire +
disque + parité indexation, sans casser parité oracle ni latence.

## 1. Layout segments

```
<surch_data_dir>/<index_name>/
├── manifest.json                  # pointeur version actif
├── manifest.json.tmp.<pid>.<n>    # transitoire write+rename
├── segments/
│   ├── seg-00000001/
│   │   ├── meta.json              # schema, doc_count, doc_id range, codec
│   │   ├── source.dat             # _source deflate, append-only
│   │   ├── source.idx             # doc_id local → (offset:u64, length:u32)
│   │   ├── postings.dat           # FoR-encoded blocks (doc_id + freqs)
│   │   ├── postings.idx           # per-term (start_off, n_blocks, last, max_tf)
│   │   ├── terms.fst              # FST term dict
│   │   ├── doc_id_map.fst         # forward String → doc_id local
│   │   └── doc_len.dat            # SmallFloat quantized 1 oct/doc
│   └── seg-00000002/ …
└── _tombstones/seg-NNN.tomb       # Roaring bitmap doc_ids supprimés
```

Génération s'incrémente à chaque refresh. `seg-NNNNNNNN` triable, croît
monotoniquement, jamais réutilisé.

**Header 64 octets par `.dat`/`.idx`** :

```
magic:        b"SRCH"        4 oct
format_vers:  u32 LE         4 oct
codec:        u16 LE         2 oct (0=raw, 1=deflate, 2=zstd, 3=lz4)
flags:        u16 LE         2 oct (bit0=fsynced, bit1=sealed)
doc_count:    u32 LE         4 oct
created_unix: u64 LE         8 oct
crc32c:       u32 LE         4 oct (payload qui suit)
padding:      36 oct
```

Magic = détection corruption immédiate. `sealed=1` = segment immutable.
CRC32C du payload (pas du header) — pas de checksum par bloc (overhead
inacceptable hot path).

**Durabilité** : append-only ; segment scellé ne se réécrit jamais.
Ordering fsync au `_refresh` :
1. `source.dat` + `source.idx`
2. `postings.dat` + `postings.idx`
3. `terms.fst` + `doc_id_map.fst` + `doc_len.dat`
4. `meta.json` + fsync dir segment
5. Puis seulement : write `manifest.json.tmp.<…>` + fsync + rename
   atomique + fsync index dir.

Crash entre 1-4 → segment orphelin (non référencé) purgé au boot. Crash
en 5 → manifest pointé sur version précédente, état stable.

## 2. Manifest pattern (Iceberg-style)

### 2.1 Schéma

```json
{
  "format_version": 1,
  "surch_version": "0.3.x",
  "index": "deces",
  "generation": 17,
  "active_segments": [
    { "id": "seg-00000015", "doc_count": 1200000,
      "doc_id_min": 0, "doc_id_max": 1199999,
      "size_bytes": 412034112, "tombstone": null },
    { "id": "seg-00000017", "doc_count": 160000,
      "doc_id_min": 1200000, "doc_id_max": 1359999,
      "size_bytes": 54231040, "tombstone": "seg-00000017.tomb" }
  ],
  "schema": { "properties": { … } },
  "settings": { … },
  "aliases": { … },
  "snapshot_tags": ["snap-2026-06-11T08:00Z"]
}
```

Le manifest liste **ce qui est vivant**. Tout segment de `segments/` non
listé est invisible. Permet compaction propre + snapshot tags par
hardlink.

### 2.2 Atomicité — rename POSIX

```
write("manifest.json.tmp.<pid>.<nonce>", new_bytes);
fsync(".tmp");
rename(".tmp", "manifest.json");   // atomique POSIX
fsync(".");
```

`rename(2)` atomique sur même filesystem : lecteur voit l'ancien ou le
nouveau, jamais d'intermédiaire. Pattern Iceberg metadata pointer +
Lucene `segments_N`. **Aucun verrou applicatif pour les lecteurs.**

`AppState` charge le manifest une fois + swap via `ArcSwap` au refresh.
Lecteurs en cours terminent sur l'ancien snapshot.

## 3. Bulk path (write)

1. À l'arrivée du `_bulk`, ouvrir le segment courant (ou créer
   `seg-NNN+1`). **`posix_fallocate(source.dat, 64 MiB)`** à la création
   — pré-alloue extent contigüe ext4, évite l'expansion répétée qui
   causait timeout 58 min sur M1.1.
2. Le batch construit postings en RAM (PostingsBuilder existant) et
   écrit `_source` deflate au fur et à mesure dans `source.dat` via
   `pwrite`. `source.idx` agrégé en RAM dans `Vec<(u64, u32)>`, écrit
   en bloc à la fin du bulk.
3. **Pas de fsync ni rename pendant le bulk** — durabilité différée au
   `_refresh`. Le hot path bulk n'attend AUCUN fsync.

**`_refresh` = scellage** :
1. Compléter `postings.dat`/`.idx` + FSTs + `doc_len.dat`.
2. Truncate `source.dat` à la taille réelle.
3. `flags.sealed=1`, recompute CRC32C, fsync tous fichiers + dir
   segment.
4. Write `manifest.json.tmp.<…>` + rename atomique.
5. Swap `Arc<Manifest>` pour rendre segment visible.

Coût fsync 50-200 MiB SSD NVMe : 50-200 ms. Acceptable au refresh
(périodique, default 1s côté ES).

**Update/Delete** : marque tombstone dans `seg-NNN.tomb` (Roaring on-disk).
À la prochaine compaction, expulsés. Le `_search` filtre les tombstoned
avant scoring.

## 4. Read path

`parsed_source(id)` top-K hydration :

1. `id: String` → `(segment_id, doc_id_local)` via `id_to_segment_map` en
   RAM (HashMap, ~12 oct/doc → 16 MiB sur 1.36 M).
2. `Arc<MmapSegment>` chargé au démarrage (memmap2 read-only par fichier).
3. `source.idx[doc_id_local] = (off, len)` → `&mmap[off..off+len]`
   zero-copy.
4. Décode deflate via thread-local `Decompress` existant.
5. `serde_json::from_slice(&decoded)`.

Page fault unique sur premier accès doc froid (~10-50 µs SSD NVMe). Pages
chaudes top-K répété → page-cache OS, RSS Surch reste bas.

**Intersection postings multi-segments** :

```rust
fn run_leapfrog_global(query, manifest) -> Vec<Hit> {
    let mut hits = Vec::new();
    for segment in &manifest.active_segments {
        let seg_hits = run_leapfrog_in_segment(query, segment);
        hits.extend(seg_hits.into_iter().map(|h|
            translate(h, segment.doc_id_min)));
    }
    score_and_topk(hits)
}
```

Surcoût N appels. Sur deces post-MVP : 1 main + 1 delta = 2. Quand N>8,
compaction ramène à 1-2.

**FoR postings on-disk** : encode au scellage, décode au vol via mmap
(0.1 µs/block 128 doc_ids). Amortit page fault.

## 5. Compaction

Trigger : `n_active_segments > 8` OU `tombstoned/total > 0.15`.

**MVP : déclenchement manuel** via `POST /_optimize` ou
`POST /_forcemerge?max_num_segments=1` (compat ES). Pas de thread
background en P1-P3.

Algo : log-merge-policy à la Lucene (plus petits d'abord) ; streaming
re-attribuer doc_ids contigus (compactage trous tombstone) ; concat
`source.dat` (skip tombstoned) ; merge postings par term k-way + FoR
encode ; reconstruire FSTs ; fsync + scellage + swap manifest. Anciens
segments référencés par lecteurs concurrents jusqu'à fin requête, puis
GC après 1 h.

À ne PAS lancer pendant un benchmark indexation. Pour mesure axe disque
#19 : forcer `_forcemerge` avant `du -sh`.

## 6. Snapshot natif Surch

```bash
mkdir -p _snapshots/<tag>/
cp -al segments/ _snapshots/<tag>/segments/     # hardlinks (zéro espace)
cp manifest.json _snapshots/<tag>/manifest.json
```

Restore = swap manifest atomique. ~1 s sur 1.36 M docs. Compatible ES
wire-format via `snapshot_es::*` existant (sait emballer un répertoire
en tarball ES-compatible — livré `service.rs`).

Bonus : sauvegarde S3 incrémentale, segments scellés immutables → cache
ETag parfait.

## 7. Schema evolution

Hors scope MVP. `meta.json` par segment permet de lire des segments de
schémas additivement compatibles. `format_version` immuable ; bump
casse rétro-compat → opérateur force `_forcemerge` puis upgrade.

## 8. Crash recovery

Au démarrage `surch-api` :

1. **Purge `.tmp.*`** : tous orphelins, jamais référencés.
2. **Validation manifest** : parse + CRC32C. Si KO → fallback automatique
   `manifest.json.prev` (toujours maintenu, overwrite à la 2e génération
   suivante). Si prev KO aussi → refus boot.
3. **Validation segments actifs** : magic + format_version headers
   match. Sinon refus boot.
4. **GC orphelins** : segments présents non listés dans manifest →
   candidats suppression après 1 h.
5. **Validation tombstones** : `.tomb` orphelins supprimés.

## 9. Comparaison Lucene / Iceberg / proposition

| Mécanisme | Lucene | Iceberg | **Surch** | Décision |
|---|---|---|---|---|
| Segments immutables | `.cfs`/`.si` | Parquet | `segments/seg-NNN/` | **ADOPTÉ** fondation |
| Manifest pointer | `segments_N` | `vN.metadata.json` + version-hint | `manifest.json` + rename POSIX | **ADOPTÉ** style Iceberg (plus simple que IndexCommit Lucene) |
| Atomicité écriture | IndexWriter lock + rename | Optimistic concurrency + atomic swap | rename POSIX + Arc<Manifest> | **ADOPTÉ** Iceberg (no write-lock contention) |
| Append-only WAL | OUI (translog) | NON | NON pour MVP | **SKIPPED** — `_refresh` synchrone suffit (matchID read-mostly post-load) |
| Doc deletes | `.liv` bitset | Position/equality deletes | `.tomb` Roaring on-disk | **ADOPTÉ** style Lucene |
| Compaction | LogMergePolicy auto | RewriteFiles manuel | `_forcemerge` API + auto P4 | **HYBRIDE** |
| Snapshot | IndexCommit refcount | Snapshot tags vers manifest | hardlink + tag | **ADOPTÉ** Iceberg |
| Schema evolution | FieldInfos par segment | schema_id par snapshot | hors scope MVP | **SKIPPED** |
| Time travel | NON | OUI (snapshot_id) | NON | **SKIPPED** not needed matchID |
| Catalog distribué | NON | OUI (Hive/Glue) | NON | **SKIPPED** single-node by design |
| Mmap zero-copy | MMapDirectory | NON (Parquet decode) | memmap2 par fichier | **ADOPTÉ** Lucene — levier RAM principal |
| FST term dict | BlockTreeTerms | N/A | `terms.fst` | **ADOPTÉ** (déjà en place) |
| Posting lists FoR | PFOR/FoR delta | N/A | `postings.dat` blocks | **ADOPTÉ** block FoR |

**Synthèse** : layout segments + mmap de Lucene (zero-copy hot path),
manifest + tags de Iceberg (atomicité rename + hardlinks), skip tout ce
qui est hors-scope single-node.

## 10. Plan d'implémentation phasé

### P1 — Restaurer mmap M1 + `posix_fallocate` (Effort S, 3 j)

Cherry-pick `3c864af` (mmap `_source` via pread). Ajouter
`posix_fallocate(source.dat, 64 MiB)` à création + truncate au refresh.

Crates : `memmap2 = "0.9"`, `rustix = "0.38"` (fallocate). Périmètre :
`_source` uniquement, postings/FST restent RAM, 1 seul segment, pas
encore de manifest. Segment dans `<surch_data_dir>/<index>/source-current/`.

Gains :
- **RAM : −1100 MiB** (stored_fields 1187 → ~50 MiB pages chaudes).
- **Disque : +200-400 MiB visibles** (axe #19 mesurable).
- **Indexation : neutre** si `posix_fallocate` (sinon −50 % comme M1.1).
- **Latence : +0.2-0.5 ms p99** (page fault froid), p50/p95 inchangés.

### P2 — Manifest atomique + multi-segments (Effort M, 5-7 j)

Structure `Manifest` + rename POSIX. Bulk = nouveau segment, `_refresh`
= scellage + swap. `parsed_source` route par `id_to_segment_map`.
Crash recovery §8 minimal. Compaction = `_forcemerge` API only.

Gains :
- **Disque : axe #19 verrouillé**, comparable à `_cat/indices?v` ES,
  cible ≤ 600 MiB.
- **RAM** : −50 MiB side-tables segmentées.
- **Indexation : +5 %** (rename différé fsync, acquis en P1).
- **Snapshot** : feature unlock (hardlinks `_snapshots/<tag>/`).

### P3 — FoR delta-encoding postings on-disk (Effort M, 5 j)

Déplacer 753 MiB RAM → `postings.dat` on-disk FoR delta + per-block
max_doc_id skip-list. Side-table `.idx` reste RAM. Roaring chunks haut-df
restent RAM (~35 MiB).

Gains :
- **RAM : −700 MiB** (postings 753 → ~80 MiB chauds + 15 MiB idx).
- **Disque : −20 % vs raw** (FoR delta ~3.5 bits/doc_id).
- **Indexation : neutre** (build RAM puis flush au refresh).
- **Latence bool/full : +0-5 %** (FoR decode rapide).

### P4 — Compaction background (Effort L, 8-10 j)

Thread tokio low-priority scrutant `n_active_segments` tous 30 s.
LogByteSizeMergePolicy. k-way merge écrit nouveau segment + swap. GC
orphelins après 1 h.

Gains :
- **Indexation steady-state : −10 %** par compaction (OPS améliore
  niveau acceptable), évite explosion segments long terme.
- **Latence** : sans P4, dégradation ~+0.1 ms/segment leapfrog ; P4
  ramène à 1-2 segments stables.
- **RAM** : −id_to_segment_map duplication.

### Récap effort/gain par axe

| Phase | Effort | RAM Δ | Disque | Indexation Δ | Latence Δ | Risque |
|---|---|---|---|---|---|---|
| P1 mmap+fallocate | S 3j | **−1100 MiB** | +400 MiB | 0 | +0.2 ms p99 | bas |
| P2 manifest multi-seg | M 5-7j | −50 MiB | atomic mesurable | +5 % | 0 | moyen |
| P3 FoR postings | M 5j | **−700 MiB** | −20 % | 0 | +0-5 % bool/full | moyen |
| P4 compaction bg | L 8-10j | −id_map dup | stable | −10 % steady | stabilisée | élevé |
| **P1+P2+P3** | 13 j | **−1850 MiB** | <600 MiB cible | +5 % | +0.5 ms p99 max | mitigeable |

**Cumul vs cibles master-plan** :
- RAM 6621 → **~4770 MiB** : encore >> cible 875 MiB. **Le gap heap
  #17c (3.8 GiB inexpliqués) reste le vrai blocker** — P1+P3 ne le
  ferment pas, mais alignent 2 GiB structurés et débloquent le diagnostic
  du reste.
- Disque : mesurable, < 600 MiB attendu sur deces (FoR + deflate).
- Indexation : neutre à +5 %, **permet comparaison honnête avec ES** sur
  24 000 docs/s cible (le 1.16× actuel devient vrai 1.16× charge
  équivalente).
- Latence : préservée par construction.

## 11. Gates obligatoires par phase

Chaque phase doit passer sur cluster CI K8s avant merge :

1. **Parité oracle b1/b2 deces : 0 divergence SACRÉE.** Format on-disk =
   mêmes bytes que RAM actuel. Test bit-identité unitaire par term.
2. **Indexation ≥ 14 000 docs/s** (baseline non-régression). P3 plus à
   risque (FoR encode scellage).
3. **Latence p95 STRICT acquis** : bool ≤ 1.75 ms, full ≤ 1.6 ms, match
   ≤ 2.1 ms (gates 2× ES STRICT actuels). P3 surveille.
4. **NDCG SciFact ≥ 0.65 + TREC-COVID ≥ 0.465**. Aucune phase ne
   touche scoring → passifs par construction.
5. **RSS TREC-COVID ≤ 1000 MiB** (marge sur 964 acquis).
6. **Crash recovery test** : `kubectl delete pod surch-api --grace-period=0`
   pendant Artillery → restart → 0 divergence b1/b2.

## 12. Risques + mitigations

| # | Risque | Prob | Impact | Mitigation |
|---|---|---|---|---|
| 1 | Page fault froid coûte cher SSD cloud lent (Scaleway) → p99 régresse | M | É | (a) `mlockall` opt sur `terms.fst` + `postings.idx` ; (b) warmup `madvise(MADV_WILLNEED)` au boot ; (c) gate p99 5-rep W2. |
| 2 | fsync `_refresh` bloque trop long (200 ms / 200 MiB SSD lent) | M | M | (a) `fdatasync` skip metadata ; (b) `_refresh` async — 200 OK après rename, fsync background. Risque crash entre rename et fsync → segment scellé pas durable ; acceptable, prochain refresh re-fsync. |
| 3 | Crash mid-bulk laisse `.tmp.*` consomme espace | F | F | Recovery §8 purge tous `.tmp.*`. Sentry/monitoring sur `du _tombstones` et `find -name '*.tmp.*'`. |
| 4 | Manifest tronqué (write partiel crash filesystem) | TF | C | (a) CRC32C manifest ; (b) garder `manifest.json.prev` = ancienne version (overwrite 2e gen suivante). Boot fallback automatique `prev`. |
| 5 | Multi-segments explose n→inf sans compaction → latence search dégrade linéairement | É si P4 différé | M | (a) `_forcemerge` API dès P2 (opérateur ou cron) ; (b) métrique Prometheus `surch_active_segments_count` alerte > 16 ; (c) P4 prioritisé si prod usage. |
| 6 | Parité oracle régresse car FoR encode/decode pas bit-identique RAM actuel | M | C SACRÉ | Test bit-identité unitaire AVANT P3 : encode FoR + decode + comparer doc_ids set vs RAM. 100 termes + 100 paires. CI gate bloquant. |
| 7 | SSD cloud lent (Scaleway ~100 MB/s) rend P1 contre-productif sur très-gros corpus | M | M | Gate documenté : `dd if=/dev/zero` au boot, warning si < 200 MB/s. Doc deployment : NVMe local required for production. |
| 8 | Snapshot hardlink cross-filesystem échoue | F | F | Détection `stat` même device au démarrage. Fallback `cp` (copie réelle) avec warning log. |
| 9 | Race fsync `manifest.json` vs segment files (reader voit manifest pointant sur segment pas fsynced) | F | M | Ordering strict §1.3 : fsync segments AVANT fsync manifest. Respecté → race impossible (POSIX visibility ordering). |
| 10 | Gap heap 3.8 GiB (#17c) inchangé par cette campagne → on n'atteint pas 0.5× OS RAM | É | Stratégique | Documenter honnêtement : P1+P3 ferment axe disque + 50 % gap RAM structuré. #17c instrumenter SÉPARÉMENT (walker complet, heaptrack). Pas une régression — chantier complémentaire. |

## 13. Décision recommandée

**P1 immédiat** : quick-win 3 j, fonde l'architecture, débloque mesure
axe disque honnête. RAM −1100 MiB, parité-safe par construction.

**Puis P2+P3 en série** (~10 j total) — ensemble ils ferment l'axe
disque ET débloquent la **comparaison indexation équivalente avec ES**
(les deux écrivent disque, on peut enfin parler de 2× docs/s STRICT).

**P4 différé** post-validation production. Pas requis pour atteindre
cibles master-plan.

## 14. À valider sur cluster (post-P1)

1. **`dd` SSD throughput** runner Scaleway K8s : confirmer ≥ 200 MB/s
   sustained, sinon documenter impact P1 indexation.
2. **Page-cache OS W=2** : Artillery 5-rep, mesurer `/proc/meminfo
   Cached|Buffers` au pic. Vérifier mmap segment reste cached sous
   pression mémoire.
3. **Page fault rate** : `perf stat -e page-faults` Artillery cache-OFF.
   Si > 10 K page faults/s sustained, P1 régresse latence — gate à
   serrer.
4. **fsync latency** : tracer `_refresh` p50/p99 sous bulk continu. Si
   > 500 ms p99, basculer `fdatasync` (option §risque 2).
5. **Crash recovery functional** : `kubectl delete pod --force` pendant
   bulk → restart → oracle b1/b2 0-divergence.
