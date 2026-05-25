# wp-f-perf-paper Plan — Objectif F : article scientifique perf

Track principal: F - scientific write-up of the Surch performance
optimisation programme.
Branch: `main`.
Owner: conductor.
Status: open (2026-05-25). Goal set by the user: assess whether the
replayed perf evaluations of the Surch optimisations are rigorous
enough to support a scientific article, and close the gaps so they
are.

## Thèse de l'article

Un moteur de recherche OpenSearch-compatible écrit en Rust pur
atteint puis dépasse OpenSearch 2.17.1 sur l'indexation bulk, la
latence de recherche et l'empreinte mémoire, via une séquence
d'optimisations **mesurées et isolées** en K8s, **sans régression
de qualité** (NDCG@10 / Recall@10 stables), sur des charges BEIR
(SciFact, TREC-COVID 171k) et matchID (INSEE).

Résultat phare déjà acquis : bulk TREC-COVID `1002 s -> 56 s`
(`~17.8x`, Surch dépasse OpenSearch), RSS pic `4802 -> 2156 MiB`,
latence search p95 `-13%` / p99 `-18%` (skip lists), qualité
inchangée.

## Ce qui est déjà solide (publiable en l'état)

| Optimisation | Isolation K8s | Rapport |
|--------------|---------------|---------|
| Lot 1 — incremental bulk append | oui (avant/après) | `2026-05-24-ndcg-gate-incremental-bulk-K8s/` |
| Lot 1.5 — refresh-finalize RAM | oui | `2026-05-24-ndcg-gate-lot1.5-ram-K8s/` |
| Lot 1.6 — deferred FST build | oui + run de contrôle Lot 2-only | `2026-05-24-ndcg-gate-lot1.6-K8s/` |
| Lot 1.7 — jemalloc | oui | `2026-05-24-ndcg-gate-lot1.7-jemalloc-K8s/` |
| Lot 2 — skip lists (search) | oui, paired control `b9f6636` vs `d73c862` | `2026-05-25-insee-lot2-skiplists-K8s/` |

Atouts méthodo déjà en place :
- Environnement décrit (Scaleway burst pool, limites pod, tags
  d'image `sha-<full>`, run ids GHA).
- Parité allocator avec ES/OS (jemalloc des deux côtés depuis
  Lot 1.7) → comparaison équitable.
- Garde-fou qualité : NDCG@10/Recall@10 reportés à chaque lot,
  stables.
- Schémas machine stables (`surch.bench.*.v1`) + rapports humains
  promus, reproductibles depuis les artefacts CI.

## Gaps à combler pour un article rigoureux

- [ ] **F-gap-1 — Rigueur statistique** : toutes les mesures
  récentes sont en **single-run**. Le protocole replay Track A
  (`plan/perf-replay-wp-a-algo-ledger.md`) exige déjà `>= 3`
  répétitions avec médiane + IQR pour un verdict final. Re-rejouer
  les lots 1 / 1.6 / 1.7 / 2 en 3 reps (ndcg-gate pour bulk+RSS,
  insee-bench pour la latence). La variance de traîne observée
  (max 21.6 vs 64.1 ms entre 2 runs Lot 2) confirme le besoin.
- [ ] **F-gap-2 — Optims historiques non isolées** : la famille
  Lot -2 (top-K scalaire `5081cc7`, lazy `_source` `3157afb`,
  WAND OR-match `ed76014`, WAND multi_match `65ccfbe`, Block-Max
  WAND per-128 `e38bf91`, LRU cache `644f62b`, shared sources
  `4e9405a`, FST term dict `c5f3155`, per-block stats `b680232`,
  FoR metadata `df3b0aa`) est en statut "historical only" dans le
  tableau de bord — pas d'avant/après K8s individuel. C'est le
  Lot 4 / `perf-replay-wp-a-algo-ledger.md`, **bloqué** car les
  anciens SHAs n'ont pas la surface workflow (`docker-build.yml`,
  `ci-k8s.yml`).
- [ ] **F-gap-3 — Débloquer Lot 4** : créer des refs de replay
  durables (branches/tags) aux SHAs baseline/head historiques, y
  porter la surface CI/K8s actuelle SANS réécrire le code applicatif
  historique (le plan replay autorise déjà ce "plumbing autour du
  code historique"), puis rejouer A-replay-1/2/3.
- [ ] **F-gap-4 — Charges et généralité** : aujourd'hui SciFact +
  TREC-COVID + INSEE. Un papier gagnerait à (a) BEIR multi-datasets
  (NFCorpus, FiQA…) pour la généralité qualité, (b) un sweep de
  taille de corpus pour la courbe de scaling bulk (montrer la
  sortie de la quadraticité).

## Plan F (lots)

- [ ] **F1 — Section méthodologie** : rédiger
  `docs/paper/methodology.md` (harness K8s, schémas `surch.bench.*`,
  protocole replay, environnement, équité allocator, garde-fou
  qualité). Rédigeable MAINTENANT — la méthodo est déjà en place.
- [ ] **F2 — Multi-rep des lots récents** : 3 reps ndcg-gate
  (bulk+RSS) + 3 reps insee-bench (latence) pour Lots 1.6/1.7/2 ;
  agréger médiane + IQR ; mettre à jour les rapports.
- [ ] **F3 — Débloquer + rejouer les historiques** (F-gap-2/3) :
  le gros morceau, dépend de la surface workflow aux anciens SHAs.
- [ ] **F4 — Charges additionnelles** (F-gap-4) : BEIR multi +
  sweep de taille (optionnel pour un premier draft).
- [ ] **F5 — Draft de l'article** : assembler résultats + figures
  (courbes bulk/latence/RSS par lot) + discussion (Rust pur vs JVM,
  jemalloc, deferred FST, skip lists).

## Verdict de faisabilité (au 2026-05-25)

**Article faisable, mais pas encore prêt.** Les lots récents sont
bien isolés et racontent une histoire forte (Surch > OpenSearch sur
3 axes, qualité stable). Manquent surtout : la rigueur multi-rep
(F-gap-1, facile mais coûteux en CI) et l'isolation des optims
historiques (F-gap-2/3, bloqué techniquement). Un premier draft
"engineering experience report" est possible avec les seuls lots
récents en multi-rep ; un article "complet" sur toute la séquence
d'optimisations nécessite de débloquer Lot 4.
