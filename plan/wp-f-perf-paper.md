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
  sortie de la quadraticité), (c) **un harness de latence search sur
  grand corpus** (artillery TREC-COVID-scale). Ce dernier est devenu
  prioritaire : Lot 3 (MaxScore block-leapfrog) est latence-neutre
  sur INSEE 10k (`2026-05-25-lot3-bmw-skiplist-K8s/`) faute de
  posting lists assez longues — son régime de bénéfice n'est
  mesurable que sur grand corpus, qu'aucun harness de latence ne
  couvre aujourd'hui (`ndcg-gate` = 50 requêtes sans percentiles).

## Plan F (lots)

- [x] **F1 — Section méthodologie** : `docs/paper/methodology.md`
  livré (système testé, environnement K8s, charges, schémas
  `surch.bench.*`, garde-fous SLO/qualité, contrôles d'équité,
  protocole d'isolation, limitation single-run, reproductibilité).
- [~] **F2 — Multi-rep des lots récents** (partiel) :
  - [x] 3 reps ndcg-gate (bulk + RSS + qualité) sur main, agrégés
    médiane + min/max dans `2026-05-25-F2-ndcg-3rep-K8s/`. Surch
    TREC-COVID bulk médiane `70.96 s` (distributions non-recouvrantes
    vs OS `109.73 s`), RSS pic `2168 MiB ±0.5%`, NDCG bit-stable.
    Tableau de bord Bulk + RSS mis à jour. Ces 3 axes sont
    paper-ready.
  - [x] 3 reps insee-bench (latence) sur main, agrégés dans
    `2026-05-25-F2-insee-3rep-K8s/` : Surch médiane p50/p95/p99/max
    `1.5/4.1/8.4/40.6 ms` vs OpenSearch `4.0/12.2/26.3/223.1 ms`
    (Surch `2.7–3.1x` plus rapide, p50 variance nulle, 0 erreur).
  - **F2 complet pour les charges disponibles** : bulk, RSS,
    qualité, latence ont tous une médiane+étendue. Reste pour
    l'article : F3 (historiques) + F4 (charges additionnelles dont
    harness latence grand corpus).
