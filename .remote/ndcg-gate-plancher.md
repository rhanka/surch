# Pose du plancher `ndcg-gate` (SciFact + TREC-COVID) — 2026-07-29

Périmètre strict respecté : seuls `deploy/k8s/jobs/ndcg-gate.yaml` et
`docs/ops/k8s-ci.md` (une ligne) ont été touchés. Aucun code Rust, aucun
`deploy/bench-local/`, aucun `beir-extra-ndcg.yaml`, aucun `ci-k8s.yml`.
Rien n'a été lancé sur le cluster.

## 1. Vérification des chiffres avant d'écrire quoi que ce soit

Relu intégralement `.remote/beir-diagnostic.md` et l'état actuel de
`deploy/k8s/jobs/ndcg-gate.yaml` avant tout changement.

- **Le gate n'avait effectivement aucun plancher.** Le driver exécutait
  `scifact-ndcg.sh` et `trec-covid-ndcg.sh` contre les deux moteurs,
  concaténait les sorties dans `summary.md`/`bench.json` (schema
  `surch.bench.ndcg_gate.v1`, un simple index de chemins de fichiers) et se
  terminait par une suite d'`echo` — aucune comparaison à un seuil nulle
  part, `exit 0` implicite garanti. Confirmé en lisant le fichier avant
  modification.
- **scifact = 0,6599** : confirmé §2 et §4 de `beir-diagnostic.md`, table de
  fidélité locale↔cluster — stable **au bit près sur trois mesures en
  pod**, datées 2026-06-15, 2026-06-30 et 2026-07-03. OpenSearch 2.17.1 =
  0,6537. **Surch gagne de +0,0062** (pas une perte).
- **trec-covid = 0,4777** : confirmé §4 de `beir-diagnostic.md` (« mesure en
  pod »), OpenSearch 2.17.1 = 0,4902, **Surch perd de 0,0125**. Nuance
  importante que je signale : contrairement à scifact, je n'ai trouvé
  qu'**une seule** mesure en pod documentée pour cette valeur (pas de
  triple répétition datée comme pour scifact). Le diagnostic la qualifie
  quand même de « stable et connue » et « écrivable immédiatement » (§7.2),
  mais je le note explicitement pour ne pas sur-représenter la confiance —
  c'est une valeur simple, pas triplement confirmée.
- Root-cause référencée et lue intégralement :
  `docs/paper/ndcg-trec-covid-rootcause-22.md` (#22, quantization Lucene
  SmallFloat du `doc_len`, arbitrage délibéré : gain sur scifact/trec-covid,
  coût sur nfcorpus/fiqa). Ce document pose en « garde-fou obligatoire »
  `TREC-COVID NDCG@10 >= 0,4902` — cible qui n'est PAS atteinte
  aujourd'hui (0,4777 < 0,4902). Je ne l'ai pas corrigé (hors périmètre,
  c'est un document d'analyse historique daté 2026-06-08, antérieur à
  l'implémentation du fix qu'il proposait) mais je signale la tension :
  quelqu'un devra un jour amender ou annoter ce document pour refléter que
  cette cible a été sciemment non tenue en échange du gain scifact — ce
  n'est pas fait ici, volontairement, pour rester dans le périmètre.

Les prémisses de la tâche sont donc confirmées : chiffres exacts, gate
sans plancher. J'ai codé dessus sans ajustement de fond.

## 2. Planchers posés, méthode reprise de `beir-extra-ndcg.yaml`

Nouvelles variables d'environnement du conteneur `ndcg-driver` :

```
BEIR_BASELINE_DATE = 2026-07-29
BEIR_BASELINE_RUN  = 30451966000
SCIFACT_NDCG_FLOOR     = 0.6599
TREC_COVID_NDCG_FLOOR  = 0.4777
SCIFACT_MAX_OS_GAP     = 0
TREC_COVID_MAX_OS_GAP  = 0.0130
```

- **Planchers absolus (`*_NDCG_FLOOR`) : la valeur observée exacte, sans
  marge.** Toute baisse de 1e-4 fait échouer — vérifié par test (scénario
  B et C ci-dessous).
