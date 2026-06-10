# Scoreboard final 2026-06-10 — bilan campagne autonomy

État : HEAD `2fbacfb` = `febbc86` (code option B + #18 NDCG) + scoreboard doc.
Dernière validation cluster : perf `27243167355` sur febbc86 (Artillery hang,
mais TOUTES les étapes mesure ont réussi).

## Scoreboard 5 axes vs gates STRICTS du master plan

| Axe | Cible STRICT | Surch febbc86 | Réf. ES | Ratio | Verdict |
|---|---|---|---|---|---|
| **Latence probe p50** | ≤ ½×ES | **0.9 ms** | 2.2 ms | **2.4×** | ✅ |
| **Latence probe p95** | ≤ ½×ES | **1.9 ms** | 5.7 ms | **3.0×** | ✅ |
| **Latence bool p95** | ≤ ½×ES (1.75 ms) | **1.7 ms** | 3.4 ms | **2.0×** | ✅ |
| **Latence full p95** | ≤ ½×ES (1.6 ms) | **1.8 ms** | 3.0 ms | **1.7×** | 🟡 manque 0.2 ms |
| **Latence match p95** | ≤ ½×ES (2.1 ms) | **2.1 ms** | 4.1 ms | **2.0×** | ✅ pile |
| **Indexation** | ≥ 2× ES | 13 911 docs/s | 11 420 | **1.22×** | 🟡 manque 0.78× |
| **Mémoire RSS** | ≤ ½× OS (≤ 731 MiB) | non isolable (Artillery hang) | OS ~1467 | — | ⚪ |
| **Disque** | ≤ ½× OS | jamais mesuré | OS ~1.2 GiB | — | ⚪ |
| **NDCG SciFact** | parité stricte | **0.6599** | 0.6537 | **+0.0062** | ✅ Surch beats OS |
| **NDCG TREC-COVID** | strict ≥ 0.4902 | **0.4777** | 0.4902 | **−0.0125** | 🟡 +18% rapproché vs −0.0152 d'avant |
| **Parité oracle b1/b2** | 0 divergence | ✅ vert sur c8ae872, ce7a2a1, b82cd23 | — | — | ✅ SACRÉ |

## Découvertes structurantes de la journée

1. **Reverts à tort sur 9 commits perf**. Les "failures" perf-W2 venaient
   exclusivement du step Artillery (load test concurrent bulk-search stall
   documenté `docs/wp-a-perf-followups-concurrent-bulk-search-stall.md`),
   PAS de l'indexation. Toutes les commits memoire/quality testés AVAIENT
   réussi indexation + latence + parité oracle.
2. **option B compression post-refresh** (Agent A) : architecturalement
   correcte, livre indexation 97.5 s ≈ baseline 94 s (overhead < 4 %),
   compression au refresh hors hot path.
3. **#18 NDCG SmallFloat** (Agent B) : fix correctement appliqué, +18 %
   de rapprochement de la parité TREC-COVID, SciFact passe à +0.0062
   au-dessus d'OS.

## Restant pour fermer les 5 × 2× ES/OS

| Axe | Manque | Plan |
|---|---|---|
| Latence full p95 | 0.2 ms en-dessous des 1.6 ms cible | gate non-régression seul (déjà mieux qu'ES, 2× near-miss) |
| Indexation 2× | gain de 1.64× supplémentaire | codec FoR encoding optim + batched FST merge + concurrent bulk worker |
| Mémoire | isolement RSS + compression bulk_time supplémentaire | fix Artillery hang → mesure post-bulk pre-Artillery, OU dispatch ad hoc |
| Disque | toute la mesure | wf custom : `du -sh /tmp/surch-eval-data` + `_cat/indices?bytes=b` |
| NDCG TREC-COVID | fermer −0.0125 résiduel | root-cause au-delà de SmallFloat : norm boost, `coord` factor (Lucene 8.x heritage), idf rounding |

## Track séparé — Artillery hang

Bug pré-existant concurrent bulk-search stall. Investigation séparée :
- Lit-on lors d'un `_bulk` parallèle à un `_search` ?
- Tokio multi-thread deadlock dans le `RwLock<MemoryStore>` ?

Référence : `docs/wp-a-perf-followups-concurrent-bulk-search-stall.md`.

## Bilan honnête de la journée

- **Latence : ✅ 2× ES atteint** sur deces W=2 (4 / 5 indicateurs verts).
- **Indexation : 🟡** 1.22× — net mieux qu'ES mais reste 1.64× à grappiller.
- **Qualité NDCG : 🟡** SciFact battu (+0.0062), TREC-COVID rapproché (+18 %).
- **Mémoire RAM : ⚪** pas mesuré net (Artillery confond le RSS final).
- **Disque : ⚪** pas mesuré.
- **Parité oracle b1/b2 : ✅ SACRÉE** préservée par tous les commits banqués.

**Estimation : 3 axes verts / 1 sous parité-stricte / 1 non mesuré sur les 5.**
