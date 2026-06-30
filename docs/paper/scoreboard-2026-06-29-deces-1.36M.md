# Scoreboard — matchID deces 1,36 M (réel) — Surch `sha-ea15496` vs ES 8.6.1

> 2026-06-29 — run `surch-eval-perf` `28405896293`, corpus deces **1 355 728 docs** (snapshot dev
> `esdata_eb84b2eb_74bab91a`), runner `ubuntu-latest`, probe workers=2. Chiffres capturés via le
> **checkpoint pré-Artillery** (commit `52677bc` sur surch-eval) — le runner est mort pendant
> Artillery (load-test end-to-end trop lourd pour ubuntu-latest), mais les mesures cœur ont survécu.
>
> ⚠️ **1,36 M n'est PAS le full.** Le full deces = **28 M** (bucket prod). Ce run est un
> intermédiaire de confirmation, pas le full.

## Chiffres

| Axe | Surch | ES 8.6.1 | Ratio | Cible (×2 / ÷2) | Verdict |
|---|---|---|---|---|---|
| Latence moteur p50 | 0,7 ms | 2,3 ms | 3,3× + rapide | ≤ ES/2 | ✅ |
| Latence moteur p95 | 1,4 ms | 5,1 ms | **3,6×** | ≤ ES/2 | ✅ |
| Latence moteur p99 | 2,9 ms | 7,4 ms | 2,6× | ≤ ES/2 | ✅ |
| Decompose p95 match | 1,4 ms | 4,3 ms | 3,1× | ≤ ES/2 | ✅ |
| Decompose p95 bool | 1,2 ms | 3,5 ms | 2,9× | ≤ ES/2 | ✅ |
| Decompose p95 full | 1,2 ms | 3,0 ms | 2,5× | ≤ ES/2 | ✅ |
| Indexation bulk | 15 269 doc/s | 12 050 doc/s | 1,27× | ≥ 2× | 🟡 miss |
| RSS pic | **5 418 MiB** | 1 684 MiB | **3,2× pire** | ≤ ES/2 | ❌ |
| Disque | non capturé | — | — | ≤ ES/2 | ⚪ |

Bulk : Surch 88,79 s / ES 112,5 s (le `total_s` inclut le `dump_s` ES-side, hors périmètre).
0 erreur, 0 bulk_failure des deux côtés ; counts == 1 355 728 == expected des deux côtés.

## Lecture

- **Latence = vrai gain, propre, > 2× partout** (2,5–3,6×). Point fort confirmé sur corpus réel.
- **Indexation = 1,27×** seulement (les 8,9× BEIR étaient corpus-dépendants). Sous la cible 2×.
- **Mémoire = 3,2× PIRE.** Le « 16,9× mieux » d'insee-bench 10k était un **artefact de petit corpus**.
  Le mmap a sorti le `_source` (RSS 9951 → 5418 MiB) ; restent en RAM postings + FST + 2 maps
  String→id (1,36 M) + doc_len + live_docs.

## Implication 28 M (chemin critique)

À 1,36 M l'architecture tout-en-RAM tient (5,4 GiB) mais **perd la mémoire**. À 28 M (~20×) →
**~110 GiB RAM**, infaisable mono-nœud. **Le full 28 M est bloqué par l'architecture tout-en-RAM**
tant que le disk-backed/S3-natif (étude `s3-native-storage-study-2026-06-29.md`, Lot C : découpler
l'executor du `MemoryStore`, lecture paresseuse des blocs de postings) n'est pas livré. L'axe
mémoire et le full 28 M ont la **même** dépendance.

## Suites

1. Capturer le **disque** (le step a tourné mais n'a rien écrit ce run — à corriger).
2. Lot 0 S3 (CAS atomique) puis Lot C (executor disk-backed) = débloque mémoire + 28 M.
3. Pour Artillery sur gros corpus : runner non-`ubuntu-latest` (décision en attente).
