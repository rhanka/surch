# P3 — correctif 2 du harnais

Base revue : `070e862`. Le contrat de seuils de
`.remote/p3-campagne-plan.md` est conservé sans assouplissement. Le pin C
reste `d0accd6e4809bc7340a6cd55cef0a94fcb6c062d` : il n'est pas repinné sur
HEAD dans ce correctif.

## Sept corrections exigées

### 1. N1 — bijection des neuf runs : FERME

`p2-gate.sh` impose désormais exactement les répertoires et scorecards
`A1..A3`, `B1..B3`, `C1..C3`, sans répertoire additionnel. Chaque groupe de
paires doit contenir exactement les trois noms contractuels. Pour chaque
paire, le gate lie le nom du répertoire, `parity.json`,
`pair-summary.json.a_dir/b_dir`, les scorecards et les empreintes du manifeste
d'entrée. Les trois contrastes sont donc exactement A/B, A/C et B/C pour les
mêmes triplets numérotés.

Preuve versionnée et rejouable :
`deploy/bench-local/test-p3-harness.sh` construit neuf scorecards synthétiques
distinctes et le cas `n1-runs-dupliques`, où les trois familles réemploient
A1/B1/C1. L'exécution du gate réel exige alors `INVALIDE P3` et le code `1`.
Toute incohérence structurelle écrit aussi un résumé `INVALIDE P3`, jamais un
PASS.

### 2. M3 — fraîcheur des jauges dans les images mesurées : FERME

Choix explicite : pas de repin de C. La revendication de fraîcheur jemalloc a
été retirée du plan et du gate de la campagne contractuelle : les valeurs
jemalloc de C ne sont plus utilisées dans un dérivé ni une gate de récupération
fraîche. Elles restent des diagnostics bruts, sans conclusion de fraîcheur.

Dans HEAD, `crates/surch-api/src/stats.rs` rend tout échec de lecture
`/proc/self/status`, d'`epoch::advance` ou de lecture jemalloc observable par
`surch_runtime_memory_refresh_success`,
`surch_runtime_memory_refresh_age_seconds` et
`surch_runtime_memory_refresh_errors_total`. Une ancienne valeur ne peut donc
pas se faire passer pour fraîche. Cette télémétrie ne rétroagit pas sur C
épinglé.

Preuve versionnée : `deploy/bench-local/p2-gate.sh` ne publie plus de dérivé
`jemalloc_resident`; `plan/p2-segmented-postings.md` interdit explicitement
toute revendication ou repin sans protocole ultérieur. Le code Rust est soumis
au job CI de formatage, Clippy et tests après push ; aucun test Cargo local n'a
été lancé conformément à la contrainte de mission.

### 3. N6 et M2 — schéma JSONL P3 complet : FERME

Le validateur indépendant exige désormais les douze champs obligatoires de
`metrics.p3_integrity` : `bytes`, `pages`, `verified_bytes`,
`hash_failures`, `fallbacks`, `fallback_fields`, `term_occurrences`, `blocks`,
`fields`, `term_payload_bytes`, `csr_bytes` et `directory_bytes`.

Preuve versionnée et rejouable : la matrice retire chacun des douze champs,
un à un, de toutes les télémétries C et vérifie le verdict `INVALIDE P3`, code
`1`, du gate réel.

### 4. N3, N4 et N5 — smoke protecteur : FERME

`p2-campaign.sh` est maintenant en `set -euo pipefail`. Un full refuse un
smoke sans preuve v4 : protocole, trois SHA, identités image/digest,
manifeste d'entrée commun et empreinte, trois scorecards `smoke-A/B/C`, et
verdict README exact sont tous relus. Le smoke exécute aussi une fixture
déterministe des formules de compaction et de récupération, y compris le refus
d'un dénominateur nul. L'écriture de la preuve, du README et sa relecture sont
gardées ; une erreur disque retourne donc non zéro.

Preuve versionnée : `verify_smoke_prerequisite`,
`p3_smoke_formula_fixture` et `write_smoke_proof` dans
`deploy/bench-local/p2-campaign.sh`. Elles sont relues au démarrage du full et
immédiatement après la production du smoke. Le smoke complet reste une
exécution Docker externe : aucun faux artefact de smoke n'est déclaré ici.

### 5. N2 — biais ASCII : FERME

Le sélecteur et ses validateurs appliquent une table AWK déterministe couvrant
Latin-1 Supplement et Latin Extended-A, puis `tolower`, comme le mapping du
moteur. Les expansions `Æ/Œ/ß/Þ`, les lettres barrées et les diacritiques sont
traités ; les caractères que le NFD moteur laisse non ASCII restent exclus du
chemin mono-token. Un `PRENOMS` n'est exigé mono-token que pour bool et
chauffe ; le témoin `match NOM` ne le contraint plus.