- **`SCIFACT_MAX_OS_GAP = 0`** : puisque Surch **gagne** sur scifact
  (+0,0062), le critère devient « Surch ne doit pas repasser derrière
  OpenSearch ». Pas besoin de marge IEEE754 ici : l'écart réel
  (`0.6599-0.6537` = `0,00620000000000009432` en double, vérifié) est à des
  ordres de grandeur du bruit de représentation (~1e-16) et de la borne 0.
- **`TREC_COVID_MAX_OS_GAP = 0.0130`** : l'écart brut observé est 0,0125.
  Piège IEEE754 vérifié explicitement, **y compris dans l'image
  bench-driver réelle** (`debian:bookworm-slim`, `mawk`, pas seulement en
  local) :
  ```
  awk 'BEGIN { printf "%.20f", 0.4777-0.4902 }'
  -> -0.01250000000000001110
  ```
  Un plafond fixé À EXACTEMENT `0.0125` échoue donc sur le bruit de
  représentation (confirmé : `rc=1` avec `maximum_gap=0.0125`, `rc=0` avec
  `0.0130`, testé dans le conteneur `debian:bookworm-slim` lui-même). C'est
  exactement le même piège que celui déjà rencontré sur nfcorpus/fiqa
  (`0.3021-0.3034` = `-0.0013000000000000123`). Le cran retenu (+0,0005)
  suit le même motif que celui utilisé pour fiqa dans
  `beir-extra-ndcg.yaml` (0,0115 → 0,0120). C'est écrit en commentaire
  dans l'en-tête du manifeste, avec le calcul exact.

Toute la provenance (date, run, table des deltas SmallFloat, référence à
`beir-diagnostic.md` et à `ndcg-trec-covid-rootcause-22.md`, calcul IEEE754
littéral) est en en-tête du manifeste, avant `apiVersion`.

## 3. Correction de fond du driver

Reprise fidèle de la méthode appliquée à `beir-extra-ndcg.yaml` :

1. Les quatre mesures (scifact×2, trec-covid×2) sont lancées via une
   fonction `measure()` qui **n'interrompt plus** le driver sur un échec :
   elle enregistre `FAILURES+1` et continue. `set -eu` reste en tête (comme
   dans le modèle), mais l'échec est capté par un `if ! ...; then` — le
   piège maison « `set -e` ne s'applique pas dans un `if` » joue ici en
   notre faveur, pas contre nous.
2. `summary.md` est assemblé et imprimé entre les marqueurs
   `BEGIN_SURCH_K8S_SUMMARY` / `END_SURCH_K8S_SUMMARY` **avant** tout appel
   à `check_dataset`/`gate_fail` — donc avant toute décision de gate.
3. Les **deux jeux sont systématiquement évalués** (`check_dataset scifact
   ...` puis `check_dataset trec-covid ...`), chacun accumulant ses échecs
   propres dans `FAILURES` global — un plancher raté sur scifact ne masque
   plus le verdict de trec-covid (et réciproquement), vérifié par test.
4. `bench.json` passe à `surch.bench.ndcg_gate.v2` (le fichier n'avait
   jusqu'ici que des chemins de fichiers, `v1` ; je n'ai pas réutilisé le
   nom `v3` de `beir-extra-ndcg.yaml` pour éviter de laisser croire que
   les deux Jobs partagent un schéma — ce sont deux manifestes distincts).
   Valeurs observées sérialisées en **chaînes** (une mesure ratée donne
   `""`, pas un JSON cassé). Un champ machine-lisible
   `opensearch_gap_is_constat_not_cible: true` sur `trec_covid` encode le
   traitement honnête (point 4 ci-dessous) au-delà du seul commentaire.
5. L'échantillonnage RSS (spécifique à ce Job, absent du modèle
   `beir-extra-ndcg.yaml`) est conservé à l'identique — démarré avant les
   mesures, attendu (`wait_rss_sample`) après l'assemblage du résumé,
   sans toucher à sa logique.

