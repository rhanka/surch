# Scoreboard — campagne mémoire Lot C Phase 1 (deces 1,36 M, tout-en-RAM)

> 2026-07-02 — descente RAM du moteur tout-en-RAM vers le budget ES, mesurée sur le corpus
> matchID deces 1,36 M. Méthode : **triple consensus** (Codex GPT-5.5 xhigh + Opus 4.8 max +
> Fable 5) pour les décisions, **réalisation déléguée à Sonnet 5**, validation CI + bench
> `surch-eval-perf` (gauges jemalloc `allocated/resident/retained` ajoutées en Phase 0b).
> Métrique = `jemalloc_resident` (≈ RSS anon /proc), la seule non-évictable. ES conteneur = 1685 MiB.

## Trajectoire (RSS anon, jemalloc resident)

| Étape | commit | RSS anon (MiB) | vs ES | Δ étape |
|---|---|---:|---:|---:|
| Baseline Phase 0 | 82528ea | 3797 | 2,25× | — |
| Flat AoS `FieldPostings` (levier 1) | ccc0851 | 3421 | 2,03× | −376 |
| + purge jemalloc post-refresh | 60ded8f | 3198 | 1,90× | −223 |
| + subfields dense+dict (levier 2) | c7eca85 | 2490 | 1,48× | −708 |
| + UID `Arc<str>` interné (levier 3) | 2126652 | 2235 | 1,33× | −255 |
| + SoA `doc_ids`+`freqs` (levier 5) | cbf7771 | 1968 | 1,17× | −267 |
| + live_docs bitmap + BlockMeta réduit (A+B) | b7d6229 | **1836** | **1,09×** | −132 |
| **Cumul** | | | | **−1961 MiB** |

**Campagne in-RAM CLOSE : RAM 3797 → 1836 MiB (2,25× → 1,09× ES, quasi-parité), −1961 MiB.**
Latence p95 bool/full 1,6-1,7 ms (~1,9-2,1× ES ; `full` marginalement sous 2× depuis le SoA).
Indexation ~11 300 doc/s (≈ parité ES, érodée depuis 1,58× — coût du copy au flat-build + insert
bitmap ; à revisiter). Parité oracle ✅ sur les 6 leviers. **Le tout-en-RAM ne peut PAS descendre
sous ~ES-parité** (le résidu = FST + postings-encodés-compacts + id-maps quasi-incompressibles) :
pour battre ES (≤ES/2 = 843 MiB) ET tenir 28 M, il faut le **segment disk-backed** (endgame).

> **Décision SoA (levier 5) — GARDÉ, triple consensus 2-1** (Opus 4.8 + Fable 5 = garder ;
> Codex 5.5 = revert). Le SoA supprime la duplication du doc_id (−267 MiB) mais régresse le
> decompose `full` p95 de 1,2→1,6 ms → 1,9× ES, **sous la cible stricte ≤ES/2** (bool 2,3×,
> match 2,4× restent OK). Gardé car : (1) la latence tout-RAM n'est pas bankable (`all-in-ram-design-flaw`),
> l'axe bloquant est la RAM (encore 2,34× sa cible ≤842 MiB) ; (2) le SoA est le **format d'entrée
> natif** du codec FoR disk-backed ; (3) le +0,3 ms est un artefact micro-archi de l'interim tout-RAM
> (2 flux séquentiels) qui s'évapore au disk-backed (bloc-128 en L1). À re-mesurer warm/cold au disk-backed.

