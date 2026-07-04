# Design C1b — postings disk-backed (FoR/pread bloc-adressé)

> 2026-07-02 — triple consensus **Codex GPT-5.5 xhigh + Opus 4.8 max + Fable 5**, CONVERGENCE FORTE.
> Fait suite à la campagne in-RAM close (RAM 1,09× ES) : sortir les postings (526 MiB) sur disque
> pour viser ≤ES/2 + tenir 28 M. Le SoA `doc_ids_flat`+`freqs_flat` est le format d'entrée.

## Décisions (convergentes)

1. **Read path = décodeur BLOC-ADRESSÉ streaming** (pas terme-entier, qui recrée la régression opt #7/#11
   sur termes communs). Le hot path est déjà par blocs de 128 (BMW skip via méta résidentes).
   - **Conjonction/recall (bloc-adressé, v1 obligatoire)** : roaring reste en RAM (intersection
     word-parallel, ZÉRO décode pour le recall) ; `freq_at` fait des lookups bloc-adressés seulement
     sur les docs intersectés (petit N). Leapfrog : `advance_to` via directory RAM → décode SEULEMENT
     le bloc landé. Préserve #11/#20.
   - **Scoring OR-match (`maxscore_match`) v1** : matérialisation terme-entier dans l'arène (simple,
     parité triviale, `MaxScoreToken` reçoit toujours `&[u32]` pointant l'arène). v2 block-lazy si le gate serre.
2. **Codec bloc auto-contenu** : ajouter `encode_postings_blocked` (delta doc_id remis à 0 par bloc,
   `[doc_ids du bloc][freqs du bloc]` interleavé) — NE PAS toucher `encode_postings_doc_id_freq` (tests).
   `inspect_postings_blocks` calcule déjà les byte-ranges par bloc. Décode random-access du bloc j.
3. **pread, PAS mmap** : les deux sont page-cache-backed → évictables sous pression cgroup à l'identique ;
   mmap n'économise que ~0,5 µs/accès (memcpy) mais coûte unsafe (hors surch-index), SIGBUS sur
   truncate concurrent, remap au refresh. `FileExt::read_exact_at` + `Arc<File>` par génération (pattern
   `source_store`), reste 100% safe dans surch-index.
4. **Hotcache RAM** (remplace `doc_ids_flat`/`freqs_flat`) : fst + roaring + CSR offsets + **NOUVEAU
   `byte_offsets` par terme (~22 MiB) + `block_max_doc_ids` par bloc (~21 MiB)** — le levier B est
   partiellement ré-introduit car min/max ne sont plus dérivables de `doc_ids` (parti sur disque).
   `max` seul suffit (`block_first` conservatif = `max[j-1]+1`, parité-safe : skip = pure optim).
