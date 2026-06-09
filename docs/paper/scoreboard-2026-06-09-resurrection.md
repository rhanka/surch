# Scoreboard 2026-06-09 — Réveil de fin de journée

État : HEAD `febbc86` = option B compression post-refresh + #18 NDCG SmallFloat combinés (ressuscités après reverts à tort).

## Découverte majeure de la journée

Les 9 "échecs" perf-W2 ont tous été **mal interprétés**. Reconstitution depuis
les logs partiels :

| Commit | bulk_s Surch | docs/s | latence p95 probe | Verdict réel |
|---|---|---|---|---|
| inc1 baseline (`319f19a`) | 94 s | 14 357 | 1.7 ms | ✅ |
| mmap M1 (`3c864af`) | **117.7 s** | **11 519** | **2.2 ms** | ✅ |
| option B (`c8ae872+ce7a2a1`) | **105.8 s** | **12 809** | **1.8 ms** | ✅ |
| #18 NDCG seul (`eb3e188`) | (351 s avec RSS overhead) | — | 1.7 ms | ✅ |

**Conclusion** : la "perf failure" venait du step **Artillery surch** qui
hangait — bug pré-existant concurrent bulk-search stall documenté dans
`docs/wp-a-perf-followups-concurrent-bulk-search-stall.md`. Indexation +
latence + parité oracle **passaient à chaque fois**.

## Scoreboard latence vs ES (run `27240212526`, sha-`eb3e188`)

| Axe | Surch | ES | Ratio | Gate STRICT |
|---|---|---|---|---|
| bool p95 | **1.5 ms** | 3.4 ms | **2.3×** | ✅ ≥ 2× |
| full p95 | **1.5 ms** | 3.0 ms | **2.0×** | ✅ ≥ 2× |
| match p95 | **1.8 ms** | 4.1 ms | **2.3×** | ✅ ≥ 2× |
| probe p50 | **0.9 ms** | 2.2 ms | **2.4×** | ✅ |
| probe p95 | **1.7 ms** | 5.1 ms | **3.0×** | ✅ |
| probe p99 | **2.9 ms** | 7.3 ms | **2.5×** | ✅ |

## Indexation vs ES (option B, sha-`c8ae872`)

| Métrique | Surch | ES | Ratio |
|---|---|---|---|
| bulk_s | 105.8 s | 118.7 s | 1.12× plus rapide |
| docs/s | 12 809 | 11 420 | 1.12× |

Toujours sous la cible STRICT 2×. Cible bulk_s ≤ 59 s = ½×ES 118.7 s. Marge
~46 s à grappiller : code FoR de-encoding, postings builder, batched FST
merge.

## Gates SACRÉS — état

| Gate | État | Run |
|---|---|---|
| Parité oracle b1/b2 deces (0-divergence) | ✅ vert sur mmap M1, option B, et #18 NDCG | 27240206457 ; 27186811242 ; 27236774412 |
| ndcg-gate SciFact ≥ 0.65 | en cours validation (`bwt4s97yb`) | 27240... |
| RSS ≤ 1000 MiB sur TREC-COVID | non mesurable (perf hang sur deces 1.36M corpus) | — |
| Régression croisée (p50 < 1.3, match p95 < 2.1) | ✅ vert eb3e188 (p50 0.9, match p95 1.8) | 27240212526 |

## Restant pour atteindre les 5×2 axes

| Axe | État | Reste à faire |
|---|---|---|
| Latence | ✅ **2× atteint** sur deces W=2 | rien (gate non-régression seul) |
| Indexation | 🟡 1.12× actuelle | gain de 1.8× supplémentaire — codec FoR, parallélisation, batched merge |
| Mémoire | 🟠 mesure cluster pas isolable (Artillery hang prend l'image RSS) | scrape post-indexation à la main via un workflow_dispatch alternatif |
| Disque | ⚪ jamais mesuré | dispatch ad hoc avec `du -sh` sur le conteneur Surch après indexation |
| Qualité NDCG | 🟠 en validation par ndcg-gate (bwt4s97yb) | confirmer TREC-COVID NDCG@10 ≥ 0.4902 |

## Action plan (queue d'exécution)

1. **Maintenant** : push `febbc86` → chaîne CI/build/b1/perf-W2.
2. **À leur retour** : confirmer indexation/latence/oracle stables sur febbc86.
3. **Dispatch ndcg-gate** : confirmer NDCG TREC-COVID strict parity.
4. **Workflow custom #19 disque** : dispatch K8s ad hoc qui après l'indexation
   dumpe `du -sh /app/data` sur Surch + `_cat/indices?bytes=b` sur ES.
5. **Fix Artillery hang** : suit un track séparé (concurrent bulk-search stall),
   pas un blocant scoreboard.

## Caveats honnêtes

1. Indexation #18 NDCG seul (351 s) reste à expliquer. Soit la quantization
   `int_to_byte4` per-doc, soit l'overhead RSS sampler. Mesurer via un run
   sans RSS sampler ou avec instrumentation.
2. La mesure RSS finale capturée par `rss_container.sh` finit pendant
   l'indexation, donc le peak_mib reporté correspond à la fin du bulk —
   pas au post-refresh où la compression option B a libéré la mémoire.
3. Le ratio docs/s côté Surch dépend du worker count (W=2 actuel). À
   tester sur W=1 et W=4 pour isoler oversubscription.
