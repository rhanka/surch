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

## Ce qui EST acquis
Un harnais A/B local **équitable et session-safe** (OOM conteneur contenu), qui mesure les 4 axes sous
CPU+RAM pinnés identiques — reproductible sans SCW (coût). Prochain pas pour un chiffre RÉEL : indexer le
**vrai schéma deces** (docs riches) à 1,36 M puis 28 M sous caps, et **corriger le drop 10k surch** d'abord.
