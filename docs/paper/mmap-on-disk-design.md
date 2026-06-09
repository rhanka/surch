# Design — mmap on-disk format pour postings et `_source`

Date : 2026-06-09
État : design draft, pas encore d'implémentation. Décision pilier de la campagne mémoire RAM/disque master-plan Phase 4.

## Contexte chiffré (run 27067004820 sha-319f19a, deces 1.36 M, W=2)

```
RSS harness:             7903 MiB
─ stored_fields:         1187 MiB  ← cible mmap #1 (_source)
─ postings:               753 MiB  ← cible mmap #2 (postings FoR)
─ field/term/block_metas: 244 MiB
─ FST + roaring:           70 MiB
─ state_overhead:         263 MiB
─ TOTAL structuré:       2800 MiB
─ Gap heap inexpliqué:   3787 MiB  ← #17c
─ Retained jemalloc:    ~2244 MiB  (cgroup vs /proc)
```

OS 8.6.1 sur même corpus : ~1700 MiB. Cible STRICT 0.5× = ≤ **850 MiB**.

**Bilan** : mmap des deux gros postes (`postings 753 + stored_fields 1187 = 1940 MiB`) ramènerait Surch à ~6000 MiB. Reste à attaquer le **gap 3.8 GiB** (#17c) et la rétention jemalloc pour s'approcher de la cible.

## Architecture proposée

### Levier 1 — mmap `_source` (Arc<[u8]> → MmapBlob)

**État actuel** : `documents: BTreeMap<String, Arc<[u8]>>` (#15 inc3c : deflate-compressed). Tout en RAM.

**Cible** : segment fichier `_source.dat` mappé en lecture seule via `memmap2::Mmap`. Lookup par `doc_id → (offset, length)` via une table side `_source.idx` (binaire, 12 B/doc).

```
_source.dat   :  [compressed_blob_0][compressed_blob_1]...[compressed_blob_N]
_source.idx   :  [offset_0:u64][length_0:u32] × N  (12 B/doc, indexable par doc_id)
```

- **Lecture (hot path)** : `parsed_source(id)` → `doc_id = id_maps.get(id)` → lire `(off, len) = idx[doc_id]` → `&mmap[off..off+len]` → deflate decode → JSON parse. Le `mmap[off..off+len]` est zero-copy, page-cache géré par l'OS.
- **Écriture (bulk path)** : append au fichier `.dat`, update `.idx`. Pas de mmap remap nécessaire si on agrandit avec `MmapOptions::new().len(new_size).map_mut`.
- **Snapshot** : déjà la moitié du chemin — un snapshot Surch peut copier ces 2 fichiers tels quels.

**Gain RAM attendu** : `stored_fields_bytes` 1187 → ~50 MiB (juste les pages chaudes du top-K répété). Sur deces : `RSS 7903 → ~6800 MiB`.
**Gain disque** : `_source.dat` mesurable directement → débloque axe #19.

**Coût latence** : page fault sur premier accès doc, ~10-50 µs/doc. Avec top-K=20 docs/query, p99 peut prendre +0.5 ms si docs froids. Mitigation : warm sur le 1er passage, mlockall optionnel.

### Levier 2 — mmap `postings` (FoR blocks → MmapPostingsList)

**État actuel** : `postings_bytes` 753 MiB répartis sur ~M termes × Vec<BlockMeta> + chunks Vec<u8>.

**Cible** : un segment `postings.dat` par index, BlockMeta side-table en `.idx`.

```
postings.dat  :  [block_0_for_encoded_doc_ids][block_0_for_encoded_freqs]...
                 [block_1...]
postings.idx  :  per-term : (start_offset:u64, n_blocks:u32, last_doc_id:u32)
                 per-block: (max_doc_id:u32, byte_len:u16, max_term_freq:u32)
```

- **Lecture (intersection / leapfrog)** : `PostingsBlockSkipIter` lit la séquence de blocks via `&mmap[off..off+len]`, décode FoR au vol. Skip-list = comparaisons sur le `max_doc_id` (déjà en RAM via `.idx`).
- **Roaring chunks** : restent en RAM pour les termes haut-df (centaines de termes, ~35 MiB) car l'accès en intersection est aléatoire et le bitmap dense est petit.

**Gain RAM attendu** : 753 → ~80 MiB (seulement blocks chauds dans page cache + l'`.idx` ~ entre 5-20 MiB). Sur deces : `RSS 6800 → ~6100 MiB`.

**Coût latence** : la séquence de blocks d'un terme est lue séquentiellement → préfetcher OS efficace. ~0.1-1 µs/block après page-in. Sur bool/full p95 1.3 ms → impact estimé < +5 % si les segments sont sur SSD NVMe.

### Niveau format on-disk

Pour faire les deux : on définit **un layout d'un segment Surch** :

```
<index_dir>/
├── meta.json              (index version, schema, doc_count, mapping)
├── _source.dat            (compressed JSON blobs, append-only)
├── _source.idx            (doc_id → offset/length)
├── postings.dat           (FoR-encoded blocks, term-grouped)
├── postings.idx           (per-term metadata)
├── terms.fst              (FST term dictionary, déjà partiellement on-disk via build snapshot)
├── doc_id_map.dat         (forward: id String → doc_id u32, FST)
└── doc_len_dense.dat      (Lucene SmallFloat quantized si #18 livré, sinon u64)
```

C'est aussi le **format snapshot Surch natif**. Un `_snapshot/<repo>/<snap>` est juste un `cp` (ou un upload S3) de ce dossier.

## Plan d'implémentation

### Phase M1 — mmap `_source` seul (effort M, 3-5 j)
1. Crate `memmap2 = "0.9"` ajoutée à workspace deps.
2. `crates/surch-store/src/source_store.rs` : type `MmapSourceStore { dat: Mmap, idx: Vec<(u64, u32)> }`.
3. Bascule `InMemoryIndex::documents` : `MmapSourceStore` au lieu de `BTreeMap`.
4. Path bulk : append au `_source.dat` + push à `_source.idx`.
5. Path read : `parsed_source(doc_id)` → mmap slice → deflate decode → JSON.
6. **Gate** : oracle b1/b2 0-divergence, NDCG SciFact ≥ 0.65, RSS deces baseline (attendu −800 à −1100 MiB).

### Phase M2 — mmap `postings` (effort L, 1 semaine)
1. `crates/surch-index/src/mmap_postings.rs` : `MmapPostingsList`.
2. Format on-disk doc_id + freq blocks FoR (déjà encodés en RAM via `surch-codec`).
3. Bascule `DocumentIndex::postings` BTreeMap<term, PostingsList> → `MmapPostingsIndex`.
4. **Gate** : oracle b1/b2 0-divergence, bool/full p95 régression < 10 %.

### Phase M3 — mesure axe disque #19 (effort S, 1 j)
Une fois les segments sur disque : `du -sh <index_dir>` côté Surch, comparé à `_cat/indices?v` côté ES. Snapshot K8s job qui dump les deux dans l'artefact. Master-plan ligne 132 enfin renseignée.

### Phase M4 — snapshot natif Surch (effort M, post-M1/M2)
- `_snapshot/<repo>/_create` = `cp` des fichiers segments + meta.json.
- `_snapshot/<repo>/_restore` = `cp` inverse + remap.
- Compatible ES wire-contract via `snapshot_es::*` (déjà livré).

## Risques + mitigations

| Risque | Mitigation |
|---|---|
| **Parité oracle b1/b2** (sacré) | Format on-disk = même bytes que l'encodage RAM actuel. Test bit-identité unitaire par term. |
| **Régression latence top-K** | Page-cache OS chaud avant CI bench. `mlockall` optionnel pour les segments chauds. |
| **Crash recovery** | Append-only + fsync sur close. Un fichier `.partial` détecté au démarrage → tronque à la dernière ligne valide de `.idx`. |
| **Indexation -X %** | Append au lieu d'in-memory insert → I/O système. Mitigation : write-back buffered, fsync différé. Acceptable si < 15 %. |
| **NDCG drift** | Si SmallFloat #18 livré en parallèle, deux changements en même temps. Phaser : M1 puis #18 puis M2. |

## Décision

**M1 d'abord** (mmap `_source` seul), puis #18 SmallFloat (parité NDCG TREC-COVID), puis M2 (mmap postings), puis M3 (mesure disque), puis M4 (snapshot natif).

Cela séquence proprement la campagne Phase 4 du master plan :
- RAM : M1 → −1100 MiB, M2 → −700 MiB. Total ~−1800 MiB. Reste #17c gap heap.
- Disque : axe ouvert par M3 (un `du` après M1+M2).
- Qualité : NDCG TREC-COVID par #18 indépendamment.
- Latence : préservée par construction (page-cache hot).
- Indexation : à mesurer en gate après M1 et M2.

Pré-requis bloquant : **valider le verdict #15 inc3c** (`bhfkbpces` en cours) — si #15 réussit, M1 part dessus ; si #15 échoue, M1 le remplace directement.

## Coûts en complexité

- +1 dep : `memmap2 = "0.9"` (1500 LOC, no_std, audité).
- +400 LOC environ : `MmapSourceStore`, `MmapPostingsList`, snapshot natif.
- Recovery logic + tests de robustesse (crash mid-write) : +200 LOC test.

Vs la campagne #15 compression : meilleur ROI structurel, parité-safe par construction, débloque axe disque ET snapshot natif en bonus.