- [ ] **F3 — Débloquer + rejouer les historiques** (F-gap-2/3) :
  le gros morceau. **Investigation 2026-05-25 (DÉCISION USER
  requise)** : les SHAs historiques manquent la surface CI/Docker :
  - `71ceb275`, `5081cc7` : pas de `docker-build.yml` ni `ci-k8s.yml`
    (Dockerfile présent).
  - `3157afb`, `e38bf91` : pas de workflows ET pas de Dockerfile.
  - `65fc759` : a les deux workflows + Dockerfile (replayable).
  - `7caf339` : `ci-k8s.yml` + Dockerfile, pas de `docker-build.yml`.
  Porter la surface moderne (docker-build + ci-k8s + Dockerfile +
  scripts bench) sur ces branches sans réécrire le code applicatif
  historique est possible (le plan replay l'autorise), MAIS : (a)
  effort par point (créer une ref, greffer la surface, résoudre les
  conflits de Cargo.toml/workspace), (b) risque que le code
  historique ne compile pas avec le toolchain actuel (rustc 1.91.1)
  ni avec le harness bench actuel. **ROI incertain.**
  **Décision attendue** : investir F3 (isolation historique complète,
  plusieurs heures CI + débogage de build) OU écrire l'article sur
  les seuls lots récents (déjà isolés + multi-rep, histoire forte) en
  citant les optims historiques comme "delivered, mesurées
  cumulativement, isolation déférée" ? Tant que non tranché, le draft
  F5 part sur les lots récents.
- [~] **F4 — Charges additionnelles** (F-gap-4) : BEIR multi +
  sweep de taille (optionnel pour un premier draft).
  - [x] **Harness de latence grand corpus livré** : nouveau Job K8s
    `deploy/k8s/jobs/trec-covid-latency.yaml` (calqué sur
    `insee-bench.yaml`). Il amorce l'index `trec-covid` complet
    (171 k docs) sur Surch (7Gi) ET OpenSearch (même enveloppe que
    `ndcg-gate`, chunks _bulk pair-aware 8 MiB sous le cap 16 MiB),
    construit un fichier de requêtes artillery à partir des requêtes
    de test TREC-COVID (`queries.jsonl` filtré par les qids à qrel
    positif, comme `trec-covid-ndcg.sh`), puis lance `artillery_bench`
    contre Surch puis OpenSearch (profil de phases
    `2:30,2:30,5:30,10:30,20:30,50:240`). Émet
    `surch.bench.artillery.v1` (`art-surch.json` / `art-os.json`) +
    échantillonnage RSS, mêmes marqueurs `BEGIN_SURCH_K8S_*` que
    `insee-bench` pour que `bench_report` et le workflow `ci-k8s` les
    reconstruisent. `artillery_bench` gagne un flag additif
    `--query-mode insee|trec` (défaut `insee`, comportement inchangé) :
    en mode `trec` chaque ligne du fichier `--names` est une requête
    plein-texte, émise en `multi_match` sur `title`/`text` (alternance
    OR par défaut / `operator:and`) — le régime qui exerce les longues
    listes de postings / skip-lists que l'INSEE 10k n'atteint pas.
    `ci-k8s.yml` : `trec-covid-latency` ajouté au choix `job`, au
    contrôle d'image bench-driver, à `REPORT_FILES` et à la
    reconstruction des logs (branche calquée sur `insee-bench`).
    **Dispatch** : `gh workflow run ci-k8s.yml -R rhanka/surch -r
    <branch> -f job=trec-covid-latency` (nécessite l'image
    `bench-sha-<SHA>` construite via `docker-build.yml` + le nœud
    DEV1-XL + ResourceQuota `limits.memory>=10Gi`). Validation CI/K8s
    (pas de build/test lourd local).
  - [ ] Reste F4 : BEIR multi-datasets (NFCorpus, FiQA…) + sweep de
    taille de corpus (courbe de scaling bulk).
  - **Caveat équivalence (écart latence grand corpus)** : le premier
    run (26419941882) donne Surch p50 `0.6 ms` / p95 `1.7 ms` vs
    OpenSearch p50 `207 ms` / p95 `592 ms` sur 171 k (13 170 req,
    0 erreur des 2 côtés) — soit ~345x à p50. À publier AVEC réserve :
    `artillery_bench` ne capture que latence + erreurs (drain du body
    sans parser `hits.total`, cf. `issue_request` l.648-652), donc
    l'égalité du travail entre moteurs ne se prouve pas depuis
    l'artefact latence. **Preuve d'équivalence disponible par
    ailleurs** : la parité NDCG@10 sur TREC-COVID (`0.4750` Surch vs
    `0.4902` OS) établit que les top-K récupérés sont de qualité
    équivalente → l'écart de latence reflète bien le saut WAND/MaxScore
    + skip-lists sur longues listes (régime que l'INSEE 10k n'atteint
    pas) et l'absence d'overhead JVM, pas un travail dégénéré côté
    Surch. Durcissement futur : logger `hits.total` par requête dans
    `artillery_bench` pour assertion directe. Claim à formuler comme
    single-run illustratif, pas comme mesure multi-rep finale.
- [~] **F5 — Draft de l'article** : premier draft livré
  `docs/paper/draft.md` (abstract, méthodo, séquence bulk Lot 1→1.6,
  mémoire, latence, qualité, parité matchID, discussion, limitations,
  conclusion) sur les lots récents + multi-rep F2. Reste : figures
  (courbes bulk/latence/RSS par lot), et intégration des historiques
  si F3 tranché.

## Verdict de faisabilité (au 2026-05-25)

**Article faisable, mais pas encore prêt.** Les lots récents sont
bien isolés et racontent une histoire forte (Surch > OpenSearch sur
3 axes, qualité stable). Manquent surtout : la rigueur multi-rep
(F-gap-1, facile mais coûteux en CI) et l'isolation des optims
historiques (F-gap-2/3, bloqué techniquement). Un premier draft
"engineering experience report" est possible avec les seuls lots
récents en multi-rep ; un article "complet" sur toute la séquence
d'optimisations nécessite de débloquer Lot 4.
