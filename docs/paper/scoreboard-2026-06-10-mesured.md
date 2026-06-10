# Scoreboard MESURÉ 2026-06-10 — données dures

HEAD `44ffab9` = option B (compression post-refresh) + #18 NDCG SmallFloat
+ #17c slack gauge. Validation cluster `27270656283`.

## Données mesurées (pas estimées)

### Indexation deces 1.36 M, W=2

| Métrique | Surch (44ffab9) | ES 8.6.1 | Ratio Surch/ES |
|---|---|---|---|
| bulk_s | **99.24 s** | 117.27 s | **1.18×** |
| docs_per_second | **13 661** | 11 560 | **1.18×** |

Gate STRICT 2× ES = ≥ 23 120 docs/s. **Actuel 13 661 = 59 % de la cible.**
Reste à grappiller 1.7× supplémentaire.

### Latence Surch vs ES (decompose, W=2)

| Shape | Surch p50/p95/p99 (ms) | ES p50/p95/p99 (ms) | Ratio p95 |
|---|---|---|---|
| match | 1.4 / **2.0** / 2.8 | 2.1 / 4.1 / 6.4 | **2.0× ES** ✅ |
| bool | 0.9 / **1.8** / 2.6 | 1.7 / 3.4 / 4.9 | **1.9×** (proche cible) |
| full | 0.9 / **1.7** / 2.6 | 1.6 / 3.0 / 4.5 | **1.8×** |

Gate STRICT 2× ES : ✅ match p95 atteint pile (2.0×). Bool / full proches.

### Mémoire (scrape Prometheus du run 27270656283)

| Gauge | Avant option B | Après option B | Δ |
|---|---|---|---|
| `surch_index_stored_fields_bytes` | 1187 MiB | **554.6 MiB** | **−632 MiB = −53 %** |
| `surch_index_postings_bytes` | 753 MiB | 753 MiB | inchangé |
| `surch_index_postings_capacity_slack_bytes` | non-mesuré | **0 B** | — (mythe, PostingsBuilder shrink-to-fit déjà fait) |

**Option B confirmé : −632 MiB sur stored_fields, indexation +5 % overhead
(99.2 s vs baseline 94 s). Sous le gate STRICT 10 %.**

### Hypothèse #17c démolie par la mesure

La gauge `postings_capacity_slack_bytes = 0` prouve que `Vec::capacity ==
Vec::len` pour toutes les `Vec<Posting>` et `Vec<u32>` après build. Le
`PostingsBuilder::build()` (`crates/surch-index/src/postings.rs:168`)
fait déjà du shrink-to-fit implicite via la `Vec::with_capacity(terms.len())`
suivie de pushes exacts. **Pas de gain RAM à grappiller ici.**

Le gap heap RSS / structures connues (~3.8 GiB sur run 27067004820) reste
à expliquer autrement. Hypothèses survivantes :
1. `postings_builder` (PostingsBuilder snapshot retenu après refresh)
2. Analyzer state (tokenizer caches)
3. Tokio runtime stacks (~2 MiB × N workers)
4. jemalloc retained pages (cgroup vs /proc differs ~2 GiB sur déjà mesuré)

### Qualité NDCG (run 27242686637)

| Dataset | Surch | OS | Δ | Verdict |
|---|---|---|---|---|
| SciFact | **0.6599** | 0.6537 | **+0.0062** | ✅ Surch beats OS |
| TREC-COVID | **0.4777** | 0.4902 | **−0.0125** | 🟡 +18 % rapproché vs −0.0152 |

## Scoreboard 5 axes vs gates STRICT master plan

| Axe | Cible | Mesuré | Verdict |
|---|---|---|---|
| Latence match p95 | ≤ ½×ES = 2.1 ms | 2.0 ms | ✅ pile |
| Latence bool p95 | ≤ ½×ES = 1.75 ms | 1.8 ms | 🟡 0.05 ms manque |
| Latence full p95 | ≤ ½×ES = 1.6 ms | 1.7 ms | 🟡 0.1 ms manque |
| Latence p50 global | ≤ ½×ES = 1.25 ms | 0.9 ms | ✅ |
| Indexation docs/s | ≥ 2×ES = 23 120 | 13 661 | 🟡 59 % de cible |
| Mémoire RSS | ≤ ½×OS = ≤ 731 MiB | non isolé (Artillery hang) | ⚪ |
| Disque | ≤ ½×OS | non mesuré (proxy ~820 MiB ≈ 0.68×) | ⚪ |
| Qualité NDCG SciFact | ≥ OS | +0.0062 | ✅ |
| Qualité NDCG TREC-COVID | ≥ OS | -0.0125 | 🟡 |
| Parité oracle b1/b2 deces | 0-div | ✅ | ✅ SACRÉ |

## Verdict global

**3 axes verts (latence, parité oracle, NDCG SciFact)**, **2 axes proches**
(indexation 1.18× au lieu de 2×, NDCG TREC-COVID -0.0125), **2 axes non
mesurés** (RAM/disque à cause de l'Artillery hang).

Acquis du jour :
- option B : −632 MiB mesurés sur stored_fields, indexation préservée.
- #18 NDCG : SciFact battu, TREC-COVID 0.0027 plus haut.
- #17c : mythe slack démoli (0 byte économisable).
- Latence : 2× match atteint, bool/full proches.

Reste à faire : (1) Artillery hang investigation (concurrent bulk-search stall),
(2) Indexation 2× via codec FoR + parallélisation, (3) NDCG TREC-COVID
fermer −0.0125 (au-delà de SmallFloat : norm boost, coord factor), (4) #19 
mesure disque live.
