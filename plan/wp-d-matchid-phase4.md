# wp/d-matchid Phase 4 Plan

Track principal: D - matchID
Branch: `wp/d-matchid` (long branch; Phase 4 commits land directly on
`main`, branch kept for history).
Worktree: `.worktrees/wp-d`
Owner: conductor / SearchEngine / APIServer / StorageEngine depending
on slice
Status: ouvert. Phase 1-3 closes sur `main` : B1 oracle Elasticsearch
8.6.1 PASS (`ci-k8s` run `26192816780`, 30/30, 0 skipped,
0 divergence). Tous les axes ci-dessous sont listés comme `partial` ou
follow-up dans `docs/wp-d-matchid/gap-analysis.md` et explicitement
hors-scope dans `docs/wp-d-matchid/B1-phase-3-plan.md`.

## Finality

- [ ] Élargir la parité matchID au-delà des 30 requêtes B1: ajouter le
  replay deces_v2 INSEE sur slice 10k (B2) avec multi-field, dates
  formatées, geo et edge_ngram câblés.
- [ ] Tenir un oracle Elasticsearch 8.6.1 vert sur le replay v2 dans
  `ci-k8s`, sans rouvrir B1.

## Scope (axes Phase 4)

- [ ] A1/A2/A7/A13 multi-field widening — mapping `deces_v2`
  multi-field (`text` + sous-champs `keyword/raw` + `normalizer`),
  `date{format:yyyyMMdd}` runtime, `geo_point` étendu, edge_ngram.
- [ ] A2 — `geo_bounding_box` et `geo_polygon` (Surch retourne 400
  aujourd'hui ; ES retourne des hits).
- [ ] A5 — `linear` / `exp` decay, `script_score`, `random_score`.
- [ ] A6/A13 — keyword-prefix iterator pour `DATE_NAISSANCE`
  short-input (chemin postings dédié, pas le scan source).
- [ ] A7 — chrono parsing runtime + epoch_millis + math `now-1y/d`.
- [ ] A10 — write-time fan-out des sub-fields (stockage
  `parent.subname` avec analyzer/normalizer du sub-field, pour
  débloquer agg/composite/cardinality propres au champ).
- [ ] A12 — composite `date_histogram` source ; `histogram` numérique.
- [ ] B2 deces_v2 — fixture replay INSEE sur `slice-10000.ndjson.gz`
  avec mapping multi-field complet.

## Ordre proposé

1. **A10 write-time fan-out** (débloque toute la suite : sans
   stockage `.raw`/`.norm`, A12 composite + sort + agg restent en
   fallback) → 1 lot de refactor index/storage.
2. **A1/A13 multi-field + edge_ngram câblé côté indexation** (suit
   A10) → mapping `deces_v2` peut atterrir.
3. **A7 runtime dates** (`chrono`, `epoch_millis`, `now-…`) → permet
   les range dates en math, et les `date_histogram` correctes.
4. **A2 geo_bounding_box + geo_polygon** → étendu indépendant ;
   coverage tests `search_router_a2_*`.
5. **A5 linear/exp/script_score/random_score** → travail isolé
   `parse_scoring_function_clause` ; tests `search_router_a5_*`.
6. **A6/A13 keyword-prefix iterator** → côté postings; nécessite
   d'introduire un side-table prefix pour keyword (parallèle à
   l'existant texte).
7. **A12 composite date_histogram source + `histogram` numérique**
   → s'appuie sur A7 pour le date.
8. **B2 deces_v2 replay** → fixture + binaire `b1_oracle`
   réutilisable; promotion oracle gate sous nouveau nom
   `b2-oracle-gate.yaml`.

## Critères de finality par axe

- A10 : un `GET /:index/_mapping` round-trip avec `.raw` ; un `sort:
  NOM.raw` honore le normalizer sans fallback ; un `agg.cardinality`
  sur `.raw` n'utilise pas `lookup_sort_value` mais le storage.
- A1/A13 : `deces_v2` se charge sans erreur et `match NOM=…`
  parité avec ES 8.6.1.
- A7 : `range { gte: "now-1y/d" }` et `epoch_millis` parsés et
  exécutés ; round-trip mapping `format` non perdu.
- A2 : `geo_bounding_box` et `geo_polygon` couverts par tests +
  oracle ES.
- A5 : `linear`/`exp`/`script_score`/`random_score` exécutent et
  passent oracle ES sur fixtures dédiées.
- A6/A13 : un `prefix` sur keyword utilise une side-table dédiée
  (pas de source scan) ; benchmark micro p50 < 1 ms sur 10k docs.
- A12 : composite `date_histogram` source et `histogram` numérique
  parité ES.
- B2 deces_v2 : promotion `docs/ops/bench-reports/<date>-b2-oracle-ES861-K8s/`
  avec 0 divergence sur fixture étendue.

## Risk + ordering

- A10 est le seul refactor indexation de la phase. Tout le reste
  (A1/A2/A5/A7/A12) est code de query/parse + tests.
- A6/A13 keyword-prefix peut être différé si A1/A10 suffisent pour
  débloquer B2 — décider après A10 livré.
- B2 réutilise le binaire `b1_oracle` ; il faut une nouvelle `--replay`
  + manifest K8s frères (`b2-oracle-gate.yaml`) pour ne pas casser
  B1 (qui reste vert sur le replay v1).

## Out of scope Phase 4

- N'importe quelle fonctionnalité indexation/score qui ne sert pas
  matchID (ex: `script_fields`, `inner_hits`, `nested`).
- Mode cluster multi-node ; tout reste single-node.
- Cross-cluster search.

## Gates (à compléter au fil de l'eau)

- [x] Lot 0 — A10 write-time fan-out livré côté indexation
  (`DocumentIndex::index_subfields` / `subfield_terms` /
  `subfield_values` + `DocumentIndex::subfield_value`), tests
  `*subfield*` étendus (5 unitaires fan-out + 1 accounting mémoire).
- [x] Lot 0b — A10→A12 hinge : `sort`/`agg` côté query consomment le
  storage A10 (`AppState::subfield_projection` →
  `DocumentIndex::subfield_value`). `sort: NOM.raw` compare la valeur
  pré-analysée stockée (plus de normalize au read) ; `terms` /
  `cardinality` / `date_histogram` / `composite` sur un sous-champ
  stocké lisent le storage, plus `lookup_sort_value`. L'alias
  `_source` reste le fallback pour les chemins sans projection stockée
  (index sans mapping multi-field explicite). Tests
  `search_router_a10_phase4_*` (sort + cardinality + terms + alias
  fallback). Ferme le critère A12 « agg.cardinality sur .raw n'utilise
  pas lookup_sort_value mais le storage ».
  - [x] **Parité matchID A12 validée en K8s** : b1-oracle 30/30,
    0 divergence vs Elasticsearch 8.6.1 (GHA `26423292686` @ `9640169`,
    `2026-05-25-b1-oracle-A12-ES861-K8s/`). A10+A12 parité-neutres.
- [ ] Lot 1 — A1/A13 multi-field + edge_ngram câblé.
- [ ] Lot 2 — A7 runtime dates.
- [ ] Lot 3 — A2 geo widening.
- [ ] Lot 4 — A5 scoring widening.
- [ ] Lot 5 — A6/A13 keyword-prefix (optionnel post-A10).
- [ ] Lot 6 — A12 composite date_histogram + histogram numérique.
- [ ] Lot 7 — B2 deces_v2 fixture + replay binaire + oracle gate
  promu.