Preuve versionnée et rejouable : le test accepte `Évrard / Noël` et
`Þór / Eð`, refuse la collision analysée `Évrard`/`Evrard`, et exécute les
validateurs AWK réellement utilisés par le constructeur.

### 6. N8 — matrice versionnée du gate : FERME

`test-p3-harness.sh` exécute désormais `p2-gate.sh`, non plus seulement des
helpers de sérialisation. Sa matrice synthétique porte neuf runs distincts,
les trois familles de paires et tous les seuils contractuels : côtés acceptés
et rouges des bornes, intégrité cible et plafond technique, compaction,
récupérations, témoins, sonde, blocs et coûts C/B. Elle vérifie les quatre
verdicts et leurs codes : `PASS P3`/0, `ÉCHEC P3`/1,
`INVALIDE P3`/1 et `REJOUER P3`/1.

Preuve versionnée et rejouable : le job CI `harnais P3 synthétique` de
`.github/workflows/ci.yml` lance cette matrice. La commande locale autorisée
`bash deploy/bench-local/test-p3-harness.sh` est verte dans ce tour.

### 7. N7 — coût du refresh : FERME

Avant tout repin de C, HEAD borne le refresh runtime à une tentative par
seconde. Le verrou est non bloquant (`try_lock`) : un scrape concurrent ne
fait ni attente ni lecture `/proc` ni `mallctl`; il sert la dernière
publication avec son âge. Les échecs sont comptés et positionnent la jauge de
succès à zéro. Il n'y a pas de repin de C dans ce commit.

Preuve versionnée : `RUNTIME_MEMORY_REFRESH_MAX_AGE`, le cache `OnceLock`
et les jauges de succès/âge/erreur dans `crates/surch-api/src/stats.rs` ; le
plan interdit un futur repin sans preuve externe de coût de scrapes
concurrents.

## Statut des seize défauts initiaux après ce tour

| ID | Statut | Preuve actuelle |
|---|---|---|
| B1 | FERME | Sérialisation `io.stat` et test synthétique versionné. |
| B2 | FERME | Asciifolding AWK, disjonction analysée et cas accentués. |
| B3 | FERME | Bijection N1, cible 17 Mio et plafond 32 Mio dans le gate. |
| B4 | FERME | Bijection, compaction et récupérations RSS/RssAnon/fichier ; aucune fraîcheur jemalloc revendiquée. |
| B5 | FERME | Matrice C/B `size:10` et `size:0`, médiane et répétitions. |
| B6 | FERME | Matrice témoin `took`/client, bornes hautes et basses. |
| B7 | FERME | Neuf observations issues des trois runs distincts, B et C. |
| B8 | FERME | Gate non zéro hors PASS et matrice des codes de sortie. |
| B9 | FERME | SHA, image, digest et manifeste liés aux neuf scorecards. |
| M1 | FERME | Ordre latin et hard-stops déjà versionnés, non régressés. |
| M2 | FERME | Schéma JSONL complet et douze suppressions invalidantes. |
| M3 | FERME | Claim retirée pour C épinglé ; erreurs HEAD observables. |
| M4 | FERME | Rejet NaN et préservation du zéro dans le test versionné. |
| M5 | FERME | Calcul CPUSet déjà versionné, non régressé. |
| M6 | FERME | Échecs Docker fail-closed déjà versionnés, non régressés. |
| M7 | FERME | Bijection et matrice `REJOUER P3` à 0,94. |

## Vérifications de ce tour

- `bash -n deploy/bench-local/fair-ab.sh deploy/bench-local/p2-campaign.sh deploy/bench-local/p2-gate.sh deploy/bench-local/test-p3-harness.sh`
- `bash deploy/bench-local/test-p3-harness.sh`
- `cargo fmt --all -- --check`
- `git diff --check`

Aucun build, check, test ou Clippy Cargo local ; aucun Docker, smoke réel,
campagne complète ni workload n'a été lancé.

## Recommandation

**NO-GO smoke officiel, full et publication à cet instant.** Le correctif est
versionné et sa matrice locale est verte, mais il faut d'abord le commit puis
un CI vert incluant le job `harnais P3 synthétique`. Après cette preuve, un
smoke v4 neuf pourra être lancé ; le full reste interdit tant que ce smoke ne
porte pas sa preuve rejouable. Aucune mesure P3 ni fraîcheur jemalloc de C
n'est publiée par ce tour.

FIX2_DONE
