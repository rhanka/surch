# Pivot mémoire — décision après 7 échecs

Date : 2026-06-09  
Auteur : campagne perf, post-mortem #15 (5 tentatives compression) + mmap M1/M1.1.  
HEAD au moment de la décision : `828fc77` (= inc1 baseline équivalent `b538bdf`, reverts inclus).

## 1. Pourquoi mmap M1/M1.1 a timeouté en chiffres

Baseline indexation : **94,4 s pour 1,36 M docs = 70 µs/doc** (run 27067004820,
sha-319f19a).  
mmap M1.1 timeout workflow : **58 min = 3480 s pour ≤ 1,36 M docs ≥ 2560 µs/doc =
~37× ralenti**, le timeout ayant probablement coupé bien avant la fin
(extrapolation 21 ms/doc dans la fin de course = ~300× ralenti).

Décomposition du surcoût per-doc (mmap M1.1, scénario `pwrite` append + page
fault read) :

| Composant | Coût modélisé | Cumul 1,36 M docs |
|---|---|---|
| `pwrite` extension fichier ext4/xfs cloud (block alloc + métadata) | 1–10 ms/doc | **23–230 min** |
| Syscall `pwrite` overhead (user→kernel→user) | ~1 µs | 1,4 s |
| `pread` lors d'`append_to_index` (M1) — résolu par M1.1 | ~5 µs (page-cache hot) | 7 s |
| Cache `indexed_fields` (M1.1) — ajouté pour éviter re-pread | ~négligeable | 0 |

**Conclusion** : la cause dominante est **l'extension répétée du fichier** sur
ext4/xfs sur disque cloud Scaleway. Le `pwrite` au-delà de la taille actuelle
force l'allocation d'un nouveau bloc + maj métadata + journal ext4. M1.1 a
résolu le côté lecture mais PAS le côté écriture, d'où re-timeout.

Le sanity check : `1,36 M × 1,5 ms ≈ 34 min`, cohérent avec le timeout 58 min
(qui inclut snapshot, gauges, K8s spin-up).

