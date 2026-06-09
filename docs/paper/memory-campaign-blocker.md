# Campagne mémoire — obstacle structurel diagnostiqué

Date : 2026-06-09
État : note d'arrêt après 7 tentatives. Documenté pour l'itération suivante.

## Tentatives et verdicts (toutes sur deces 1.36 M, W=2)

| # | Approche | RAM gain | Indexation | Latence | Décision |
|---|---|---|---|---|---|
| inc2-deflate | flate2 par appel | n/a | −40 % | n/a | REJET |
| inc2-zstd | zstd par appel | n/a | −32 % | n/a | REJET |
| inc2-zstd-redo | zstd no-dict par appel | n/a | −32 % | n/a | REJET |
| inc3 (d0bb0ab) | flate2 thread_local Compress::compress_vec | n/a | n/a | n/a | bug truncate large docs |
| inc3b (136016a) | inc3 + boucle Status::StreamEnd | n/a | n/a | n/a | bug DecompressError >1 MiB |
| inc3c (579202c) | flate2 high-level Encoder/Decoder | **−855 MiB** ✅ | **−56 %** ❌ | match p95 1.5→2.2 ms ❌ | REJET (2 gates SACRÉS violés) |
| **mmap M1** (3c864af) | _source file-backed via pread | (pas mesuré) | **timeout 58 min** ❌ | (pas mesuré) | REJET (workflow timeout) |
| **mmap M1.1** (c2fd9ad) | M1 + cache indexed_fields à l'écriture | (pas mesuré) | **timeout 58 min** ❌ | (pas mesuré) | REJET (cache n'a pas résolu) |

**Parité oracle b1/b2 = 0-divergence sur TOUTES** ces tentatives → l'architecture est correcte fonctionnellement.

## Diagnostic du timeout 58 min sur mmap M1.1

Hypothèses :
1. **Tokio runtime blocking** : `FileExt::write_all_at` est sync. Appelé depuis un handler axum async, il bloque le worker thread. Sur runtime multi-threaded, OK ; sur current-thread, fatal. Surch utilise `tokio::main` par défaut → multi-thread → ne devrait pas bloquer mais introduce queueing.
2. **Expansion fichier ext4/xfs** : chaque `pwrite` au-delà de la taille actuelle force le kernel à allouer un nouveau bloc + maj métadata. Sur disque cloud (Scaleway), ces ops peuvent prendre 1-10 ms. 1.36 M × 5 ms = 6800 s ≈ 113 min. ⚠ **Cohérent avec le timeout observé.**
3. **fsync implicite** sur close ou via syscall workflow.

Action de validation (TODO si on reprend ce sujet) : `posix_fallocate` la taille estimée du segment AVANT le bulk pour éviter l'extension répétée. Ou écrire en mémoire d'abord puis flush en une seule passe à `_refresh`.

## Leçons structurelles

1. **5 tentatives compression _source** sur la couche RAM : la couche est intrinsèquement chère (encode/decode + per-call codec init OU per-doc allocation). **Pas de solution simple.**
2. **2 tentatives file-backed** : le coût n'est pas la lecture (pread) mais l'écriture (fragmentation/fallocate). Solution implique `posix_fallocate` ou batching write.
3. **L'axe mémoire ne se gagne pas par optimisation à la marge.** Il demande un changement architectural majeur : soit du format on-disk mature (Lucene-like segments avec write-once-read-many), soit une compression bulk-time (post-refresh, pas hot path).

## Pivot suivant — par ordre ROI

| Chantier | Effort | Risque | Gain attendu |
|---|---|---|---|
| **#17c** instrumentation walker complet | S | bas (read-only) | localise le gap heap 3.8 GiB → identifie le VRAI levier |
| **#18** NDCG SmallFloat | M | bas (parité oracle vérifiée par test) | ferme la parité stricte qualité + bonus −65 MiB field_stats |
| mmap M2 (avec posix_fallocate + batched write) | L | moyen | levier RAM réel mais complexe ; reprise après #17c |
| **#19** mesure disque | S | nul | débloquée par mmap M2 |

**Décision** : maintenir HEAD = `f9af2c0` (équivalent inc1 baseline `b538bdf` + reverts) ; passer à #17c immédiatement, puis #18 en parallèle, et reporter mmap à une refonte ultérieure avec `posix_fallocate`.
