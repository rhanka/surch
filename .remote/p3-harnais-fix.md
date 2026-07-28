# P3 — fermeture de la revue du harnais

Date : 2026-07-28. Portée : harnais local P3, sans workload ni commande Cargo
interdite par la mission.

## Bloquants

| Défaut | Statut | Correctif | Vérification |
|---|---|---|---|
| B1 | FERME | `p2_cgroup_io_json` sépare désormais après le premier objet (`first++`), ferme le tableau dans le même awk et fait relire la sortie compacte par `jq -ce` avant le `--argjson` consommateur. | Fixture `8:0 rbytes=1 wbytes=2 rios=3 wios=4` : quatre objets, `jq .` passe. |
| B2 | FERME | La clé de sélection, `used`, les buckets, le fallback, les validateurs d'unicité/intersection et l'exclusion de `MARTIN` passent par `tolower` du terme ASCII. Tout terme non ASCII est refusé fail-closed : aucune approximation silencieuse de `asciifolding`. Le protocole passe en v4, donc les entrées v3 sont refusées. | `Dupont`/`DUPONT` dans deux ensembles est rejeté ; `Martin` est rejeté comme terme fixe. |
| B3 | FERME | Le gate conserve `32 Mio` comme validité technique mais ajoute la cible P3 contractuelle `17 Mio = 17 825 792` octets sur les bornes avant/après des 18 phases C. | Fixture gate : `10 Mio` donne `PASS P3`; `20 Mio` donne `ÉCHEC P3` et un exit non nul. |
| B4 | FERME | Le gate relit les JSONL, calcule par triplet compaction `directory_bytes(C)/directory_bytes(B)`, récupérations RSS/RssAnon/cache et jemalloc resident, plus refaults, lectures et PSI de `match_control`. Il gate compaction `<= 0,0100`, médianes `>= 0,90`, aucune répétition `< 0,80`. | Fixture gate positive et contrôles de types/présence des dérivés. |
| B5 | FERME | C/B lit `size:10` et `size:0`, applique pour chacun médiane `<= 1,05` et aucune répétition `> 1,10`. | Fixture gate couvre les deux séries et publie les deux tableaux dans `campaign-summary.json`. |
| B6 | FERME | Le gate collecte le p95 client de `match_control`, impose médiane `<= 1,05` et aucune répétition `> 1,10`. | Fixture gate inclut la série client autonome et passe à `1,05`. |
| B7 | FERME | Les neuf observations de blocs B et les neuf observations C sont extraites et gatées séparément à `<= 0,25`. | Fixture gate exige et valide les deux populations. |
| B8 | FERME | Le script retourne zéro uniquement pour `PASS P3`; `ÉCHEC P3`, `INVALIDE P3` et `REJOUER P3` retournent non zéro. | Fixture à `20 Mio` : résumé `ÉCHEC P3`, code non nul. |
| B9 | FERME | A/B/C sont fixés aux trois SHA du plan ; toute surcharge divergente est refusée. Les métadonnées image sont de vrais objets JSON, la provenance de session les ancre et le gate vérifie SHA, image, image id et digest sur les trois répétitions de chaque variante. | Fixture gate avec neuf scorecards et provenance cohérente : `PASS P3`; toute identité divergente est refusée avant verdict. |

## Majeurs

| Défaut | Statut | Correctif | Vérification |
|---|---|---|---|
| M1 | FERME | Ordre rétabli : `C1-A1-B1`, `A2-B2-C2`, `B3-C3-A3`. Hard-stop après C1 sur intégrité/validité, puis après triplet 1 sur C/A, C/B (deux tailles), témoin et récupérations `< 80 %`. | Ordre et appels aux deux hard-stops relus ; ils écrivent les artefacts de présélection. |
| M2 | FERME | Le gate ouvre chaque JSONL, vérifie les 13/15 bornes, le schéma complet mémoire/cgroup/PSI/io, les deltas IO et P3 pour C ; les dérivés requis sont produits dans le résumé. | Fixture gate positive ; JSONL ou dérivé absent provoque un refus fail-closed. |
| M3 | FERME | Chaque scrape `/_prometheus_metrics` rafraîchit les jauges processus et jemalloc avant rendu. | Diff Rust relu ; `cargo fmt --check` passe. Pas de compilation locale, interdite. |
| M4 | FERME | `p2_metric_value` refuse syntaxe non numérique, `NaN` et infinis ; zéro numérique reste une valeur valide. | `test-p3-harness.sh` vérifie rejet de `NaN` et conservation de `0`. |
| M5 | FERME | Le steal CPU somme uniquement les lignes `/proc/stat` des CPU du `CPUSET` moteur, jamais la ligne agrégée. | Lecture statique de la fonction et analyse syntaxique Bash. |
| M6 | FERME | `recover_host` distingue une erreur `docker ps`/`docker volume ls` de l'absence normale du conteneur/volume ; toute erreur ferme le pilote. | Lecture statique et analyse syntaxique Bash. |
| M7 | FERME | Un témoin `< 0,95` donne `REJOUER P3`, non un simple échec performance ; le code est non nul. | Fixture `match_control=0,94` : verdict `REJOUER P3`, `replay_required=true`, code non nul. |

## Vérifications exécutées

- `bash -n` : `fair-ab.sh`, `p2-campaign.sh`, `p2-gate.sh`,
  `test-p3-harness.sh`, `p2-report.sh`.
- `bash deploy/bench-local/test-p3-harness.sh` : PASS.
- Fixture synthétique du gate : `PASS P3`, puis `ÉCHEC P3` à 20 Mio, puis
  `REJOUER P3` à 0,94.
- `cargo fmt --check` : PASS.
- `shellcheck` : indisponible dans l'environnement.

## Mineurs

La revue ne retenait aucun mineur autonome. Aucun n'est laissé.

## Recommandation

**NO-GO campagne pour l'instant.** Les défauts locaux sont fermés, mais le
smoke externe puis les neuf runs pré-engagés restent à exécuter sur la VM
dédiée ; ils sont les seules preuves recevables de performance, mémoire,
parité et provenance réelle.

FIX_DONE