5. **Arène par requête** (`SearchScoringContext`, surch-api) : blocs décodés possédés à portée de requête,
   `HashMap<(term,block)>` anti-double-décode. La durée de vie `'a` des slices passe « du guard RwLock »
   à « de l'arène » → 90% des consommateurs gardent leur signature `&'a [u32]`.
6. **Écriture batchée par-champ au refresh SEULEMENT** : accumuler le payload FoR du champ dans 1 `Vec<u8>`,
   flush par chunks 8-16 MiB (~12-25 syscalls total, PAS 5,46 M — évite la falaise C0 −43%). Writer safe
   dans surch-index (écriture séquentielle gros chunks, pas de posix_fallocate). Swap par génération.
   Impact indexation < 5% ; le pic transitoire du build BAISSE (FoR ~½ remplace les 2 buffers).

## Latence (chiffrée, à valider au bench warm/cold)
- **Warm bool/full ~1,9-2,3 ms** (< ES 3,0-3,5 SÛR ; ≤ES/2 non garanti au 1er jet → LRU blocs 64 MiB en rattrapage).
- **Large `match` mono-terme commun** : +0,5-1 ms (le cas sensible au décode). À surveiller au gate.
- **Cold (fadvise DONTNEED)** : +0,5-8 ms NVMe local ; 20-60 ms si PVC réseau. Warmup `fadvise WILLNEED`
  au refresh ramène cold≈warm.
- **La latence ≤ES/2 n'est PAS bankable une fois disk-backed** (c'était une propriété du tout-en-RAM).
  Objectif défendable : warm < ES, régression ≤ ~+1 ms vs in-RAM ; cold < 2× warm.

## Atteignabilité (honnête)
- **1,36 M** : allocated 1119 − 526 + 43 (directory) ≈ **636 MiB < 843 ✓**. MAIS anon = live + frag ;
  le frag ~717 MiB est porté par les petites allocs (subfields dict, id_maps), pas les gros extents postings.
  **Anon post-C1b ≈ 850-1050 MiB → ≤ES/2 (843) sur le fil, côté défavorable.** Le paquet ≤ES/2 =
  **C1b + C0-retry subfields (−118 live) + tuning frag jemalloc**, PAS C1b seul.
- **28 M** : C1b NÉCESSAIRE (postings → page cache évictable, sinon ~10,8 GiB anon = OOM), mais NON
  suffisant : id_maps ~2,8 GiB (linéaire) = **C2 obligatoire**. Le directory doit être SÉCABLE (garder en
  RAM une couche skip grossière, externaliser min/max/offset en skip-file) — à concevoir dès maintenant.

## CATCH tmpfs (Fable) — VÉRIFIÉ BÉNIN
`PostingsSegment` (postings.rs:87) et `source.dat` (state.rs:82) écrivent dans `std::env::temp_dir()`
(`/tmp`). Risque : si `/tmp` était tmpfs, les pages seraient anon NON-évictables → gain RAM fictif.
**Vérifié bénin** : (1) les manifestes k8s (`deploy/k8s/jobs/*.yaml`) ne montent PAS `/tmp` en tmpfs
(seul un PVC `scratch` sur `/var/surch` + de petits emptyDir disque) ; (2) surtout, les mesures deces
existantes caractérisent le page cache de `source.dat` comme **évictable** (delta `process_rss` −
`jemalloc anon` = page cache), ce qui PROUVE que `/tmp` est un fs disque (overlay), pas tmpfs. Donc le
gain RAM de C1b sera RÉEL. Robustesse optionnelle : pointer explicitement vers `/var/surch` (PVC) via
un env `SURCH_DATA_DIR` (non bloquant). Toujours **mesurer l'anon ET le container RSS SOUS limite** au gate C1b.

## ✅ C1a-batché VALIDÉ (bench 28636114241, HEAD 51a4f70)
- `count` 1 355 728 = docs, **verdict PASS** (refresh ne crashe plus). Indexation **12 179 doc/s**
  (~parité ES, la batche ne coûte rien). Codec round-trip vert en CI.
- **Ratio FoR 3,36×** : postings **518 MiB RAM → 154 MiB disque** (page-cache évictable). Prémisse
  disk-backed validée : 518 MiB sortent de l'anon en C1b.
- **`skipped_terms` = 12 266** (~0,2% des ~5,46 M termes) : des termes ont des doc_id NON strictement
  croissants (champs `Value::Array` ES éclatés sans dédup → doublons). Rendu best-effort (segment
  shadow ne crashe jamais l'indexation). **Enjeu C1b** : ces termes n'ont pas de couverture disque →
  il faut soit (a) **dédupliquer `(term, doc_id)` dans `PostingsBuilder`** (fix de CORRECTION : le df
  Lucene compte un doc une fois, pas N ; à valider oracle car ça touche l'idf), soit (b) fallback RAM
  pour ces termes. La dédup est préférable (corrige aussi une inflation df latente du chemin RAM).

## ✅ Dédup (term,doc_id) VALIDÉE (commit 22b2747, gates verts)
Fix de correction Lucene multi-valué (`dedup_merge_postings` au build, freq=somme, positions non
stockées donc rien à merger). Corrigeait un df/tf latent-FAUX que `term_scoring_view` calculait sur
les 12 266 termes à doublons.
- **ci-k8s b1-oracle-gate : `divergence_count: 0`** (parité deces vs OpenSearch PARFAITE, 0 divergence).
- **ci-k8s ndcg-gate : vert** (BEIR scifact + TREC-COVID NDCG@10 tenus — cross-corpus intact).
- **bench deces : `skipped_terms` → 0** (tous FoR-encodables, C1b débloqué), count PASS, indexation
  13 630 doc/s, latence p95 match/bool/full 1,8/1,5/1,5 ms (marginalement meilleure). Mono-valué byte-identique.

## 🏆 C2 (id_maps flat) + C1b flag-ON — ≤ES/2 ATTEINT (bench 28682072869, sha-a664ed8)
Aplatir les 3 id_maps (FST uid→doc_id + reverse `Box<[u8]>`+offsets + `documents: Vec<Option<SourceBlob>>`)
+ drop `intern_index` a tué les ~1,36 M `Arc<str>` qui épinglaient les slabs fragmentés :

| axe | C1b seul | **C2 + C1b flag-ON** | ES | vs ES |
|---|---:|---:|---:|---:|
| jemalloc allocated (live) | 731 | **532** | — | — |
| jemalloc **active** | 1402 | **844** | — | ≈ cible 843 |
| **RAM anon (resident)** | 1446 | **881 MiB** | 1685 | **0,52× ES** |
| match / bool / full p95 (ms) | 2,0/2,6/2,5 | 2,0/2,8/2,6 | 4,3/3,5/3,0 | sous ES |
| indexation doc/s | 13 222 | 12 827 | ~12 000 | ≥ ES |
| skipped_terms | 0 | 0 | — | — |

**Surch bat ES ~2× sur la RAM (0,52×) HONNÊTEMENT (disk-backed, pas tout-en-RAM), tout en restant sous
ES en latence sur les 3 types, indexation ≥ ES.** Le frag interne a chuté de 671 → ~312 MiB (active−allocated),
exactement le mécanisme prédit par le triple consensus (tuer les small-alloc long-vivants dépingle les slabs).

**✅ PARITÉ SCELLÉE : oracle b1 vrai-corpus VERT — `divergence_count: 0, divergences: []`** (ci-k8s run
28689787902, sha-69668db, sur les nouveaux nœuds `burst-rwx`). Le changement d'ordre match_all (lex→insertion)
est bit-parfait vs OpenSearch. + CI verte (6 tests id_maps + test parité flag-ON==flag-OFF).
(Le blocage oracle initial était infra : rescaling cluster → pool `burst`→`burst-rwx`, 8 manifests re-pointés.)

## ⚠️ SCORECARD HONNÊTE (deces 1,36 M) — objectif ≥2× ES sur chaque axe : NON tenu (sauf qualité)
Correction d'un sur-claim antérieur : j'avais comparé l'anon de Surch au RSS conteneur d'ES (biaisé).
La comparaison honnête = **RSS conteneur vs RSS conteneur** (mesuré, bench 28682072869) :

| axe | cible | Surch | ES 8.6.1 | verdict |
|---|---|---:|---:|:--|
| **RAM (RSS conteneur)** | ≤0,5× | **2378 MiB** (anon 881, +page-cache) | 1698 | ❌ **1,40× ES** (pire ; anon 881 > cible 843) |
| latence match/bool/full p95 | ≤0,5× | 2,0 / 2,8 / 2,6 ms | 4,3 / 3,5 / 3,0 | ❌ match 0,47× seul ; bool 0,80× full 0,87× |
| indexation | ≥2× | 12 827 doc/s | 11 563 | ❌ 1,11× (loin de 2×) |
| disque | ≤0,5× | mesure cassée (0) | 0 | ❌ non mesuré |
| parité oracle | parité | 0 divergence | — | ✅ |
| corpus | 28 M | 1,36 M | — | ❌ subset |

**Ce qui EST réel** : l'archi disk-backed fonctionne, parité bit-parfaite, anon non-évictable 3797→881
(−77%). **Ce qui N'EST PAS tenu** : RAM ≤ES/2 (le disk-backed remplace l'anon par du page-cache → RSS
conteneur PIRE qu'ES sans limite ; et l'anon 881 > 843 → OOM sous limite 843, JAMAIS testé sous limite).
Latence/indexation/disque loin du barème 2×. **Prochain gate honnête obligatoire : tourner Surch sous
limite cgroup (843, 1024, …) et mesurer tient/latence — c'est LE test ≤ES/2, non fait.**

## ✅✅ C1b FONCTIONNEL + MESURÉ (flag-ON, bench 28651348936, sha-94e11a8)
Read-path disk-backed câblé derrière `SURCH_POSTINGS_DISK`, dual-path, **parité flag-ON==flag-OFF
bit-identique** (test `postings_disk_parity` vert). Mesure flag-ON sur deces 1,36 M :

| axe | flag-OFF (RAM) | **flag-ON (disque)** | ES |
|---|---:|---:|---:|
| RAM anon (jemalloc resident) | 1916 | **1446 MiB = 0,86× ES** | 1685 |
| jemalloc allocated (live) | 1119 | **731** | — |
| match / bool / full p95 (ms) | 1,8 / 1,5 / 1,5 | **2,0 / 2,6 / 2,5** | 4,3 / 3,5 / 3,0 |
| indexation doc/s | ~13 000 | 13 222 | ~12 000 |
| skipped_terms | — | 0 | — |

**PREMIÈRE FOIS : Surch passe SOUS ES sur la RAM *honnêtement* (disk-backed, pas tout-en-RAM), tout
en restant SOUS ES en latence sur les 3 types.** Les 518 MiB de postings → 160 MiB de page-cache
disque évictable. Latence +0,2..+1,1 ms (coût décode), conforme (« ≤ES/2 latence non bankable disk-backed »).

**Barrage vers ≤ES/2 (843) = fragmentation jemalloc ~715 MiB** (resident 1446 − allocated 731). Ce
n'est PAS du dirty/muzzy (MALLOC_CONF déjà `dirty=0,muzzy=0,background_thread` + purge explicite) : c'est
de la VRAIE frag (extents retenus du churn PostingsBuilder). `allocated`=731 est DÉJÀ ≤843 → si on tuait
la frag, on passerait ≤ES/2. La réduire = **arène jemalloc dédiée aux allocs transitoires du build,
détruite après** (retourne tout l'extent), OU bump-alloc du builder. Dur + incertain. C0-retry subfields
(−118 live) + C2 id_maps (−134 live, requis pour 28 M) aident mais ne suffisent pas seuls (la frag reste).

## Séquence + flag + revert
0. **Dédup** ✅ FAIT (voir ci-dessus) — prérequis C1b : plus aucun terme non-encodable.
1. **C1a-batché** ✅ FAIT (voir plus haut) : writer par-champ best-effort au refresh + gauges
   (`disk_postings_bytes`, `disk_postings_skipped_terms`). Prochain sous-pas AVANT C1b :
   **dédup `(term, doc_id)`** (résout les 12 266 skips + corrige le df), bench-gate parité oracle.
2. **C1b sous flag runtime `SURCH_POSTINGS_DISK`** : conditionne le build (matérialiser les flats RAM ou
   pas) ET le read path. Bench A/B même image : decompose warm ET cold (fadvise DONTNEED), parité oracle,
   anon ventilé SOUS limite. Gate : parité bit-identique ; warm bool p95 ≤ 2,3 / full ≤ 2,0 (garde <ES) ;
   cold p95 ≤ 10 ms NVMe ; anon mesuré. Si gate rate → LRU 64 MiB, re-bench ; si encore → **flag off = revert 1-ligne** (sans rebuild).
3. **Drop des buffers RAM** (point de non-retour) en DERNIER, après N runs verts.
4. Puis **C0-retry subfields** (pour ≤843) puis **C2 id_maps** (pour 28 M).
