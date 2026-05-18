# Directives de developpement agentique - Surch

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

## Format de retour subagent obligatoire

Tout retour terminal ou intermediaire doit utiliser exactement:

```md
## Fait
### Track A
- ...
### Track B
- ...
### Track C
- ...
### Track D
- ...
### Track E
- ...

## A faire
### Track A
- ...
### Track B
- ...
### Track C
- ...
### Track D
- ...
### Track E
- ...

## Attendus
### Track A
- ...
### Track B
- ...
### Track C
- ...
### Track D
- ...
### Track E
- ...
```

Regles de forme:

- exactement ces trois sections top-level
- couvrir Track A a Track E meme si c'est `RAS`
- pas de tableaux larges dans les statuts utilisateur
- preferer des listes courtes avec SHAs, run ids, chemins et verdicts
  inline
- definir les acronymes de benchmark au premier usage si le contexte
  ne les rend pas evidents

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
