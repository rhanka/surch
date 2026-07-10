# Contre-expertise tranche 2b — compression du `_source` (Opus 4.8 max, 2026-07-10)

Double consensus sur le design 2b de `brainstorm-4-fronts-2026-07-09.md` (zstd `_source` par blocs
64-128 KiB). **Verdict : GO-AMENDÉ — blocs 64-128 KiB REJETÉS, compression PAR-DOC d'abord.**

## A. Ce que le code dit vraiment (corrige le brainstorm)

1. **Le `_source` est GLOBAL, pas per-segment** : `source_store: SourceStore` champ de
   `InMemoryIndex` (`state.rs:687`), un seul `source.dat` append-only sous TMPDIR, indexé par
   doc_id interne monotone jamais réutilisé. Les merges de segments **ne touchent jamais**
   `source.dat` → compression indépendante du merge, **zéro write-amp de merge** (Q3 tranchée).
2. **Aujourd'hui : non compressé, 1 pread/doc au fetch** (~500 o = 1-2 pages) — granularité de
   référence à préserver (`state.rs:1071-1072, 2418, 420-424`).
3. **Un socle de compression existe déjà** : `SourceBlob::Compressed` + `compact_after_refresh`
   (flate2/DEFLATE, par-doc) mais `#[allow(dead_code)]`, plus appelé depuis mmap M1. Le pattern
   thread-local `Compress`/`Decompress` (`state.rs:314-411`) est le squelette à réutiliser.
4. **zstd n'est pas une dépendance** (seul `flate2`) — crate à ajouter.
5. **Side-table `Vec<Option<SourceBlob>>` ≈ 24 o/doc × 28,9M = ~660-694 MiB anon** (~25 % du budget
   anon @4g) — chiffre load-bearing.
6. **Oracle : risque nul par construction** — le store round-trippe déjà par serde
   (`to_vec`/`from_slice`), zstd lossless → 0 divergence garantie.
7. **Orphelins sur update** : `source.dat` jamais compacté ; mesure disque 2a à faire en insert pur.

## B. Le calcul qui disqualifie les gros blocs (Q1)

Sous requêtes ALÉATOIRES, un top-10 fetche 10 docs NON adjacents (position dans `source.dat` =
ordre d'insertion, décorrélée du nom) → 10 blocs distincts. zstd n'a pas d'accès aléatoire
intra-bloc : extraire 1 doc décompresse TOUT le bloc.

| Design | pread/doc | pages/doc | Décompress/doc | p50 CHAUD (hit) | p95 MISS |
|---|---|---|---|---|---|
| Aujourd'hui (500 o brut) | ~500 o | 1-2 | 0 | ~5 µs | ~70-95 µs |
| Bloc 128 KiB (~37 KiB comp) | ~37 KiB | ~10 | 65-85 µs | **~75 µs** | ~150-190 µs |
| Bloc 16 KiB (~5 KiB comp) | ~5 KiB | ~2 | ~8-11 µs | ~12 µs | ~80-105 µs |
| **Par-doc (~180-250 o comp)** | ~200 o | **1** | ~1-2 µs | **~7 µs** | ~72-97 µs |

Bloc 128 KiB : top-10 tout-chaud = 650-850 µs CPU sérialisé vs ~50 µs aujourd'hui → **régression
p50 chaud ×13-17** et p95 +45-90 % — anti-corrélé avec le front 1 (rendre la latence bankable).

## C. Réponses clés restantes

- **Q2 (bulk)** : par-doc = compression inline avant `append`, aucun buffering, aucun pic anon,
  toujours 1 pwrite/doc (plus petit). zstd niv. 3 sur ~500 o ≈ 2-3 µs/doc ≈ 5,5 % d'un cœur à
  21,8k doc/s → impact indexation attendu <5 % (gate). Le ledger « jamais 1 pwrite/valeur »
  concernait la granularité sub-doc, pas le par-doc.
- **Q4 (index frugal)** : par-doc = ZÉRO nouvelle structure (`OnDisk{offset,length}`, length devient
  compressée). Levier orthogonal recommandé : **packing 40 bits offset + 24 bits length = 8 o/doc
  dans un `Vec<u64>`** → ~231 MiB au lieu de ~660 (−430 MiB anon rendus au page cache) sans toucher
  la granularité. Blocs 16 KiB (annuaire 16 o/bloc, −650 MiB) = dernier recours seulement.
- **Q5 (réversibilité)** : flag `SURCH_SOURCE_COMPRESS=off` bit-identique OK, MAIS ajouter dès 2b un
  **tag codec `u8` sur `SourceBlob::OnDisk`** (raw/zstd/zstd+dict — gratuit dans le padding de
  l'enum) : blobs auto-descriptifs, store à codec mixte possible pendant un flip de flag. Header de
  version complet = sujet P2 (persistance).
- **Q6 (chemin XS à 80 %)** : **OUI — par-doc zstd sans dict** (~15 lignes autour de
  `upsert_document_deferred`/`parse_source_blob`) : ratio JSON deces ~1,7-2× → `_source` 7,3-8,5 GiB
  → **total ~11,3-12,5 GiB ≤ parité ES (12,6)**. Escalade optionnelle : **dictionnaire zstd
  entraîné** sur ~10k premiers docs (bootstrap, ~5 MiB anon one-time) → ratio ~2,5-3× → total
  **~8,8-9,8 GiB, bat ES confortablement**, toujours 1 page/doc. « Compresser les docs froids
  seulement » : non applicable (Zipf-random → position ⊥ fréquence).

## D. Amendements + gates (le contrat d'implémentation 2b)

1. **(bloquant)** Blocs 64-128 KiB rejetés ; plafond 16 KiB SI blocs un jour.
2. **(structurant)** **Par-doc d'abord, pas de blocs.** Mesurer le ratio réel via 2a ; dictionnaire
   seulement si `_source` compressé > ~6 GiB ; packing 8 o/doc si la marge anon doit être récupérée ;
   blocs 16 KiB en tout dernier recours prouvé.
3. Tag codec `u8` auto-descriptif dès 2b.

**Gates** : (1) le gate liant = p95 ET p50 sous sonde ALÉATOIRE + COLD du front 1 (jamais la sonde
fixe), non-régression <10 % sur les deux ; (2) indexation ≥ 21,8k doc/s @28M (inline ≤5 %) ;
(3) `memory.stat` anon non augmenté ; (4) oracle-local 0 divergence ; (5) disque ≤13-14 GiB @28M en
insert pur — 2a doit d'abord ventiler ≥90 % des 18,6 GiB et confirmer la part `_source` (~14,5 GiB
estimés) avant de coder.

**Ordre** : 2a (ventilation) → par-doc sans dict + tag codec + gate front-1 → (si ratio insuffisant)
dict entraîné → (si marge anon) packing 8 o/doc → blocs 16 KiB seulement si prouvé nécessaire.
