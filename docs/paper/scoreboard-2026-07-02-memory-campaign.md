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
| + SoA `doc_ids`+`freqs` (levier 5) | cbf7771 | **1968** | **1,17×** | −267 |
| **Cumul** | | | | **−1829 MiB** |

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
