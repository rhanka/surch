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
  ### Plan d'implémentation A1/A13 (cadré 2026-05-25)
  **État** : le parsing ET l'implémentation existent déjà, seule la glue
  manque.
  - Parsing mapping/settings : `crates/surch-index/src/mapping.rs`
    (`FieldMapping.fields` multi-field l.180-212 ; `AnalysisSettings`,
    `EdgeNgramTokenizerDefinition`, `AnalyzerDefinition`,
    `from_index_settings_value()` l.357-682). `token_chars` capturé mais
    pas appliqué (gap A13).
  - Analyzers prêts : `crates/surch-analysis/src/lib.rs` — `EdgeNgramAnalyzer`
    (l.323-371), `NormAnalyzer`, `Normalizer` fonctionnels.
  - Fan-out A10 : `document_index.rs::index_subfields()` (l.530-557) +
    `subfield_terms()` (l.645-668) — produit `.raw` (keyword+normalizer)
    aujourd'hui.
  **Glue manquante (3 points)** :
  1. Hook de résolution : `AnalyzerName::from_name()` (mapping.rs l.119-129)
     ne connaît que 6 builtins. Ajouter une résolution `nom custom →
     analysis().analyzers[nom] → tokenizer (edge_ngram → bornes min/max) +
     chaîne de filtres (lowercase/asciifolding)`.
  2. Index-time : `index_subfields()` / `analyzed_terms()` doivent router
     un sous-champ `type:text, analyzer:<custom>` vers l'analyzer résolu
     (génère les ngrams `NOM.autocomplete`).
  3. Query-time : `state.rs::normalized_terms_for_field()` (l.605-606).
  **⚠️ Subtilité parité-critique (non relevée par l'explo)** : pour
  edge_ngram, ES applique l'analyzer ngram **à l'indexation** mais un
  `search_analyzer` **distinct** (typiquement standard/keyword) **à la
  requête**. Si on edge-ngram les DEUX côtés, on diverge d'ES (faux
  positifs). Donc : honorer `search_analyzer` séparément de `analyzer`
  (parser le champ `search_analyzer` du mapping, défaut = `analyzer` pour
  les analyzers non-ngram). C'est la condition de parité `match NOM` == ES.
  **Sécurité régression** : `deces_v1` n'utilise aucun analyzer custom →
  b1-oracle reste 30/30 quelle que soit l'implémentation. La parité du
  NOUVEAU chemin edge_ngram n'est validable que via une fixture
  `deces_v2` + oracle ES (= Lot 7 / B2). Implémenter A1/A13 et B2 ensemble,
  ou A1/A13 d'abord (régression-safe) puis B2 pour valider.
  **Risque/blast radius** : matchID-critique ; mérite l'arbitrage user
  D-vs-F (voir handover.md) avant d'écrire la glue parité.
  ### Incréments validés CI (priorité D, en cours 2026-05-25)
  - [x] **Inc.1 — résolveur** : `AnalysisSettings::resolve_analyzer` →
    `ResolvedAnalyzer` (builtin ou edge_ngram + filtres), testé. Sans
    câblage (régression-safe).
  - [ ] **Inc.2 — modèle + parse + index** (le gros) : aujourd'hui le
    parseur de champ (`mapping.rs` ~l.803) **rejette** tout analyzer
    non-builtin (`UnsupportedAnalyzer`) → un mapping `deces_v2` avec
    `analyzer: autocomplete_analyzer` ne se charge PAS. Donc :
    1. Changer `FieldMapping.analyzer` (et `normalizer`, `search_analyzer`)
       pour porter soit un builtin soit un **nom custom** (ex. enum
       `{Builtin(AnalyzerName), Named(String)}`), et accepter les noms
       custom au parse (valider contre `settings.analysis.analyzer` quand
       dispo, sinon différer). Met à jour tous les appelants de
       `FieldMapping::analyzer() -> AnalyzerName`.
    2. Threader `&IndexMapping` (ou `&AnalysisSettings`) jusqu'à
       `index_subfields` → `subfield_terms` (l.530-668 de
       `document_index.rs`) — `add_documents_with_mapping_internal` a déjà
       le `&IndexMapping` complet (l.194), il suffit de le passer plus bas.
    3. `subfield_terms` : pour un sous-champ `text` à analyzer custom,
       résoudre via `resolve_analyzer` et fan-out les ngrams.
  - [x] **Inc.2 — modèle + parse + index** : `FieldMapping.custom_analyzer`,
    parseur tolérant aux noms custom, round-trip `_mapping`, fan-out ngrams
    à l'indexation. CI vert + tests.
  - [x] **Inc.3 — requête + search_analyzer** : `FieldMapping.search_analyzer`,
    `IndexMapping::custom_search_terms_for_field` (search_analyzer prioritaire),
    `normalized_terms_for_field` câblé. CI vert + tests.
  - [x] **Inc.4a — bout-en-bout via l'API** : le create d'index attache
    `settings.analysis` au mapping stocké (`merge_mapping_fields` propage
    désormais l'analysis — bug d'intégration trouvé via le test e2e qui
    donnait 0 hit). Test e2e `matchid_autocomplete.rs` : PUT deces2
    (edge_ngram + autocomplete_analyzer + search_analyzer standard) → bulk →
    `match NOM.autocomplete=dup` touche DUPONT, pas MARTIN ni zzz. **CI vert.**
    **A1/A13 fonctionnellement complet de bout en bout.**
  - **Régression confirmée en K8s** : b1-oracle deces_v1 reste 30/30, 0
    divergence après tous les changements A1/A13 (GHA `26427249349` @ `9d17f75`).
  - [~] **Inc.4b — fixture deces_v2 + oracle B2** : fixture
    `tests/matchid_compat/deces/mapping_v2.json` créée (NOM/PRENOMS avec
    sous-champs `.raw` keyword+norm et `.autocomplete` edge_ngram +
    `settings.analysis`) et validée e2e via Surch (test
    `deces_v2_fixture_autocomplete_and_raw_subfields`). Reste : oracle B2 K8s
    comparant Surch vs ES 8.6.1 (le e2e prouve la correction interne, pas la
    parité ES). **Cadrage : extension CONTENUE, sans changement de binaire** —
    `b1_oracle::bootstrap` PUT déjà le corps complet (settings+mappings) verbatim
    aux 2 moteurs (ES 8.6.1 reçoit `settings.analysis` edge_ngram), et
    `Request.expected` est `Option` → manifeste en comparaison LIVE Surch-vs-ES.
    Étapes restantes :
    - [x] manifeste `tests/matchid_compat/replays/deces_v2.json` (8 requêtes :
      autocomplete NOM/PRENOMS, .raw exact, bool, baseline norm, sort sur .raw).
    - [ ] Dockerfile : `COPY mapping_v2.json` + `replays/deces_v2.json` sous
      `/usr/local/share/deces/` + check `ls`.
    - [ ] `deploy/k8s/jobs/b2-oracle-gate.yaml` (clone de b1, `--mapping
      mapping_v2.json --replay deces_v2.json`).
    - [ ] `ci-k8s.yml` : ajouter `b2-oracle-gate` (choix `job`, contrôle image,
      REPORT_FILES, reconstruction logs).
    - [x] Dispatch 1er run (GHA `26427933905` @ `f62894b`) :
      **7/8 à parité**, 1 divergence. Rapport
      `2026-05-25-b2-oracle-deces-v2-ES861-K8s/`. À parité : autocomplete
      préfixe (2/3 chars), prefix accentué, `.raw` normalisé, baseline norm,
      sort sur `.raw`. **Le gate B2 est opérationnel** (ES accepte le mapping
      v2, comparaison live OK).
    - [x] **Divergence corrigée + parité certifiée** : fix `eeefcaf`
      (`FieldMapping::analyze_subfield_value` + `field_tokens_for_source`
      conscient des sous-champs + tokenisation requête `search_analyzer`),
      test de régression bool. Re-run B2 GHA `26428660584` : **8/8, 0
      divergence**. **A1/A13 certifié à parité Elasticsearch 8.6.1.**