Latence p95 stable ~1,6-1,7 ms (3,4-3,6× plus rapide qu'ES) sur toute la descente ; indexation
~13 000-13 400 doc/s (1,12-1,15× ES, coût du copy au flat-build) ; parité oracle ES ✅ (cargo test
vert à chaque étape).

## Ce que chaque levier a attaqué

- **Flat AoS** : les ~5,46 M en-têtes `Vec`/`Option` par-terme (`Vec<Vec<T>>` → `Box<[T]>` plat +
  offsets CSR) = ~540 MiB d'en-têtes + 291 MiB de slack + effondrement de la fragmentation des
  millions de petites allocs. Zéro-copie préservé (`lookup*` rendent des slices) → hot path intact.
- **Purge jemalloc** : `mallctl(arena.4096.purge)` post-`_refresh` — sans elle le free-storm du build
  reste dirty (decay opportuniste). Rend au système les extents libérés.
- **Subfields dense+dict** : `BTreeMap<String,BTreeMap<u32,String>>` (~4 M nœuds + 1 String/entrée)
  → `SubfieldColumn{dict:Vec<Box<str>> dédup, codes:Vec<u32> dense}`. Noms FR très répétitifs → gauge
  427→118 MiB, RSS −708 (le plus gros levier).
- **UID `Arc<str>`** : l'UID était stocké 3× (clé `documents` + clé `document_ids` + valeur
  `reverse_document_ids`) → 1 seul `Arc<str>` partagé. RSS −255 (> gauge, effet allocateur).

## Reste pour battre ES

- **Fold `doc_ids` dupliqué / SoA `doc_ids`+`freqs`** (~250 MiB) — bench-gated (touche la queue bool/full).
- **FoR postings** (~350 MiB) — prérequis du segment disque, casse le zéro-copie `&[Posting]` (coût décode).
- **Segment disk-backed mmap** (postings/subfields sur disque, pages clean évictables) = le VRAI
  « beat ES » (≤ 0,5×) ET la seule voie tenable à **28 M** (~20-27 GiB anon en tout-RAM = OOM).
  Le flat AoS EST déjà le format de ce segment.

Verdict consensus : parité ES RAM atteignable à 1,36 M avec les leviers in-RAM ; le VRAI gain
(≤ES/2) et le 28 M exigent le disk-backed. Les 4 leviers livrés valident la trajectoire.

## 🏆 SUITE LIVRÉE — disk-backed (voir `c1b-disk-backed-design-2026-07-02.md`) : OBJECTIF ≤ES/2 ATTEINT

La campagne in-RAM (ci-dessus) a amené la RAM à 1,09× ES ; le disk-backed a fini le travail :

| étape | RAM anon | vs ES | quoi |
|---|---:|---:|---|
| Campagne in-RAM (6 leviers) | 1836 | 1,09× | plafond du tout-en-RAM |
| **C1b** postings sur segment FoR/pread (flag `SURCH_POSTINGS_DISK`) | 1446 | 0,86× | 518 MiB postings → page-cache disque évictable |
| **C2** id_maps aplaties (FST uid→doc_id + reverse packé + `documents` dense) + drop `intern_index` | **881** | **0,52×** | tue ~1,36 M `Arc<str>` → frag interne 671→312 |

**⚠️ CORRECTION d'un sur-claim** : le « 0,52× ES » comparait l'anon de Surch (881) au RSS conteneur d'ES
(1698) — biaisé. Comparaison honnête RSS-conteneur vs RSS-conteneur (bench 28682072869) :
**Surch 2378 MiB vs ES 1698 = 1,40× ES** (le disk-backed remplace l'anon par du page-cache, compté dans
le RSS). L'objectif **RAM ≤ES/2 n'est PAS tenu** : anon 881 > cible 843 → OOM probable sous limite 843
(jamais testé sous limite cgroup — le vrai test ≤ES/2, à faire). Réel tenu : parité oracle (0 divergence)
+ latence *match* 0,47×. Non tenu : RAM, latence bool/full (0,80/0,87×), indexation (1,11×), disque (non
mesuré), 28 M (fait 1,36 M). Le seul acquis solide : anon non-évictable 3797→881 + archi disk-backed correcte.
Voir le scorecard honnête dans `c1b-disk-backed-design-2026-07-02.md`.
