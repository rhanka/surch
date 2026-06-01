# wp-f-perf-paper Plan — Objectif F : article scientifique perf

Track principal: F - scientific write-up of the Surch performance
optimisation programme.
Branch: `main`.
Owner: conductor.
Status: F5 finalized for the current Track A readout (2026-05-31);
scale proof continues with the 28M deces indexation lane. Goal set by
the user: assess whether the replayed perf evaluations of the Surch
optimisations are rigorous enough to support a scientific article, and
close the gaps so they are.

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

- [x] **F-gap-1 — Rigueur statistique pour le readout courant** :
  les charges disponibles ont maintenant une confirmation multi-rep
  (F2 ndcg-gate pour bulk/RSS/qualité, F2 insee-bench pour latence,
  F4 trec-covid-latency 3-rep). Les isolations par lot restent
  single-run et sont présentées comme telles, pas comme verdicts
  statistiques finaux.
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
- [~] **F-gap-4 — Charges et généralité** : SciFact + TREC-COVID +
  INSEE sont maintenant complétés par NFCorpus + FiQA pour la qualité
  et par un harness TREC-COVID 171k de latence grand corpus. Le sweep
  de taille de corpus reste optionnel ; le prochain vrai front n'est
  plus une charge BEIR additionnelle mais le scale proof `deces` 28M.

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
    qualité, latence ont tous une médiane+étendue. Pour le readout
    courant, F2 ne laisse plus de blocage ; F3 reste seulement pour
    des claims historiques un par un, et F6 porte le prochain scale
    proof 28M.
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
  **Décision user 2026-05-25 : OUI, investir F3** (chiffrer les anciennes
  optimisations une à une). À faire en BACKLOG, après la priorité Track D.
  Approche : créer des refs durables aux SHAs baseline/head historiques,
  y greffer la surface CI/K8s actuelle (docker-build + ci-k8s + Dockerfile
  + scripts bench) SANS réécrire le code applicatif, puis rejouer
  A-replay-1/2/3.
  **BLOCAGE confirmé 2026-05-26 (DÉCISION D'APPROCHE requise)** : greffer
  juste les WORKFLOWS ne suffit pas. Le Dockerfile/CI moderne construit les
  binaires bench `artillery_bench`, `bench_report`, `b1_oracle`, `surch` —
  qui N'EXISTENT PAS dans le code historique (vérifié sur `e38bf91` : pas de
  ces binaires dans `crates/surch-demo/src/bin`). Donc une greffe minimale
  ne build pas. Les options réelles, toutes coûteuses :
  - (a) Greffer toute la surface bench (binaires + scripts) sur le vieux
    code → cascade de compilation (les binaires bench modernes appellent des
    APIs absentes des vieux crates) ; effort + ROI très incertains.
  - (b) Porter chaque vieille optimisation EN AVANT dans l'arbre moderne via
    un toggle de mesure (env) → archéologie de code par optim + flag de
    mesure (= l'approche refusée par le user pour l'isolation Lot 3).
  - (c) Reconstruire l'ancien outillage bench tel quel au SHA → comparaison
    inter-harness non apples-to-apples.
  **Décision user 2026-05-26 : option (b) sur branche isolée** (`perf-isolation`,
  jamais mergée main ; toggle de mesure zéro-impact-défaut). 1er PoC livré :
  - [x] **WAND/MaxScore isolé** (`SURCH_DISABLE_MAXSCORE`) sur TREC-COVID 171k :
    p99 51.4→5.3 ms (−90%), max 3915→308 ms (−92%) ; p50/p95 neutres. Rapport
    `2026-05-26-F3-wand-isolation-trec-covid-K8s/`. C'est une optim de TRAÎNE
    grand corpus (complète le « neutre sur INSEE 10k »).
  - [~] Suite F3 (mêmes toggles isolés) : WAND/MaxScore, cache LRU et
    top-K/lazy hydration ont maintenant des readouts. Restent FST,
    shared sources et autres historiques seulement si l'article final
    veut les revendiquer un par un. (b)-style, jamais sur main.
  - Note : (a) greffe historique exclue (binaires bench absents du vieux code).
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
  - [x] **Premier run vert publié** : `2026-05-25-F4-trec-covid-latency-K8s/`
    (GHA `26422565840` @ `9f53ba2`, 5 checks SLO PASS). Steady-state
    Surch p50 `0.5 ms` / p95 `1.3 ms` vs OpenSearch p50 `183.8 ms` /
    p95 `487.8 ms` sur 171 k (13 170 req, 0 erreur), RSS Surch
    `2135 MB` ≤ budget `2560 MB`. OpenSearch se dégrade sous charge
    (p50 → 193 ms à 50 RPS), Surch reste plat. Tableau par phase +
    caveats dans le README. **Débloque la mesure du régime Lot 3**
    (longues listes de postings) qu'INSEE 10k n'atteignait pas.
  - [x] **Multi-rep (3 reps) du harness latence grand corpus publié** :
    `2026-05-25-F4-trec-covid-latency-3rep-K8s/` (GHA `26422565840`,
    `26423474877`, `26424070888`). Médianes Surch p50 `0.5 ms` /
    p95 `1.3 ms` (variance nulle) vs OpenSearch p50 `176.9 ms` /
    p95 `481.4 ms` (~354x / ~370x), RSS Surch `2123 MB ±0.7%`, 0 erreur
    toutes reps. Le caveat single-run du landing F4 est levé.
  - [x] **Équivalence in-artefact livrée** : sonde non chronométrée
    `surch.bench.trec_hits.v1` dans le job (run `26424807778`) — 50/50
    requêtes non vides des 2 côtés, volume total apparié à `0.04 %`
    (Surch 7 507 757 vs OpenSearch 7 510 550). Caveat d'équivalence levé
    (addendum dans `2026-05-25-F4-trec-covid-latency-3rep-K8s/`).
  - [x] **BEIR multi-datasets (NFCorpus, FiQA) — LIVRÉ** (priorité user
    2026-05-26).
    - [x] Init shell additif `deploy/k8s/jobs/00b-init-beir-extra.yaml`
      (alpine busybox wget+unzip, no-Python, nonroot, idempotent).
    - [x] Script `beir-ndcg.sh <dataset>` générique + job `beir-extra-ndcg`
      + câblage ci-k8s + planchers SLO `bench_report` (nfcorpus 0.28, fiqa
      0.20). Run GHA `26476471207` : **NFCorpus** Surch `0.3033` vs OpenSearch
      `0.3034` (quasi-identique), **FiQA** `0.2294` vs `0.2389` (~4% sous).
      Rapport `2026-05-26-F4-beir-nfcorpus-fiqa-K8s/` + draft maj. Qualité
      Surch validée sur **4 datasets BEIR** (SciFact, TREC-COVID, NFCorpus, FiQA).
    - [ ] (optionnel) Sweep de taille de corpus (courbe de scaling bulk).
  - [x] **Caveat équivalence traité** : le premier run vert
    `26422565840` a ete promu, puis F4 a recu 3 repetitions et une
    sonde `surch.bench.trec_hits.v1` (50/50 requetes non vides des
    deux cotes, volume total apparié à `0.04 %`). Le claim F4 est donc
    multi-rep et non dégénéré. Il reste formulé comme cache-on /
    LRU-masked : le cache-off raw p50 Surch reste derrière OpenSearch
    (`309 ms` vs `169 ms`), donc ce n'est pas un claim raw-engine.
