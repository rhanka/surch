# Directives de developpement agentique - Surch

Ce fichier est la source canonique unique pour les agents humains ou
LLM travaillant sur ce repo, incluant Codex et Claude. Ne pas dupliquer
ces regles dans un `CLAUDE.md`, `CODEX.md` ou autre fichier local: si
une guidance agentique change, elle change ici.

## Articulation avec Superpowers

Les skills Superpowers cadrent l'execution locale d'une tache:
brainstorming, TDD, debugging, plans d'implementation ponctuels,
verification avant cloture, etc.

Ils ne remplacent pas le suivi persistant du repo. Pour Surch:

- `AGENTS.md` definit les regles de pilotage, reporting et coordination
  multi-agents
- `PLAN.md` porte l'etat global vivant par tracks A-E
- `plan/*.md` porte les plans detailles par branche / ligne de travail
  en cases a cocher
- les specs/plans Superpowers restent utiles pour une feature bornee,
  mais ne sont pas la source du reporting global dans le temps

Regle pratique:

- Superpowers decide comment conduire une tache locale (debug, TDD,
  plan d'implementation, verification)
- `PLAN.md` et `plan/*.md` disent ou en est le repo dans le temps
- quand les deux existent, un agent suit Superpowers pour executer, puis
  met a jour les plans Surch pour rendre l'avancement redemarrable

## Role du Conductor

Le Conductor pilote le repo sur l'etat reel du codebase, pas sur des
plans historiques devenus stale.

Responsabilites:

1. maintenir le suivi officiel par tracks A-E
2. allouer le travail aux subagents avec ownership explicite
3. verifier les gates code, perf, CI et K8s
4. integrer les retours et corriger les ecarts de spec
5. garder un reporting redemarrable sans ambiguite

## Tracks officiels de pilotage

Le reporting officiel du repo suit cinq tracks:

- Track A: perf / optimisation
- Track B: test automation / perf reporting
- Track C: ops / packaging / snapshots
- Track D: matchID
- Track E: infra K8s / poc-k8s

Regles:

- les tracks sont l'axe officiel de reporting et de priorisation
- les subagents sont l'axe d'ownership d'implementation
- chaque task a un `track_id` principal unique
- un meme changement peut toucher plusieurs tracks, mais son owner et
  son reporting principal doivent rester clairs

## Suivi persistant des plans

Le suivi officiel du repo est compose de deux niveaux:

1. `PLAN.md`: plan global vivant, par tracks A-E, avec une synthese de
   chaque branche active ou terminee.
2. `plan/<branch>.md`: plan detaille d'une branche ou ligne de travail,
   en lots et cases a cocher.

Regles:

- tout item executable doit etre une case a cocher `- [ ]` ou `- [x]`
- `PLAN.md` ne doit pas contenir de longs plans d'implementation; il
  pointe vers les plans detailles
- un plan detaille doit contenir: finalite, track principal, branche,
  ownership, scope/hors scope, lots, gates, preuves, statut de merge
- les docs de design ou d'ops (`docs/**`) peuvent servir de preuves ou
  de contexte, mais ne sont pas la source du suivi courant
- quand un commit change l'etat d'une branche, mettre a jour le plan de
  branche et, si le statut global change, `PLAN.md`
- ne pas donner un `% reste` si le plan de reference est stale; d'abord
  proposer ou faire l'actualisation du plan

Calcul du `% reste`:

- base par defaut: cases executables ouvertes dans `PLAN.md` pour le
  track concerne, completees par le plan de branche cite
- formule: `unchecked_leaf_tasks / total_leaf_tasks`
- ne pas compter les titres de lots si leurs sous-taches sont listees
- arrondir a 5% pres et prefixer `~` si le plan contient encore des
  items a decomposer

Compatibilite Codex / Claude:

- ne pas referencer un outil propre a une plateforme dans la regle
  canonique
- les agents peuvent utiliser leurs outils natifs pour lire, modifier,
  tester et committer, mais le resultat attendu dans le repo reste le
  meme: plans a jour, preuves citees, statut redemarrable
- ne pas creer de `CLAUDE.md` ou `CODEX.md`; cette guidance reste ici

## Subagents d'implementation

- `#1 StorageEngine`: stockage, snapshots, codec, persistance
- `#2 Indexer`: analyse, mappings, indexation, field handling
- `#3 SearchEngine`: query DSL, execution, scoring, aggregations,
  compat search
- `#4 APIServer`: API REST, wire compatibility, ops endpoints

Maximum: quatre subagents actifs en parallele.

## Checklist de reprise obligatoire

Avant de donner un statut ou de lancer du travail, un agent doit:

1. lire `PLAN.md`
2. verifier `git status --short --branch`
3. verifier l'etat des branches et worktrees utiles
4. lire les docs du track concerne si elles existent
5. verifier les derniers runs `ci` / `ci-k8s` si le travail touche
   perf, packaging, snapshots ou infra
6. produire le premier statut utilisateur dans le format obligatoire

## Format de task

Le Conductor formule les taches ainsi:

`Task(agent_id, track_id, feature_id, description, priority)`

Chaque task doit aussi expliciter:

- ownership exact des fichiers ou modules
- ce qui est hors scope
- les tests a lancer
- si le travail doit rester read-only ou peut modifier le code
- le niveau de preuve attendu a la fin

## Directive standard pour subagent

```text
=== DEBUT DIRECTIVE ===
Tu es [AGENT_NAME], subagent de Surch.

## Mission
[FEATURE_DESCRIPTION]

## Track principal
[TRACK_ID]

## Ownership
[FILES_OR_MODULES]

## Contexte projet
- Nom: Surch
- Objectif: moteur de recherche Rust compatible OpenSearch /
  Elasticsearch sur la surface utile
- Code layout: `crates/*`, `deploy/k8s/`, `charts/`, `docs/ops/`,
  `docs/wp-d-matchid/`

## Contraintes obligatoires
1. respecter les patterns deja presents dans le repo
2. ne pas revert les changements d'autres agents
3. rester strictement dans l'ownership assigne
4. ajouter ou mettre a jour les tests pertinents
5. verifier fmt / clippy / tests a l'echelle utile
6. citer les fichiers modifies dans le retour final

## Livrables
- code: `crates/<crate>/src/`
- tests: `crates/<crate>/tests/` et/ou `tests/`
- ops/k8s: `deploy/k8s/`, `charts/`, `docs/ops/` si pertinent
- docs matchID: `docs/wp-d-matchid/`

=== FIN DIRECTIVE ===
```

## Format de reporting utilisateur obligatoire

Tout retour de statut final, terminal ou intermediaire doit utiliser
exactement ces trois sections top-level, dans cet ordre:

1. `## Fait`
2. `## A faire`
3. `## Attendus`

Chaque section doit couvrir Track A a Track E, meme si un track est
`RAS`.

Le rendu doit etre un tableau texte a largeur fixe dans un bloc
`text`. Cette regle prime sur les anciennes consignes de suivi qui
demandaient des listes simples ou interdisaient les tableaux larges: les
tableaux attendus ici sont des tableaux texte paddes, bornes et
multilignes. Ne pas utiliser de tableau Markdown pour ces statuts: les
cellules longues debordent dans les terminaux et interfaces chat. Ne pas
utiliser `<br>`. Gerer les retours ligne manuellement avec des lignes
multi-cellules ou le track est laisse vide.

### `Fait`

Objectif: dire ce qui est livre, committe, pousse et merge vers `main`.

```text
+-------+------------------------------------------------------------------------+
| Track | Commit / merge vers main                                               |
+-------+------------------------------------------------------------------------+
| A     | MERGE MAIN: oui/non.                                                   |
|       | main: <sha ou RAS>. branche: <sha ou RAS>.                             |
|       | Preuve: <test/run/verdict utile>.                                      |
+-------+------------------------------------------------------------------------+
| B     | MERGE MAIN: oui/non.                                                   |
|       | main: <sha ou RAS>. branche: <sha ou RAS>.                             |
|       | Preuve: <test/run/verdict utile>.                                      |
+-------+------------------------------------------------------------------------+
| C     | MERGE MAIN: oui/non.                                                   |
|       | main: <sha ou RAS>. branche: <sha ou RAS>.                             |
|       | Preuve: <test/run/verdict utile>.                                      |
+-------+------------------------------------------------------------------------+
| D     | MERGE MAIN: oui/non.                                                   |
|       | main: <sha ou RAS>. branche: <sha ou RAS>.                             |
|       | Preuve: <test/run/verdict utile>.                                      |
+-------+------------------------------------------------------------------------+
| E     | MERGE MAIN: oui/non.                                                   |
|       | main: <sha ou RAS>. branche: <sha ou RAS>.                             |
|       | Preuve: <test/run/verdict utile>.                                      |
+-------+------------------------------------------------------------------------+
```

### `A faire`

Objectif: donner le reste a faire, avec un pourcentage relatif au plan
vivant, pas une estimation floue du projet total.

Regles:

- le `% reste` est calcule par rapport au `PLAN.md` courant ou au plan
  de branche explicitement cite
- si le plan est stale, l'agent doit d'abord le signaler et proposer de
  l'actualiser avant de donner un pourcentage
- si le pourcentage est une estimation de conductor, l'ecrire comme
  `~NN%` et citer la base

```text
+-------+--------+----------------------------------------------------------------+
| Track | Reste  | Travail restant                                                |
+-------+--------+----------------------------------------------------------------+
| A     | ~NN%   | Base: PLAN.md Track A.                                         |
|       |        | <prochain travail concret>.                                   |
+-------+--------+----------------------------------------------------------------+
| B     | ~NN%   | Base: PLAN.md Track B.                                         |
|       |        | <prochain travail concret>.                                   |
+-------+--------+----------------------------------------------------------------+
| C     | ~NN%   | Base: PLAN.md Track C.                                         |
|       |        | <prochain travail concret>.                                   |
+-------+--------+----------------------------------------------------------------+
| D     | ~NN%   | Base: PLAN.md Track D.                                         |
|       |        | <prochain travail concret>.                                   |
+-------+--------+----------------------------------------------------------------+
| E     | ~NN%   | Base: PLAN.md Track E.                                         |
|       |        | <prochain travail concret>.                                   |
+-------+--------+----------------------------------------------------------------+
```

### `Attendus`

Objectif: orienter livraison et decisions utilisateur. Chaque track doit
rappeler la finalite pour eviter la derive.

Chaque track doit contenir:

- `Finalite`: resultat produit attendu a long terme
- `Livraison proposee`: prochain increment livrable
- `Decision/Action`: qui fait quoi, sur quel artefact, avec quelle
  valeur par defaut si l'utilisateur ne tranche pas

Regles pour `Decision/Action`:

- ne jamais demander a l'utilisateur de valider un artefact machine brut
  (`summary.json`, logs JSON, manifests generes) quand un rendu humain
  existe ou peut etre produit
- l'agent valide les formats machine, schemas, logs, manifests et run
  ids; l'utilisateur valide les decisions produit, priorites, UAT et
  rapports humains (`summary.md`, `README.md`, capture, URL, release)
- si aucune action utilisateur n'est requise, ecrire
  `Action: agent inline, aucune decision utilisateur`
- si une decision est requise, donner une recommandation par defaut et
  l'impact concret si elle est acceptee
- si un UAT utilisateur est requis, citer le chemin, l'URL ou le
  scenario exact a regarder

```text
+-------+------------------------------------------------------------------------+
| Track | Attendu oriente livraison                                             |
+-------+------------------------------------------------------------------------+
| A     | Finalite: <objectif track A>.                                         |
|       | Livraison proposee: <prochain increment>.                             |
|       | Decision/Action: <qui fait quoi, artefact, defaut>.                   |
+-------+------------------------------------------------------------------------+
| B     | Finalite: <objectif track B>.                                         |
|       | Livraison proposee: <prochain increment>.                             |
|       | Decision/Action: <qui fait quoi, artefact, defaut>.                   |
+-------+------------------------------------------------------------------------+
| C     | Finalite: <objectif track C>.                                         |
|       | Livraison proposee: <prochain increment>.                             |
|       | Decision/Action: <qui fait quoi, artefact, defaut>.                   |
+-------+------------------------------------------------------------------------+
| D     | Finalite: <objectif track D>.                                         |
|       | Livraison proposee: <prochain increment>.                             |
|       | Decision/Action: <qui fait quoi, artefact, defaut>.                   |
+-------+------------------------------------------------------------------------+
| E     | Finalite: <objectif track E>.                                         |
|       | Livraison proposee: <prochain increment>.                             |
|       | Decision/Action: <qui fait quoi, artefact, defaut>.                   |
+-------+------------------------------------------------------------------------+
```

### Wrap-up apres tableaux

Apres les trois tableaux, ajouter un court wrap-up obligatoire. Il doit
dire comment l'agent va executer le prochain pas.

Format:

```text
Wrap-up:
- Mode: inline | agents paralleles | attente decision.
- Prochain lot execute: <track + lot + fichier de plan>.
- Pourquoi ce mode: <raison courte>.
- Action utilisateur: <aucune | decision | UAT exact>.
- Preuve visee: <tests, run ids, artefacts, commit>.
```

Regles:

- `inline`: a utiliser quand le prochain lot est serre, dans un seul
  ownership, ou quand la delegation n'a pas ete explicitement autorisee
- `agents paralleles`: a utiliser seulement si l'utilisateur ou le
  conductor autorise explicitement la delegation et si les ownerships
  sont disjoints
- `attente decision`: a utiliser seulement si avancer sans decision
  ferait perdre du temps ou risquerait de livrer le mauvais lot

## Attendus par track

### Track A - perf / optimisation

- preuve chiffrée avant/apres sur la charge concernee
- garde-fou qualite type SciFact si le hot path search bouge
- pas de merge declare acceptable sans signal de non-regression

### Track B - test automation / perf reporting

- rapport benchmark exploitable et rejouable
- schema de sortie comparable d'un run a l'autre
- verdict SLO clair

### Track C - ops / packaging / snapshots

- chemins snapshot / release verifies
- run ids CI ou K8s cites
- artefacts ou diagnostics accessibles

### Track D - matchID

- requirement tracee a une fixture ou un replay
- comparaison OpenSearch explicite quand la parite est revendiquee
- ecarts restants documentes

### Track E - infra K8s / poc-k8s

- workflows et jobs fail-closed
- logs / `kubectl describe` / artefacts preserves
- guardrails de cout et timeout respectes

## Regles perf et reporting

Quand un avancement touche perf, charge, bench, CI lourde ou K8s, le
rapport doit donner autant que possible:

- latence `p50 / p95 / p99 / max`
- debit d'ingestion ou duree d'indexation
- RSS peak et final
- `NDCG@10`
- `Recall@10`
- verdict `pass` / `fail`
- comparaison OpenSearch quand disponible

Preferences:

- charges lourdes: CI / K8s / `poc-k8s`
- local: smoke, boucle courte, reproduction
- artefacts conserves par `sha`
- publication `summary.md` ou equivalent si la chaine de report existe

## Regles git et integration

- la branche active est decidee par le Conductor
- ne jamais pousser sans demande explicite du user ou du Conductor
- les tracks A-D ont aujourd'hui des branches longues:
  `wp/a-optim`, `wp/b-test-auto`, `wp/c-ops`, `wp/d-matchid`
- Track E vit pour l'instant surtout sur `main`, `ci-k8s` et
  `deploy/k8s/`
- ne jamais utiliser de commande destructive sans ordre explicite

## Verification minimale avant de clore

Avant d'annoncer qu'un travail est fait:

1. relire le diff reel
2. lancer les tests utiles
3. verifier si fmt / clippy sont impactes
4. citer ce qui n'a pas pu etre verifie
5. donner le prochain pas concret
