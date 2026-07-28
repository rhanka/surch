# État des lieux benchmark — surch vs Elasticsearch / OpenSearch (2026-07-25)

Document de synthèse : ce qui est **mesuré**, contre **qui**, sur **quel corpus**, à **quelle date**,
et ce qui est **périmé ou non établi**. Les détails de chaque campagne restent dans
`verdict-28M-6g-2026-07-11.md` (3 dernières semaines) et
`retrospective-4-axes-memoire-constante-2026-07-04.md` (historique).

## 0. Deux moteurs de référence, deux rôles distincts

| Référence | Version | Rôle | Corpus | Fraîcheur |
|---|---|---|---|---|
| **Elasticsearch** | 8.6.1 | performance 4 axes + parité fonctionnelle | deces réel 28 917 511 docs | **à jour** (25/07) |
| **OpenSearch** | 2.17.1 | qualité de pertinence (BEIR) + gates CI k8s | SciFact, TREC-COVID, NFCorpus, FiQA | **périmée** (04/07) |

Les deux ne se chaînent pas : moteurs et corpus différents. Aucun ratio ne doit être transporté de
l'un à l'autre.

## 1. Performance — 4 axes vs Elasticsearch 8.6.1, à mémoire constante

Corpus deces 28 917 511 docs, **budget mémoire strictement égal (6 Gio de cap cgroup pour les deux
moteurs)**, VM dédiée calme, 8 cœurs par moteur, harnais `deploy/bench-local/fair-ab.sh`.
« RSS conteneur » = mémoire physique du cgroup (heap + page cache, `memory.current`).

| Axe | surch (packing + zstd) | ES 8.6.1 | Verdict |
|---|---:|---:|---|
| RAM anonyme | 2 751 Mo | 3 835 Mo | **surch** (0,72×) |
| Plancher de survie 28M | **4 Gio** (2 runs consécutifs) | 4 Gio | **parité** |
| Indexation | 11 776 – 12 652 doc/s | 12 148 doc/s | parité **⚠ suspecte** (§1bis) |
| Disque | 12 296 Mio | 9 115 Mio | **ES** (surch 1,35×) |
| **Latence sonde aléatoire** | 28,2 / 243,0 / 322,4 ms | **11,5 / 58,8 / 108,1 ms** | **ES** (~4× au p95) |
| Latence froide | 28,7 / 253,6 / 337,9 ms | 9,5 / 52,4 / 102,9 ms | **ES** |
| ~~Latence sonde fixe~~ | ~~1,44 / 1,69 / 1,89 ms~~ | ~~3,11 / 4,09 / 5,42~~ | **MESURE INVALIDE** (§1bis) |

**Score honnête : 2 axes gagnés (mémoire, plancher), 1 douteux (indexation), 2 perdus (disque,
latence). AUCUN axe latence n'est gagné.**

### 1bis. Deux mesures invalidées par revue de code (Opus 5 max, 25/07)

**La sonde « fixe » ne mesurait pas surch.** Double vice, vérifié dans le code :
1. `fair-ab.sh` requêtait le champ `nom` en dur alors que le mapping 28M expose `NOM` →
   **zéro hit**, aucune hydratation, aucun scoring réel (le commentaire du script assumait ce
   défaut « par continuité historique ») ;
2. `search_response_cache_eligible` (`search.rs:1128`) rend le **cache applicatif de réponses
   actif PAR DÉFAUT** ; la sonde rejouant 1000 fois le même corps, les 999 mesures suivantes
   servaient un `Vec<u8>` mémorisé. Le contournement `request_cache=false` n'existe que depuis
   `e2cb078` (23/07), donc **après** le triptyque, les gates packing et L1. ES, lui, ne cache pas
   les requêtes `size:10` par défaut → comparaison structurellement inéquitable.

Première mesure propre (L2, sonde corrigée sur `NOM`, cache désactivé) : **25,00 / 28,44 /
31,03 ms**. Le signe s'inverse — et le fait le plus instructif est que la sonde fixe
(`NOM:MARTIN`, le nom le plus fréquent du corpus) est **deux fois plus lente** que la sonde
aléatoire (12,72 ms au p50) : la latence de surch croît avec la fréquence du terme, signature d'un
parcours proportionnel au nombre de documents portant le terme.
Ces valeurs viennent d'une VM à 3 cœurs, non comparables telles quelles à l'ES 8 cœurs ; un
face-à-face propre reste à refaire.