- [x] **F5 — Draft de l'article / reporting Track A finalisé** :
  `docs/paper/draft.md` (abstract, méthodo, séquence bulk Lot 1→1.6,
  mémoire, latence, qualité, parité matchID, discussion, limitations,
  conclusion) sur les lots récents + multi-rep F2, avec lecture finale A+F5.
  - [x] **Trajectoire par optimisation (OpenSearch + déces ES)** ajoutée dans la
    section finale de `docs/paper/draft.md`, avec effets cumulés et fronts
    ouverts par étape.
  - [x] **Données de figures livrées** : `docs/paper/figures/` (CSV
    plot-ready bulk-by-lot, RSS-by-lot, latency-by-corpus + provenance
    par SHA/rapport) + rendus SVG (`bulk-trec-covid-by-lot.svg`,
    `rss-trec-covid-by-lot.svg`, `latency-by-corpus.svg`). Référencées
    dans l'en-tête du draft.
  - [x] **Readout final A+F5** ajouté dans `docs/paper/draft.md` :
    performances atteintes, caveats cache-on/cache-off, limites 28M, et
    prochain incrément recommandé.
  - [x] **Double validation critique** : relecture locale + validation
    subagents `gpt-5.5` xhigh (read-only). Retours traités avant
    commit: overclaim cache-on supprimé, statuts F2/F3/F4 harmonisés,
    figure RSS alignée avec le point post-#9. Risques restants:
    SciFact à citer sous protocole F2, 28M non encore mesuré, tail
    déces p95/p99 encore derrière ES.
  - [ ] Suite post-F5 : full `deces` 28M indexation proof ES/Surch
    (durée bulk/dataprep, débit, RSS, doc count final, failure mode).

## Verdict de faisabilité (mise à jour 2026-05-31)

**Le readout Track A/F5 courant est finalisé comme engineering performance
report.** Il couvre les mesures multi-rep disponibles, les figures, la
trajectoire par optimisation, les caveats cache-on/cache-off, et la campagne
Elasticsearch `deces` 1.36M. Ce qui n'est pas encore un verdict fermé est
explicitement hors F5 courant : isolation historique complète si le papier veut
revendiquer chaque ancienne optimisation individuellement, et passage 28M pour
la preuve production-scale.
