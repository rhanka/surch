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
| Indexation | 11 776 – 12 652 doc/s | 12 148 doc/s | **parité** |
| RSS conteneur | 2,63 – 3,04 Gio | 4,33 Gio | **surch** (0,61–0,70×) |
| RAM anonyme | 2 751 Mo | 3 835 Mo | **surch** (0,72×) |
| Plancher de survie 28M | **4 Gio** (2 runs consécutifs) | 4 Gio | **parité** |
| Disque | 12 296 Mio | 9 115 Mio | **ES** (surch 1,35×) |
| Latence sonde fixe | 1,44 / 1,69 / 1,89 ms | 3,11 / 4,09 / 5,42 ms | **surch** (2,2×) |
| **Latence sonde aléatoire** | 28,2 / 243,0 / 322,4 ms | **11,5 / 58,8 / 108,1 ms** | **ES** (~4× au p95) |
| Latence froide | 28,7 / 253,6 / 337,9 ms | 9,5 / 52,4 / 102,9 ms | **ES** |

**Score : 3 axes gagnés (mémoire, plancher, indexation), 2 perdus (disque, latence réelle).**

Le mot « latence » ne veut rien dire sans son profil de requêtes :
- **fixe** = un seul terme répété 1000 fois → tout est en cache, c'est le meilleur cas absolu ;
- **aléatoire** = noms tirés du corpus lui-même (distribution Zipf réelle), `size:10` → c'est le
  seul profil honnête, et c'est celui où ES gagne 4× au p95.

La claim historique « surch 2,5–3,4× plus rapide qu'ES » venait de la sonde fixe **et ne survit
pas** à la sonde aléatoire. Elle est retirée.

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
latence n'est pas dans le stockage des documents, elle est dans le chemin de recherche.

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

**Le dernier gate CI k8s vert date du 2026-07-04.** Depuis, 8 commits de code sont passés
(segments S3b, compression zstd, packing side-table, fetchs parallèles, profil L2) **sans aucune
validation de la qualité de recherche**. Les chiffres ci-dessus ne sont donc **pas** l'état actuel
du moteur : ils décrivent le moteur d'il y a trois semaines. C'est le principal trou du tableau de
bord — d'autant que ces mêmes commits ont profondément touché le stockage et le read-path.

## 5. Ce qui est solidement établi, et ce qui ne l'est pas

**Défendable aujourd'hui** :
- parité d'indexation et parité de plancher mémoire (4 Gio) au corpus complet, reproductible ;
- mémoire résidente 0,61–0,70× celle d'ES à budget égal ;
- 0 divergence fonctionnelle contre ES sur les oracles ;
- décomposition de latence : 96 % de la queue hors hydratation `_source` ;
- compression zstd : −34 % de disque, coût −3,5 % en indexation, < 1 % en lecture.

**Non défendable / à ne plus affirmer** :
- « surch est 2–3× plus rapide qu'ES » — vrai seulement sur la sonde fixe (cache chaud), faux en
  requêtes réelles ;
- toute claim sur la latence de queue ou le comportement à froid ;
- toute claim de qualité de recherche à l'état actuel du code (gates périmés).

**Non mesuré du tout** : concurrence (toutes les sondes sont séquentielles, une requête à la fois),
near-real-time (indexation pendant la recherche), mises à jour et suppressions à l'échelle,
agrégations, redémarrage à froid après crash.

## 6. Prochain front

1. **Instruire le chemin de recherche** — 96 % de la queue de latence, aucune mesure fine
   aujourd'hui. Même méthode que L2 : profil par requête, phases (résolution des termes, lecture
   des postings, intersection, scoring, collecte du top-k), compteurs (termes, postings lus,
   octets, docs scorés, segments visités). Aucune optimisation avant cette mesure : le cycle L1 a
   coûté trois semaines faute d'avoir instrumenté d'abord.
2. **Remettre les gates qualité au vert** (ci-k8s / OpenSearch) sur le code actuel — sinon le
   tableau de bord restera aveugle sur l'axe pertinence.
3. Corriger les deux défauts du harnais L2 (offset d'export JSONL par phase, sonde froide sous
   root).
4. Mesurer la concurrence : c'est l'angle mort le plus proche d'un usage réel.