**« 96 % de la queue dans le chemin de recherche » est une sur-attribution.** L'énoncé exact est
« hors hydratation `_source` » : la mesure est prise côté client, un processus `curl` neuf par
requête sur un conteneur sonde non pinné, et le `took` du moteur (déjà exposé via
`surch_dbg_run_us`) n'a jamais été scrapé. Le coût de sonde n'est donc pas séparé du coût moteur.

**Indexation « à parité » : à confirmer.** Six configurations — trois codecs, 3 vs 8 cœurs —
atterrissent toutes entre 11 776 et 12 652 doc/s, ES compris. Un débit insensible au retrait de
5 cœurs suggère que le **feeder** (un `curl` par chunk, 2 892 chunks séquentiels) est la ressource
limitante, pas les moteurs. Mesure d'une heure à faire avant toute claim.

## 2. Où part la latence (mesure instrumentée, 25/07)

Décomposition par requête aléatoire `size:10`, sur volume OVH `classic` (= le `block-standard`
des Jobs K8s), profil benchmark-only par requête :

| Poste | p50 | p95 |
|---|---:|---:|
| Requête complète | 12,72 ms | 191,57 ms |
| Hydratation `_source` | 3,70 ms (29 %) | 6,86 ms (**3,6 %**) |
| Décompression zstd | 0,04 ms | 0,08 ms |
| Parse JSON | 0,06 ms | 0,11 ms |
| **Recherche + scoring + HTTP** | **9,02 ms (71 %)** | **184,7 ms (96 %)** |

Un témoin `size:0` qui ne lit **aucun** document mesure quand même 185,66 ms au p95 : la queue de
latence n'est pas dans le stockage des documents. Elle se répartit entre le chemin de recherche et
le coût de la sonde elle-même, non encore séparés (§1bis).

### 2bis. Cause racine du p95 — identifiée dans le code, pas dans le disque

Preuve que ce n'est **pas** de l'attente disque : les mesures froides sont équivalentes aux
chaudes (L1 : p95 253,5 chaud vs 262,2 froid, p50 meilleur à froid). C'est du CPU. À 28,9M docs
l'index compte 12 segments, et ce seuil fait basculer le moteur sur un chemin de repli :

1. **Le parcours à saut est débranché** : `DiskPostingsCursor` sait ignorer des blocs entiers sans
   I/O, mais dès `segment_count() > 1` les appelants routent vers `conjunction_hits_merged`
   (`state.rs:2994`), qui matérialise les listes complètes dans un `BTreeSet` — un nœud d'arbre par
   document. Le commentaire du code l'assume : « out of scope until segment merging bounds the
   segment count again ».
2. **Aucune terminaison anticipée** : le total est calculé sur l'ensemble des candidats alors que
   l'affichage plafonne à `gte 10 000`.
3. **MaxScore n'élague rien** sur ce chemin (`threshold = NEG_INFINITY`), ce qu'un test du module
   entérine en vérifiant `blocks_skipped == 0`.
4. Les entrées de bloc sur disque ne portent **pas d'impact** (score maximal), ce qui rend
   impossible un élagage de type block-max WAND — seul point qui exigerait un changement de format.

Conséquence : la latence suit la somme des fréquences des termes. Le cas défavorable est un `bool`
« nom rare ET prénom courant » : Lucene travaille en proportion du terme le plus rare, surch en
proportion du plus fréquent. C'est un défaut d'implémentation localisé, pas une limite de
conception — et cela explique aussi pourquoi la sonde `NOM:MARTIN` est la plus lente de toutes.

Conséquence directe : la parallélisation des lectures `_source` (L1) a bien divisé le temps
d'hydratation par 2,16, mais n'a gagné que 9 % sur la requête — Amdahl, vérifié expérimentalement.
Le flag reste désactivé.

## 3. Parité fonctionnelle vs Elasticsearch — le socle

Oracle `b1` (rejoue des requêtes réelles matchID contre ES 8.6.1 et compare les réponses) :
**0 divergence**, revérifié à chaque livraison de code — compression zstd on/off, packing, fetchs
parallèles, profil L2. Oracles matchID : B1 30/30, B2 8/8.

C'est l'acquis le plus solide du projet : les optimisations n'ont jamais altéré les résultats.

## 4. Qualité de pertinence vs OpenSearch 2.17.1 — ⚠️ périmée

BEIR NDCG@10 (surch / OpenSearch), dernier relevé de juin :

