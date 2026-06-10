# Verdict final 2026-06-10 — campagne autonomy bouclée

HEAD `b5722a8`. Dernière mesure : perf-W2 `27276634301` (deces 1.36 M, W=2).

## Scoreboard final — chiffres mesurés

### Latence (TOUS shapes 2× ES atteints !)

| Shape | Surch p50/p95/p99 | ES p50/p95/p99 | Ratio p95 |
|---|---|---|---|
| match | 1.4 / **1.9** / 2.7 ms | 2.2 / 3.9 / 5.7 ms | **2.0×** ✅ |
| bool | 0.9 / **1.7** / 2.6 ms | 1.8 / 3.6 / 5.0 ms | **2.1×** ✅ |
| full | 0.9 / **1.7** / 2.7 ms | 1.8 / 3.7 / 5.5 ms | **2.2×** ✅ |
| probe global | 0.9 / 1.7 / 2.7 ms | 2.5 / 6.0 / 8.9 ms | **3.5×** ✅ |

**Verdict latence : ✅ 2× ES STRICT atteint sur les 4 indicateurs.**

### Indexation

| Métrique | Surch | ES | Ratio |
|---|---|---|---|
| bulk_s | **98.1 s** | 115.0 s | **1.17× plus rapide** |
| docs_per_second | **13 817** | 11 789 | **1.17×** |

Verdict indexation : 🟡 1.17× ES (cible STRICT 2×, manque 1.7× supplémentaire).

### Mémoire RSS

ES peak/final = 1678 / 1675 MiB.
Surch RSS non isolable (Artillery hang prend le sampler RSS au pic
post-bulk pré-refresh, AVANT que option B compresse les _source).
**Mesure RSS Surch propre = à dispatcher après fix Artillery hang.**

Gauges Prometheus mesurés sur run précédent 27270656283 :
- `surch_index_stored_fields_bytes = 554.6 MiB` (post-option B, vs 1187
  baseline = **−632 MiB confirmés**).
- `surch_index_postings_capacity_slack_bytes = 0` (mythe démoli, pas de
  slack à récupérer côté postings).

### Disque

**Architecture blocker** : option B garde tout en RAM. Pas de fichier
sur disque. La comparaison "Surch disque vs OS disque" n'a pas de sens
sans persistance.

Action pour débloquer : (a) réactiver mmap M1 (file-backed _source via
`pread`), ou (b) implémenter snapshot natif `_snapshot/<repo>/_create`
puis mesurer la dir snapshot.

### Qualité NDCG (ndcg-gate run 27242686637)

| Dataset | Surch | OS | Δ |
|---|---|---|---|
| SciFact | **0.6599** | 0.6537 | **+0.0062** ✅ Surch beats OS |
| TREC-COVID | 0.4777 | 0.4902 | -0.0125 (était -0.0152, **+18 % rapproché** par SmallFloat) |

## Synthèse — 5 axes vs 2× ES/OS STRICT

| Axe | Verdict | Détail |
|---|---|---|
| **LATENCE** | ✅ **VERT 2× atteint** | bool/full/match p95 = 2.1×/2.2×/2.0× ES |
| **INDEXATION** | 🟡 1.17× | manque 1.7× ; leviers FoR + parallélisation + FST batch |
| **MÉMOIRE** | 🟡 partiel | option B confirme −632 MiB stored_fields, RSS final non isolable Artillery hang |
| **DISQUE** | 🔵 architecture | non mesurable sans persistance (option B RAM-only) |
| **QUALITÉ NDCG** | 🟢 SciFact ✅ / 🟡 TREC-COVID -0.0125 | SmallFloat partial fix ; reste idf rounding / norm boost |
| **Parité oracle b1/b2** (SACRÉ) | ✅ vert sur toute la chaîne | jamais touché |

## Bilan campagne autonomy (2 sessions, 2026-06-09 + 2026-06-10)

**Livré et confirmé en mesure cluster** :
- Option B compression _source post-refresh : −632 MiB stored_fields
  mesurés (553 → 554 MiB après / 1187 avant).
- #18 NDCG SmallFloat : SciFact battu (+0.0062), TREC-COVID rapproché
  +18 % (−0.0152 → −0.0125).
- #17c slack gauge : mythe démoli (0 byte slack à récupérer).
- Latence STRICT 2× ES atteint sur les 4 indicateurs (match, bool,
  full, probe global).
- Indexation 1.17× ES (proche, pas STRICT).

**Bloqueurs identifiés** :
- Artillery hang (concurrent bulk-search stall pré-existant) prend le
  sampler RSS au mauvais moment → mesure mémoire imprécise.
- Disque architecture pending : besoin persistance pour la mesure.
- NDCG TREC-COVID -0.0125 résiduel : au-delà de SmallFloat (norm boost,
  coord factor, idf rounding).
- Indexation 2× ES demande codec FoR + parallélisation bulk + FST batch.

## Tâches restantes (planning hors autonomy)

1. Investigation **Artillery hang** (concurrent bulk-search stall) → débloquerait
   mesure mémoire + utilisation cluster sans wait.
2. **Codec FoR + parallélisation bulk** → indexation 2× ES.
3. **mmap M1 réactivé** ou **snapshot natif** → axe disque mesurable.
4. **NDCG TREC-COVID** root-cause complémentaire → fermer la parité STRICT.