**→ Lot 1 (A1/A13) COMPLET** : implémenté (résolveur, fan-out index, requête
search_analyzer, settings au create), validé fonctionnellement (e2e), régression
b1-oracle 30/30, parité ES 8.6.1 certifiée (b2-oracle 8/8). Prochain : A7.
- [~] Lot 2 — A7 runtime dates.
  - [x] **Inc.1** : `range_field_matches` conscient de `FieldType::Date` —
    parse valeur stockée + bornes en `NaiveDate` via le `format` (yyyyMMdd,
    epoch_millis, epoch_second) + date-math (`now`, `now-1y/d`, `now+2M`,
    `now-1w`), comparaison jour ; fallback lex/numérique inchangé pour les
    non-dates ; `count.rs` threadé. Tests unitaires (helpers, ancre fixe) +
    test e2e `date_range.rs` (bornes littérales + date-math sur un champ
    `type:date` propre). CI vert.
  - **Constat parité** : DATE_NAISSANCE reste `keyword` dans matchID (deces),
    car le slice INSEE contient des dates placeholder (`19530000`, mois/jour
    00) qu'ES `type:date` REJETTE au bulk. A7 sert donc le support date
    **général** OpenSearch (validé par e2e sur dates propres), pas la parité
    deces. Pour un futur champ date matchID avec placeholders il faudrait
    `ignore_malformed` (Surch est déjà lenient : date invalide → fallback lex).
  - [ ] Inc.2 (optionnel) : `ignore_malformed`, formats additionnels, plages
    epoch_millis numériques.