| Jeu | surch | OpenSearch | Écart |
|---|---:|---:|---|
| SciFact | 0,6599 | 0,6537 | **+0,0062** ✅ |
| NFCorpus | 0,3033 | 0,3034 | parité |
| FiQA | 0,2294 | 0,2389 | −0,0095 🟡 |
| TREC-COVID | 0,4777 | 0,4902 | −0,0125 🟡 |

**Correction d'audit :** le vert CI k8s du 2026-07-04 est
`b1-oracle-gate` (`28689787902`), pas un gate BEIR. Le dernier artefact K8s
NFCorpus/FiQA est le run `26476471207` du 2026-05-26, sous
`docs/ops/bench-reports/2026-05-26-F4-beir-nfcorpus-fiqa-K8s/`. Depuis, les
commits segments S3b, compression zstd, packing side-table, fetchs
parallèles, profil L2, lot S, P1a, P2 et P3 sont passés **sans nouvelle
validation BEIR**. Les chiffres ci-dessus ne sont donc **pas** l'état actuel
du moteur. Le plan de remise en état et les seuils explicites sont dans
`docs/ops/beir-extra-ndcg-gate.md`.

## 5. Ce qui est solidement établi, et ce qui ne l'est pas

**Défendable aujourd'hui** :
- parité de **plancher mémoire** (4 Gio) au corpus complet, reproductible sur deux runs ;
- **mémoire anonyme 0,72×** celle d'ES à budget égal (2 751 vs 3 835 Mo) ;
- **0 divergence fonctionnelle** contre ES sur les oracles, revérifiée à chaque livraison ;
- l'hydratation `_source` ne pèse que 3,6 % de la queue de latence (donc : inutile de l'optimiser) ;
- compression zstd : −34 % de disque, −3,5 % en indexation, < 1 % en lecture.

**Non défendable / à ne plus affirmer** :
- **toute claim de latence, quel que soit le profil** — la sonde fixe était viciée (zéro hit +
  cache applicatif), et aucune mesure propre à conditions égales n'existe encore ;
- « indexation à parité » tant que le feeder n'est pas disculpé (§1bis) ;
- « RSS conteneur −30 % » : instantané pris avant les sondes, incohérent avec `anon + file` du même
  run — s'en tenir au plancher de survie et à la mémoire anonyme ;
- toute claim de qualité de recherche à l'état actuel du code (gates périmés) ;
- **l'objectif « 2× sur chaque axe »** : arithmétiquement hors d'atteinte sur le disque et
  l'indexation. À remplacer par « ≥ parité sur les 4 axes à mémoire constante, avec avantage
  démontré sur la mémoire » — décision de cadrage qui appartient au propriétaire du projet.

**Non mesuré du tout** : concurrence (toutes les sondes sont séquentielles, une requête à la fois),
near-real-time (indexation pendant la recherche), mises à jour et suppressions à l'échelle,
agrégations, redémarrage à froid après crash.

## 6. Prochain front

0. **Séparer le coût de sonde du coût moteur** (≈ 1 jour, zéro code moteur) : scraper le `took` des
   réponses et la gauge `surch_dbg_run_us` déjà exposée, pinner le conteneur de sonde, réutiliser
   la connexion au lieu d'un `curl` par requête. Sans cela, aucune claim de latence n'est possible.
1. **Terminaison anticipée et fin de la double matérialisation** dans le chemin de recherche : le
   total exact est calculé alors que l'affichage plafonne à 10 000 — gain attendu immédiat.
2. **Rebrancher le parcours à saut par segment** (`conjunction_hits_merged` → `DiskPostingsCursor`)
   : c'est LE chantier de fond. Passer de ~10⁶ à ~10⁴ documents scorés par requête placerait le p95
   dans la zone 10–25 ms, sous les 58,8 ms d'ES.
3. **Seuil compétitif réel** dans MaxScore (aujourd'hui `NEG_INFINITY`) et `max_term_freq`
   persisté au lieu d'être recalculé par requête.
4. Trancher l'indexation : mesurer si le feeder est le goulot (≈ 1 h).
5. **Remettre les gates qualité au vert** (ci-k8s / OpenSearch) — le tableau de bord est aveugle
   sur l'axe pertinence depuis 8 commits.
6. Corriger les deux défauts du harnais L2 (offset d'export JSONL, sonde froide sous root).

**Abandonné** : fetchs `_source` parallèles (3,6 % de la queue), sonde fixe comme axe de claim,
RSS conteneur comme métrique de tête, tout travail supplémentaire sur le stockage `_source`.
