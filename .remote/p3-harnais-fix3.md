# P3 — correctif du harnais, tour 3

Ce document ne clôture aucun point avant le passage du commit dans le CI
Ubuntu 22.04. Les preuves locales ci-dessous sont rejouables, mais la règle de
probité exige aussi un artefact versionné **et** vert dans le CI qui compte.

## 1. Compatibilité jq 1.6 et CI verte

**Statut : PARTIEL.**

Correctif : `p2-gate.sh` ne fait plus appel à `abs`; la différence de sonde
est calculée par branchement numérique. Les lectures de chemin JSON qui
utilisaient `getpath` sont remplacées par `safe_path`, qui retourne `null` sur
un objet ou un chemin absent. L'audit des filtres P3 ne relève plus `abs`,
`have_literal_numbers`, `ltrimstr`, `getpath`, `$__loc__` ni `limit`.

Artefacts versionnés : `deploy/bench-local/p2-gate.sh`,
`deploy/bench-local/p2-campaign.sh`, `deploy/bench-local/test-p3-harness.sh`
et `.github/workflows/ci.yml`.

Preuve locale : la matrice exhaustive passe avec `jq-1.6` et avec le jq local.
Reste : un run GitHub Actions `harnais P3 synthétique` vert sur ce commit est
indispensable pour passer à **FERME**.

## 2. Identité physique des neuf runs

**Statut : PARTIEL.**

Correctif : le gate refuse les liens symboliques pour `runs`, les répertoires
de runs, les scorecards et les artefacts de paires. Il compare les chemins
canoniques et les identités physiques `device:inode`, exige neuf valeurs
distinctes, puis lie un UUID d'exécution distinct à la scorecard, aux JSONL de
statut/télémétrie, à `pair-summary.json` et à `parity.json`.

Artefacts versionnés : `deploy/bench-local/fair-ab.sh`,
`deploy/bench-local/p2-campaign.sh`, `deploy/bench-local/p2-report.sh`,
`deploy/bench-local/p2-gate.sh` et `deploy/bench-local/test-p3-harness.sh`.

Preuve locale : la matrice invalide séparément une duplication partielle, un
run manquant, un lien croisé de paire et un alias physique par symlink. Reste :
validation CI de ces négatifs.

## 3. Preuve de bout en bout

**Statut : PARTIEL.**

Correctif : la paire primaire A1/C1 de la fixture est maintenant générée par
`p2-report.sh` depuis vingt séries brutes par métrique; le gate relit son ratio
et son bootstrap. Les scorecards et les statuts contractuels sont relus pour
documents, segments, CPU/quota, steal, routage, intégrité, hash/fallbacks et
la sémantique de `verified_bytes` bool versus match.

Artefacts versionnés : `deploy/bench-local/p2-report.sh`,
`deploy/bench-local/p2-gate.sh` et `deploy/bench-local/test-p3-harness.sh`.

Preuve locale : l'assertion E2E contrôle les UUID, `resamples: 10000` et
`n: 20` dans `p3-primary-pairs/A1-C1/pair-summary.json`; les négatifs associés
donnent `INVALIDE P3`. Reste : CI verte.

## 4. Matrice exhaustive en CI

**Statut : PARTIEL.**

Correctif : le job obligatoire lance désormais
`P3_MATRIX_EXHAUSTIVE=1 bash deploy/bench-local/test-p3-harness.sh`. La matrice
exerce les valeurs inférieure, égale et supérieure aux bornes inclusives,
y compris blocs, intégrité, compaction, récupération, sonde et coûts C/B.

Artefacts versionnés : `.github/workflows/ci.yml` et
`deploy/bench-local/test-p3-harness.sh`.

Preuve locale : les commandes normale et exhaustive passent. Reste : le job
obligatoire GitHub Actions n'a pas encore produit son verdict sur ce commit.

## 5. Ratification M3, protocole mémoire réduit

**Statut : PARTIEL.**

Correctif : le protocole réduit est explicitement ratifié : seules
`directory_bytes`, RSS, RssAnon et cache fichier sont décisionnels. Les jauges
jemalloc restent diagnostiques; aucune fraîcheur ni conclusion sur resident,
fragmentation, retained ou dirty n'est revendiquée pour C. Le résumé de
campagne reprend cette portée.

Artefacts versionnés : `.remote/p3-campagne-plan.md`, `PLAN.md`,
`plan/p2-segmented-postings.md` et `deploy/bench-local/p2-gate.sh`.

Preuve locale : le plan et le JSON de résumé portent la réduction de portée.
Reste : CI verte avant fermeture de la documentation et du gate ensemble.

## 6. Conservation distincte du gain historique

**Statut : PARTIEL.**

Correctif : le PASS produit conserve sans changement `C/A p95 took size:10 <=
0,70`. Un verdict distinct `historical_gain` exige désormais médiane et borne
haute de chaque IC95 `<= 0,50`, sous le libellé **conservation du gain
historique**. Il produit `CONSERVE` ou `NON CONSERVE`, sans changer le verdict
produit.

Artefacts versionnés : `deploy/bench-local/p2-gate.sh`,
`deploy/bench-local/test-p3-harness.sh` et `.remote/p3-campagne-plan.md`.

Preuve locale : les cas exacts `0,50`/IC95 `0,50` donnent `CONSERVE`; le cas
`0,51` reste `PASS P3` avec `NON CONSERVE`. Reste : CI verte.

## 7. Oracle croisé du normaliseur

**Statut : PARTIEL.**

Correctif : la table AWK partagée par les validateurs P3 est extraite dans
`p2-asciifold.awk`. Le test d'intégration Rust exécute ce fichier avec awk et
compare ses sorties au `NormAnalyzer` réel sur accents, Æ/Œ, ß/ẞ, thorn,
lettres barrées et casse mixte; il vérifie aussi le domaine mono-token ASCII
admis. Aucune équivalence générale au-delà de ce jeu représentatif n'est
revendiquée.

Artefacts versionnés : `deploy/bench-local/p2-asciifold.awk`,
`deploy/bench-local/fair-ab.sh` et
`crates/surch-analysis/tests/p3_asciifold_oracle.rs`.

Preuve locale : la sortie AWK représentative est contrôlée; le test Rust n'a
pas été lancé localement, conformément à l'interdiction de `cargo test`. Il
sera exécuté par le job CI `cargo test`. Reste : ce job vert.

## Vérifications locales exécutées

- `bash -n` sur `fair-ab.sh`, `p2-report.sh`, `p2-gate.sh`,
  `p2-campaign.sh` et `test-p3-harness.sh` ;
- `bash deploy/bench-local/test-p3-harness.sh` ;
- `P3_MATRIX_EXHAUSTIVE=1 bash deploy/bench-local/test-p3-harness.sh` ;
- `PATH=/tmp/surch-jq16.bMJFkw:$PATH P3_MATRIX_EXHAUSTIVE=1 bash
  deploy/bench-local/test-p3-harness.sh` avec `jq-1.6` ;
- `cargo fmt --check`.

Aucun `cargo build`, `cargo check`, `cargo test` ou `cargo clippy` local n'a
été lancé.

## Verdict smoke

**NO-GO smoke** : les preuves sont versionnables et les gates locaux passent,
mais aucun run CI Ubuntu 22.04 vert sur le commit FIX3 n'existe encore. Le
prochain acte sûr est de pousser ce commit puis de conserver les liens et IDs
des jobs `harnais P3 synthétique` et `cargo test` avant de requalifier les sept
points.

FIX3_DONE