**Écart assumé par rapport à la réplique exacte du modèle** : le modèle
(`beir-extra-ndcg.yaml`) délègue à `scripts/bench/beir-ndcg.sh`, qui valide
en interne le nombre de documents et de qids attendus
(`BEIR_EXPECTED_DOCS`/`BEIR_EXPECTED_TEST_QIDS`). Les scripts utilisés ici
— `scifact-ndcg.sh` et `trec-covid-ndcg.sh` — n'exposent pas ces variables
(vérifié en lisant les deux fichiers) ; les modifier est hors périmètre
strict (seul `ndcg-gate.yaml` et sa documentation sont autorisés). Le
driver ne peut donc pas imposer cette validation de cardinalité au niveau
des scripts. Le filet de sécurité générique — `assert_all_queries_processed`
(nombre de requêtes traitées == total annoncé) — est repris à l'identique
et couvre la même famille de risque (« une mesure absente doit échouer,
jamais passer »), vérifié par le scénario G ci-dessous. C'est une
différence honnête à signaler, pas une régression cachée.

## 4. Traitement honnête de TREC-COVID

- Le plancher `TREC_COVID_NDCG_FLOOR = 0.4777` verrouille la **non-
  régression de Surch**, rien d'autre.
- L'écart à OpenSearch (0,0125, plafonné à 0,0130 pour la seule raison
  numérique ci-dessus) reste un **constat**, jamais présenté comme une
  cible atteinte :
  - dans l'en-tête du manifeste (section dédiée, en toutes lettres) ;
  - dans `summary.md` imprimé par le Job (« l'écart à OpenSearch est un
    CONSTAT documenté ... pas une cible de parité atteinte ») ;
  - dans `bench.json` (`opensearch_gap_is_constat_not_cible: true` +
    `rootcause_doc` pointant `docs/paper/ndcg-trec-covid-rootcause-22.md`).
- Référence explicite et lue à `docs/paper/ndcg-trec-covid-rootcause-22.md`
  (#22) partout où l'écart est mentionné.

## 5. Vérifications faites (ÉCRIT vs VÉRIFIÉ)

Conformément à la consigne de probité, rien n'a tourné sur le cluster.
Ce qui suit a été **vérifié**, avec artefacts reproductibles :

- **Syntaxe YAML** : `envsubst '${SURCH_SHA}'` (substitution identique à
  celle du workflow) puis `kubectl create --dry-run=client -f` sur le
  manifeste rendu → `job.batch/ndcg-gate created (dry run)`, aucune erreur.
- **Logique du driver, hors cluster, sous le `/bin/sh` et le `awk` RÉELS de
  l'image `bench-driver`** (`debian:bookworm-slim` — confirmé
  `/bin/sh -> dash`, `awk -> mawk`, identiques à mon shell local qui a
  servi de premier passage) : le corps du script driver a été extrait du
  manifeste rendu via `kubectl create --dry-run=client -o jsonpath`, les
  deux scripts de mesure remplacés par des stubs simulant des sorties
  BEIR conformes au format réel (mêmes lignes `queries_processed=...` /
  `NDCG@10 = ...` que produisent `scifact-ndcg.sh`/`trec-covid-ndcg.sh`),
  et exécuté dans un conteneur `debian:bookworm-slim` monté sur le script
  ainsi que localement. Sept scénarios :

  | scénario | attendu | obtenu |
  |---|---|---|
  | A — valeurs réelles 0,6599/0,6537 + 0,4777/0,4902 | PASS, exit 0 | PASS, exit 0 (confirmé aussi dans le conteneur réel) |
  | B — scifact à 0,6598 (−1e-4) | scifact FAIL, trec-covid quand même évalué | scifact FAIL + trec-covid PASS, exit 1 |
  | C — trec-covid à 0,4776 (−1e-4) | trec-covid FAIL, scifact quand même évalué | trec-covid FAIL + scifact PASS, exit 1 |
  | D — écart OpenSearch trec-covid élargi (OS 0,4920, écart −0,0143 > 0,0130) | FAIL par le critère d'écart, pas par le plancher | trec-covid FAIL (`gap ... exceeds 0.0130`), exit 1 |
  | E — mesure scifact/surch en échec (script renvoie non-zéro) | FAIL, mais les 3 autres mesures restent dans le journal/summary | exit 1, summary.md affiche « sortie absente ou vide » seulement pour scifact/surch, les 3 autres sorties intactes |
  | F — les deux jeux en régression simultanée | les deux rapportés, aucun n'est masqué | 2 lignes `BEIR_GATE dataset=... status=FAIL`, `failures=2`, exit 1 |
  | G — décompte de requêtes incohérent (299 traitées sur 300 annoncées) | FAIL explicite, pas un vert silencieux | `incomplete or malformed query count`, exit 1 |

  Dans les sept cas, `bench.json` reste un JSON valide (vérifié via
  `jq -e .` sur chacun des sept fichiers produits), y compris quand une
  valeur est absente (`""`).
