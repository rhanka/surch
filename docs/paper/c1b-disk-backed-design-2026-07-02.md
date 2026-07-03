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

## 🚨 CATCH CRITIQUE (Fable) : le segment ne doit PAS atterrir sur tmpfs
`source_store` utilise `std::env::temp_dir()`. Si `/tmp` est tmpfs dans le conteneur, les pages sont
**anon NON-évictables** → tout le gain RAM est FICTIF. Forcer un chemin disque réel (emptyDir medium
disk / PVC), et **mesurer l'anon SOUS limite cgroup** au gate.

## Séquence + flag + revert
1. **C1a-batché** (le pas sûr) : writer par-champ au refresh + gauges (bytes segment, temps write) +
   shadow decode `debug_assert(decode_blocked == RAM)` (CI) + criterion décode/bloc. Gate : indexation
   ≥ −5%, round-trip vert. Risque ~nul, RAM reste source de vérité.
2. **C1b sous flag runtime `SURCH_POSTINGS_DISK`** : conditionne le build (matérialiser les flats RAM ou
   pas) ET le read path. Bench A/B même image : decompose warm ET cold (fadvise DONTNEED), parité oracle,
   anon ventilé SOUS limite. Gate : parité bit-identique ; warm bool p95 ≤ 2,3 / full ≤ 2,0 (garde <ES) ;
   cold p95 ≤ 10 ms NVMe ; anon mesuré. Si gate rate → LRU 64 MiB, re-bench ; si encore → **flag off = revert 1-ligne** (sans rebuild).
3. **Drop des buffers RAM** (point de non-retour) en DERNIER, après N runs verts.
4. Puis **C0-retry subfields** (pour ≤843) puis **C2 id_maps** (pour 28 M).