**Levier mécanique** : `posix_fallocate` la taille estimée AVANT le bulk supprime
ce coût (l'allocation passe à O(1) amorti, plus de journal par bloc). Ou alors
**ne pas faire 1,36 M `pwrite`** : batcher en RAM puis flush en un seul write
final.

## 2. Comparaison des 4 options

Gates SACRÉS (jamais à violer) :
- parité oracle b1/b2 = 0 divergence
- indexation ≥ 14 000 docs/s (= ne pas dépasser 97 s sur deces 1,36 M)
- match p95 < 2,1 ms ; bool/full p95 (gate non-régression actuel = 1,3 ms ± bruit)

| Option | Effort | Risque parité | Gain RAM | Coût indexation | Coût latence | Verdict |
|---|---|---|---|---|---|---|
| **A — mmap + `posix_fallocate`** | M (2-3 j) | bas (bytes identiques) | ~−1100 MiB stored_fields | **+5–15 % si fallocate OK ; >300 % si raté** | +0,2–0,5 ms (page fault sur cold doc) | risqué : si ext4 ignore fallocate (sparse file) on retombe sur M1.1 |
| **B — compression post-refresh** | S (1 j) | bas (bytes JSON identiques après decode) | ~**−800 MiB** (ratio JSON ~3,5×) | **0** (hot path Raw inchangé) | +100–200 µs top-K (decode ~5-10 µs × ~20 docs) | **option la plus sûre vs gates** |
| **C — batch write (Vec<u8> buffer + flush à refresh)** | M (2-3 j) | bas | ~−1100 MiB | +RAM pic transitoire ~1,2 GiB (le buffer) puis libéré | +0,2–0,5 ms (page fault) | annule le levier RAM PENDANT le bulk (le buffer = la RAM qu'on voulait économiser) |
| **D — A + B combinés** | L (4-5 j) | moyen (2 chemins en parallèle) | ~**−1600 MiB** | risque cumulé (fallocate + decode hot) | +400-700 µs | maximaliste, à viser SI B seul ne suffit pas |

### Pourquoi pas C en standalone

Le buffer Vec<u8> qui accumule 1,36 M blobs JSON pèse exactement le 1,2 GiB
qu'on tente d'évacuer. Pendant le bulk le pic RAM est strictement IDENTIQUE à
HEAD. Seul à refresh le buffer est flushé + libéré → on revient à zéro
gain pendant la phase chaude. Option à garder en réserve si on fait C+A
ensemble (A pour évacuer le pic, C pour amortir les syscalls).

### Pourquoi B est l'option recommandée

1. **Hot path bulk INSERT inchangé** : `upsert_document_deferred` continue
   d'insérer `Arc<str>` Raw. ZÉRO impact sur le gate ≥ 14 000 docs/s.
2. **Hot path `append_to_index` inchangé** : `parsed_source` voit Raw, parse
   direct, pas de decode. ZÉRO impact sur le gate indexation.
3. **Hot path scoring/build_hit** : `parsed_source` voit Compressed APRÈS le
   refresh, paie ~5-10 µs de decode (avec pool Decompress thread-local
   correctement bouclé jusqu'à `Status::StreamEnd`). Top-K = 20 → ~100-200 µs
   ajoutés sur match p95 (1,3 → ~1,5 ms, sous le gate 2,1 ms).
4. **Refresh** paie un coût UNIQUE (~5-10 s pour 1,36 M docs avec
   `Compress::compress_vec` réutilisé) JAMAIS dans le hot path bulk.
5. **Différence structurelle vs inc3c** : inc3c compressait à l'INSERT
   (touche bulk) + decompressait à `append_to_index` (touche bulk) +
   decompressait à `parsed_source` (touche search). Trois chemins chauds.
   Option B ne touche QU'UN chemin chaud (search, post-refresh, top-K).

### Pourquoi pas A en standalone (priorité 2)

A demande de redéfinir un store on-disk complet, recréer l'API mmap retirée
des commits 3c864af + c2fd9ad, et **dépend** de la garantie que `posix_fallocate`
n'est pas ignoré (sur certains FS / sparse files, fallocate retourne OK
mais l'allocation reste fictive). Si on choisit A, il faut ABSOLUMENT
mesurer le taux d'extension après fallocate avec `filefrag(8)` ou
`xfs_io fiemap` en CI. Possible mais coûteux en validation.

A est l'option à viser PHASE 2 (après B livré + mesuré) si on veut
descendre sous 4 GiB RSS, parce que B seule donne ~7100 MiB attendu et la
cible est ~4 GiB.

## 3. Recommandation chiffrée

**Option B — compression post-refresh**, engagement chiffré :

| Métrique | HEAD (inc1) | Cible post-B | Gate | Marge |
|---|---|---|---|---|
| RSS final deces 1,36 M | 7903 MiB | **~7100 MiB** (−800 MiB stored) | < 4000 MiB visé | reste à clore via #17c gap heap |
| Indexation | 94,4 s | **≤ 95 s** (∆ < 1 %) | ≥ 14 000 docs/s | OK |
| match p95 | 1,3 ms | **~1,5 ms** (+0,15 ms) | < 2,1 ms | OK |
| bool/full p95 | 1,3 ms | **~1,5 ms** (+0,15 ms) | non-régression | à mesurer |
| Refresh | ~3 s | ~8-13 s (+5-10 s) | non-bloquant | acceptable |
| parité oracle b1/b2 | 0 div | 0 div | SACRÉ | garanti par construction (Raw round-trip identique) |

**Engagement** : si B livre < 500 MiB de gain OU régresse l'un des gates,
REVERT et passer à A avec pré-mesure `posix_fallocate`. Pas d'itération B'.

## 4. Plan d'implémentation B

1. Introduire `enum SourceBlob { Raw(Arc<str>) | Compressed(Arc<[u8]>) }`
   dans `state.rs`.
2. Bascule `documents: BTreeMap<String, SourceBlob>`.
3. `upsert_document_deferred` insère TOUJOURS `SourceBlob::Raw`.
4. `parsed_source` :
   - `Raw(s)` → `serde_json::from_str(s)` (chemin actuel).
   - `Compressed(b)` → decode deflate dans un buffer thread-local (Decompress
     bouclé jusqu'à `Status::StreamEnd`, fix correctness inc3b/c), puis
     `serde_json::from_slice`.
5. `finalize_terms_for_refresh` appelle `compact_after_refresh()` qui :
   - itère `documents` ;
   - pour chaque `Raw(s)`, deflate-compresse via `Compress::compress_vec`
     thread-local bouclé (fix inc3b), remplace par `Compressed(b)`.
   - laisse `Compressed(_)` intact (idempotent).
6. La gauge `surch_index_stored_fields_bytes` compte la taille COMPRESSED
   après refresh (= la vraie RAM, ce qu'on veut sur scoreboard).

Hot path `append_to_index` (cas bulk-puis-refresh-puis-bulk) :
- Au 1er bulk : tous les blobs sont Raw → `parsed_source` parse direct → OK.
- Au refresh : compact → blobs deviennent Compressed.
- Au 2e bulk : `terms_finalized = true` → fallback rebuild_index, qui appelle
  `parsed_source` pour chaque doc — paie le decode (~5-10 µs × 1,36 M = 7-14 s
  ajoutés). Acceptable car ce chemin est déjà le fallback ré-build, pas le
  steady state.

## 5. Ce qu'il reste à valider sur cluster CI

État du worktree au moment de la livraison :
- branche : `worktree-agent-ab04eca27db33c0e9` (à merger sur `main`).
- HEAD : commit `[campaign-memory-reset] option B - compression post-refresh`.
- modifs : `crates/surch-api/src/state.rs` (+254 LOC nets : `SourceBlob`,
  `parse_source_blob`, `compact_after_refresh`, dispatch des accès `documents`).
- AUCUN `cargo test/clippy/check` lancé localement (consigne : valider via CI
  cluster, le PC user a déjà planté 5 fois sur ce sujet).

À valider sur le cluster CI (push de la branche) :

1. **Compile + clippy** : zéro warning, zéro erreur (Rust 1.81+).
2. **Tests unitaires** `crates/surch-api` : tous les tests `_refresh →
   search` round-trip doivent valider, notamment `bulk_router`,
   `matchid_autocomplete`, `matchid_compat`, `date_range` (≥ 5 suites
   couvrent le chemin `index_document → _refresh → search`).
3. **Run bench cluster K8s** sur deces 1,36 M, W=2, 5 reps :
   - RSS final cible **~7100 MiB** (−800 MiB vs 7903 baseline) ; tolérance ±100.
   - Indexation cible **≤ 95 s** (gate ≥ 14 000 docs/s = 97,1 s max).
   - `_refresh` durée cible **≤ 15 s** (vs ~3 s baseline) — instrumenter via
     log timing dans `finalize_terms_for_refresh`.
   - match/bool/full p95 cible **≤ 1,7 ms** (gate non-régression 2,1 ms).
   - parité oracle b1/b2 = **0 divergence** (SACRÉ).
4. **Décisions go/no-go** post-CI :
   - **ADOPTER** si tous gates verts ET gain RSS ≥ 500 MiB → mettre à jour
     scoreboard master-plan + reprendre #17c (gap heap 3,8 GiB) pour
     descendre vers la cible 4 GiB.
   - **REJETER + ITÉRER** si match p95 > 2,1 ms : tester `lz4_flex`
     (`compress_prepend_size`/`decompress_size_prepended`, ~3× plus rapide
     que deflate, ratio ~1,5× moins bon mais toujours > 2×) avant revert.
   - **REJETER + PIVOTER A** si indexation > 97,1 s OU gain RSS < 500 MiB :
     revert option B, reprendre l'option A (mmap + `posix_fallocate`) avec
     pré-mesure `filefrag` pour valider que ext4 honore fallocate.

Risque résiduel connu : pendant un re-build complet déclenché par
`set_mapping` ou `terms_finalized=true` post-refresh, `rebuild_index`
appelle `parse_source_blob` sur des `Compressed` (~5-10 µs × 1,36 M =
7-14 s ajoutés). Ce chemin n'est PAS le steady-state bulk ; il est déjà
le fallback "rebuild" assumé par la conception #15. Si une suite CI
mesure ce chemin spécifiquement (rare), elle verra le surcoût ; à
documenter mais pas à corriger.