- **Piège IEEE754 sur le seuil trec-covid** : vérifié par calcul direct
  dans le conteneur `debian:bookworm-slim` (`mawk`) — `0.4777-0.4902` vaut
  `-0.01250000000000001110`, un plafond à `0.0125` échoue (`rc=1`), à
  `0.0130` passe (`rc=0`). C'est la preuve, pas une supposition.

Ce qui **reste à valider sur le cluster réel** (non fait, volontairement,
campagne en cours) :
- que `scifact-ndcg.sh`/`trec-covid-ndcg.sh` produisent bien, en pod,
  les valeurs 0,6599/0,4777 sur l'image `sha-<HEAD>` actuelle (le
  diagnostic les a mesurées sur `188906f4` ; HEAD a changé depuis) ;
- le comportement réel de `wait_rss_sample`/`start_rss_sample` avec de
  vrais PID `surch-api`/`java` sous `shareProcessNamespace` (non
  simulable hors cluster, mais cette partie n'a pas été modifiée) ;
- que le Job complet tienne dans `activeDeadlineSeconds: 3600` avec la
  logique supplémentaire (négligeable en coût : quelques appels `awk`
  et `printf` de plus).

## 6. Commande à lancer (après la campagne en cours)

Une fois la fenêtre de mesure libérée et les images `sha-<HEAD>` /
`bench-sha-<HEAD>` publiées pour le commit de ce lot :

```bash
gh workflow run ci-k8s.yml --ref main -f job=ndcg-gate
```

Lecture attendue du journal `ndcg-driver` : bloc `BEGIN_SURCH_K8S_SUMMARY`
avec les quatre mesures présentes quel que soit le verdict, puis les deux
lignes `BEIR_GATE dataset=scifact status=...` / `dataset=trec-covid
status=...`, puis `BEIR_GATE status=PASS failures=0` (si les valeurs
tiennent) ou `status=FAIL failures=N` sinon — jamais un exit silencieux.

## 7. Fichiers touchés

- `deploy/k8s/jobs/ndcg-gate.yaml` — planchers, driver corrigé, en-tête de
  provenance.
- `docs/ops/k8s-ci.md` — une ligne mise à jour (description du fichier
  dans « Files in this repo »), pour ne pas laisser une description
  stale (« SciFact NDCG@10 parity gate » sans mention de TREC-COVID ni
  des planchers).
- `.remote/ndcg-gate-plancher.md` — ce rapport.

## 8. Notes annexes (hors périmètre, signalées pour information)

- `docs/ops/beir-extra-ndcg-gate.md` documente encore l'ANCIENNE baseline
  de `beir-extra-ndcg` (nfcorpus 0,3033 / fiqa 0,2294, run `26476471207`)
  alors que le manifeste `beir-extra-ndcg.yaml` a déjà été re-étalonné à
  0,3021/0,2274. Ce fichier est hors de mon périmètre strict (documentation
  d'un autre Job) — signalé mais non modifié.
- `docs/paper/ndcg-trec-covid-rootcause-22.md` pose encore
  `TREC-COVID NDCG@10 >= 0,4902` comme « garde-fou obligatoire », cible que
  la valeur actuelle (0,4777) ne tient pas. Signalé au point 1, non modifié
  (document d'analyse historique, correction de fond hors périmètre de ce
  lot).

NDCG_PLANCHER_DONE