- [~] Lot 3 — A2 geo widening.
  - [x] **Inc.1 — geo_bounding_box** : variante `SearchQuery::GeoBoundingBox`,
    `parse_geo_bounding_box_query` (corners `top_left`/`bottom_right` via
    `parse_geo_point_source`), `geo_bounding_box_field_matches` (point-in-box
    inclusif), dispatch + bras `query_matches` ; filtre non-scorant (catch-alls
    scoring/stats). Test e2e `geo_bounding_box.rs`. Antiméridien hors scope.
  - [x] **Inc.2 — geo_polygon** : variante `SearchQuery::GeoPolygon`,
    `parse_geo_polygon_query` (≥3 points via `parse_geo_point_source`),
    `geo_polygon_field_matches` (ray-casting point-in-polygon), dispatch +
    bras `query_matches`. Test e2e (quad autour de Paris). **A2 complet**
    (geo_bounding_box + geo_polygon ; geo_distance préexistant).
- [~] Lot 4 — A5 scoring widening.
  - Déjà présents : `Weight`, `FieldValueFactor`, `GaussDecay` (date).
  - [x] **Inc.1 — exp/linear decay** : unifié en `ScoringFunction::Decay`
    + `DecayKind {Gauss,Exp,Linear}` (parse `parse_decay_function(kind)`,
    formules dans `evaluate_scoring_function` : exp = `exp(-dist·ln(1/decay)/
    scale)`, linear = `max(0, 1-dist·(1-decay)/scale)`). Tests e2e exp+linear
    (dates proches mieux classées). Messages d'erreur de parse restent
    génériques « gauss.<field> » (même grammaire). CI à valider.
  - [~] Inc.2 — **random_score** : RÉSERVE. La parité bit-à-bit avec ES est
    **infaisable** (RNG/hash interne ES différent) ; l'implémenter ferait
    diverger l'oracle, à l'encontre de l'objectif parité. matchID ne l'utilise
    pas. **Décision : implémenter une version non-parité (documentée) ou
    déclarer hors-scope ?** Par défaut : hors-scope.
  - [ ] **script_score — DÉCISION SCOPE** : nécessite un moteur d'évaluation
    d'expressions (mini-langage type painless). Sous-système conséquent.
    **Décision user : construire un évaluateur de script minimal, ou
    déclarer script_score hors-scope matchID ?** (matchID utilise surtout
    field_value_factor/decay/weight ; script_score est rare).
- [ ] Lot 5 — A6/A13 keyword-prefix (optionnel post-A10).
- [ ] Lot 6 — A12 composite date_histogram + histogram numérique.
- [ ] Lot 7 — B2 deces_v2 fixture + replay binaire + oracle gate
  promu.
