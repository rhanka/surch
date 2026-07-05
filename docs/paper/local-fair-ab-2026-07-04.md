# A/B local équitable ES vs Surch (pinné cgroup) — 2026-07-04

Harnais `deploy/bench-local/fair-ab.sh` : docker, **CPU pinné** (`--cpuset-cpus=0-7,16-23`, 8 cœurs
physiques du Ryzen AI Max+ 395), **RAM cappée identique** (`--memory=M --memory-swap=M`, swap conteneur
OFF), **ES Xmx=Xms=M/2** (l'autre moitié = mmap Lucene = page-cache, analogue au disque de Surch). Les
deux moteurs indexent le MÊME corpus brut INSEE from-scratch, en séquence (pas de contention). Anti-triche :
ES ne peut déborder ni mémoire ni CPU (il déborde tant qu'il peut sinon). RSS = `docker stats` = cgroup
`memory.current` (page-cache inclus).

## Résultats (659 780 docs, corpus 6 champs, surch disk-backed C1b)

**Plancher de survie sous limite mémoire :**
- **ES** : OOM sous ≤1024m (heap ≤512m insuffisant pour booter/indexer) ; survit à partir de **1536m**.
- **Surch** : survit à **512m** (RSS 65 MiB) ; plancher < 512m.

**À 1536m (plancher ES, les deux survivent) — CPU + RAM égaux :**
| axe | ES | Surch | Surch/ES |
|---|---:|---:|:--|
| RSS conteneur | 1382 MiB | 65 MiB | **0,05×** (21× moins) |
| indexation | 48 280 doc/s | 78 971 doc/s | **1,64×** |
| disque index | 220 MiB | 142 MiB | **0,65×** |
| latence p50/95/99 | 4,4 / 6,1 / 7,6 ms | 0,8 / 1,1 / 1,6 ms | **~0,2×** (~5× plus rapide) |
(à 2g : ES 46 570 doc/s / RSS 1691 MiB ; surch 78 753 / RSS 66 MiB — même tableau.)

## ⚠️ CE QUE CES CHIFFRES NE PROUVENT PAS (honnêteté — ne PAS reclamer « bat ES » sur cette base)
1. **Corpus BEAUCOUP plus léger que le vrai deces matchID.** Ici 6 champs simples + `_source` minuscule.
   Le vrai deces = docs riches multi-champs. C'est pourquoi le RSS surch ici = 65 MiB alors que sur SCW
   à 1,36 M (schéma réel) l'anon surch = **881 MiB** et il **OOMait à 843m**. **Non comparable.** Le
   poids par doc pilote tout (FST, postings, subfields, id_maps).
2. **🐛 Surch UNDERCOUNT 10 000 / 659 780 (1,5 %) déterministe** (`bulk_err_chunks=0` ; ES n'en perd que 1
   = une ligne malformée de mon awk). Reproductible, NON résolu par refresh par-chunk. À 40k ça ne se
   produit pas ; apparaît à grande échelle. Vrai comportement surch à INVESTIGUER (perte de données >
   toute métrique). Complétude surch = 98,5 %.
3. **🔴 Refresh surch TRÈS coûteux (finding architectural)** : refresh-final unique = 83 607 doc/s, mais
   refresh PAR CHUNK = **5 878 doc/s** (66 refreshes) → surch devient **6× PLUS LENT qu'ES** (34 842).
   Cohérent avec l'archi `rebuild_index` (reconstruction complète du `TermDictionary` à chaque refresh).
   **L'avantage indexation de surch n'existe QU'EN bulk-puis-refresh-unique ; il s'effondre en indexation
   near-real-time** (refresh fréquent), là où ES (segments incrémentaux) reste stable. Axe indexation à
   qualifier selon le pattern de refresh, pas un « 1,64× » universel.
4. Gouverneur `powersave` (biais fréquence, non fixé). Débits gonflés vs SCW (docs légers + NVMe local +
   Zen5 pinné) pour LES DEUX.
5. 659k ≠ 1,36 M ≠ 28 M. Le plancher mémoire scale avec le corpus (surch OOMait 843m à 1,36M sur SCW).

## 🎯 VERDICT — VRAI corpus deces 1,36 M (28 champs, mapping matchID réel), A/B équitable pinné
Corpus `deces-1.36M.ndjson` (1 360 000 docs, mapping matchID `deces_index.yml` : norm/edge_ngram/.raw/dates),
harnais robuste (undercount corrigé). Sweep mémoire, 8 cœurs pinnés chacun, caps égaux, surch disk-backed.

**Plancher de survie mémoire (LE résultat) :**
| cap | ES | Surch |
|---|---|---|
| 768m | ❌ OOM boot | ❌ |
| 1536m | ✅ **survit** | ❌ OOM |
| 2g | ✅ | ❌ **OOM à l'indexation** (count=0) |
| 3g | ✅ | ✅ **survit** |

→ **ES survit à 1536m ; Surch exige 3g** (OOM à ≤2g, tué par le PIC d'indexation). **Sur le vrai corpus,
sous caps égaux, Surch a besoin de ~2× la mémoire d'ES pour survivre.** L'objectif RAM ≤ES/2 est non
seulement non tenu, il est **INVERSÉ** — c'est ES qui tient à ≤ moitié de la RAM de Surch.

**Comparaison à 3g (les deux survivent) :**
| axe | ES | Surch | Surch vs ES |
|---|---:|---:|:--|
| plancher survie | 1536m | 3072m | ❌ **ES 2× mieux** (survit à moitié) |
| RSS steady-state @3g | 2197 MiB | 688 MiB | Surch 0,31× (steady bas) |
| latence p50/95/99 | 1,30/1,81/2,10 ms | 0,39/0,57/0,63 ms | ✅ **Surch ~0,30× (3,3× + rapide, ≤ES/2)** |
| indexation | 28 513 doc/s | 29 842 doc/s | ~parité (1,05×) |
| disque index | 653 MiB | 744 MiB | ❌ Surch 1,14× (pire) |

## 🎯🎯 VERDICT FULL CORPUS 28M (28 917 511 docs riches, mapping matchID) — sweep mémoire constante
Corpus `~/surch-bench-data/deces-28M.ndjson` (56 fichiers INSEE publics 1970-2025, IDs vérifiés sur la
passe complète). Mêmes bornes : 8 cœurs pinnés chacun, `--memory=--memory-swap`, ES Xmx=M/2, surch disk-backed.

| cap | ES | Surch |
|---|---|---|
| **16g** | ✅ 22 719 doc/s · RSS 10,26 GiB · disk 11,6 GiB · lat 0,91/1,41/2,03 ms · count 28 917 511 | ❌ **OOM indexation** |
| **8g** | ✅ 25 180 doc/s · RSS 6,54 GiB · disk 12,2 GiB · lat 0,91/1,40/2,12 | ❌ OOM (~11,2M docs avant mort) |
| **4g** | ✅ 24 468 doc/s · RSS 3,24 GiB · disk 12,6 GiB · lat 0,90/1,33/2,93 | ❌ OOM (~5,1M docs avant mort) |

**Lecture :**
1. **ES indexe et sert le full 28,9M à TOUS les budgets jusqu'à 4 GiB** (plancher réel ≤4g), débit stable
   ~22-25k doc/s, latence quasi-inchangée. Son RSS **s'adapte élastiquement au budget** (10,3 → 6,5 → 3,2 GiB) :
   architecture segments-disque + heap borné = dégradation gracieuse.
2. **Surch ne survit à AUCUN budget testé jusqu'à 16 GiB.** Progression ~1,4M docs/GiB avant OOM (5,1M@4g,
   11,2M@8g) → plancher extrapolé ~**24-32 GiB** pour 28,9M (pic bulk + pic refresh). Le build tout-en-RAM
   ne dégrade pas : il meurt.
3. **À mémoire constante sur le full corpus, l'axe RAM est ≥4× en faveur d'ES** (4 vs >16 GiB), et les axes
   latence/indexation/disque de Surch sont NON MESURABLES à 28M (jamais réussi à construire l'index).
4. La latence 3,3×+rapide de Surch (réelle à 1,36M) ne compte que si l'index tient — à 28M il ne tient pas.
   Conclusion architecturale : **l'indexation streaming-vers-segments à pic borné est LE prérequis** de tout
   objectif à l'échelle ; sans elle, aucun autre axe n'existe au-delà de ~2M docs/GiB de budget.

## CONCLUSION HONNÊTE (objectif = battre ES ≥2× sur CHAQUE axe)
- **RAM ❌ INVERSÉ** : Surch exige 2× la RAM d'ES pour survivre (pic d'indexation). Steady-state bas (688)
  mais le pic build (OOM à 2g) impose le cap haut. C'est LE point dur, cohérent avec SCW (OOM 843m à 1,36M).
- **Latence ✅** : le SEUL axe réellement gagné ≥2× (3,3× plus rapide, ≤ES/2). Solide.
- **Indexation ❌** : parité (1,05×), pas 2×.
- **Disque ❌** : Surch un peu pire (1,14×).
Verdict : sur le vrai corpus en conditions loyales, **Surch gagne franchement la LATENCE (≥2×), perd la RAM
(inversée), égale l'indexation, perd un peu le disque.** L'objectif « ≥2× sur chaque axe » n'est PAS atteint ;
le blocage central reste le **pic mémoire d'indexation** (build tout-en-RAM avant écriture segments) — à
casser (build streamé/segmenté) pour espérer la RAM. `powersave` (biais égal), 28M non lancé (procédure prête).
