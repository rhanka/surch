# Fix4 — pilote P3 fail-closed après l'échec du smoke v4

## Verdict local

Le smoke v4 à e680bc9 est correctement classé **NO-GO** : il n'a créé ni
image de session, ni manifeste, ni scorecard, ni run A/B/C. La régression
était la déclaration groupée de `build_image` sous `set -u`.

Ce lot ne ferme **pas** la campagne P3 ni la gate externe. Il apporte un
correctif versionné et une preuve locale rejouable; la CI du nouveau commit
n'existe pas encore au moment de ce rapport.

## Correction

`deploy/bench-local/p2-campaign.sh` sépare maintenant chaque variable
locale en commande distincte. La correction explicite est :

```bash
local variant="$1"
local sha="$2"
local image="surch-p2-${variant,,}:${sha:0:12}"
```

Le même traitement a été appliqué à toutes les déclarations multiples du
périmètre demandé. Cinq lignes du pilote avaient une dépendance de droite à
gauche qui pouvait devenir fatale sous `set -u` :

- e680bc9:173 : `image` référençait `variant` et `sha`;
- e680bc9:233 : `score` référençait `out`;
- e680bc9:274 : `out` référençait `name`;
- e680bc9:300 : `report` référençait `pair` et `report_group`;
- e680bc9:353 : `state` référençait `name`.

Le nouveau test a aussi exécuté le hard-stop full et révélé une seconde
défaillance réelle : `mawk` refuse `-v match=...` car `match` est un
mot-clé. La variable shell/awk est désormais `match_ratio`.

La relecture `set -u` a révélé un troisième défaut, distinct du motif de
déclaration : sous Bash 4.3, `replay_input=()` suivi de
`"${replay_input[@]}"` échoue quand `P2_REPLAY_MIX_5050=0`, qui est le
chemin par défaut. `fair-ab.sh` construit maintenant `awk_inputs` avec les
sept fichiers obligatoires, puis n'ajoute le replay que lorsque le mix vaut
1. Le tableau développé par AWK est donc toujours non vide. Cette correction
évite à la fois `nounset` et l'argument-fichier vide que produirait
`"${replay_input[@]:-}"`.

## Audit exhaustif des déclarations

Audit effectué sur le contenu de e680bc9, avant correction. Le balayage
lexical mono-ligne a produit 156 candidats; une recherche dédiée n'a trouvé
aucune déclaration continuée par `\\` dans les cinq scripts. Six ne sont pas
des déclarations multiples :
une déclaration unique était suivie d'une commande après un point-virgule
(`p2-gate.sh:201`; `fair-ab.sh:2001,2126,2127,2572,2573`). Elles ont été
relues et reformattées sans changer leur ordre d'exécution. Il reste donc
150 vraies déclarations multiples, toutes séparées dans l'état final.

Les lignes suivantes constituent la liste exhaustive des candidats, par
fichier et ligne e680bc9. Hormis les cinq dépendances ci-dessus, les
initialisations sont indépendantes : elles étaient inoffensives à cet instant,
mais ont été séparées sans exception afin d'empêcher une dépendance future
latente.

```text
deploy/bench-local/p2-campaign.sh: 43, 129, 142, 147, 164, 173, 233, 274, 300, 301, 328, 353, 391, 407, 420, 438, 453, 479, 503
deploy/bench-local/fair-ab.sh: 374, 384, 398, 419, 420, 423, 493, 498, 532, 914, 915, 981, 996, 1012, 1051, 1104, 1197, 1209, 1214, 1230, 1252, 1265, 1275, 1288, 1316, 1379, 1387, 1418, 1419, 1420, 1421, 1470, 1491, 1492, 1493, 1494, 1495, 1496, 1497, 1498, 1499, 1617, 1624, 1629, 1679, 1698, 1699, 1713, 1714, 1740, 1763, 1784, 1798, 1804, 1836, 1837, 1871, 1872, 1873, 1983, 1989, 2001, 2033, 2066, 2126, 2127, 2130, 2163, 2177, 2180, 2206, 2247, 2248, 2249, 2250, 2251, 2252, 2253, 2254, 2313, 2347, 2390, 2436, 2453, 2454, 2455, 2456, 2457, 2465, 2466, 2572, 2573, 2590, 2602, 2648, 2651, 2652, 2653, 2654, 2655, 2656
deploy/bench-local/p2-gate.sh: 60, 80, 124, 201, 202, 203, 250, 320, 496, 509, 529, 614
deploy/bench-local/p2-report.sh: 118, 143, 164, 225, 237
deploy/bench-local/test-p3-harness.sh: 86, 87, 147, 167, 190, 203, 213, 221, 228, 242, 250, 256, 264, 274, 288, 302, 310, 318, 388
```

Aucun `readonly` multiple ni `declare` multiple n'a été trouvé. Les
`declare -A` existants déclarent chacun un seul tableau associatif.

## Nouvelle couverture du pilote

`deploy/bench-local/test-p3-campaign.sh` exécute le vrai
`p2-campaign.sh`, donc son vrai `set -euo pipefail`. Il injecte dans
`PATH` un faux Docker et remplace seulement les frontières coûteuses
`fair-ab.sh`, `p2-report.sh` et le gate par des doubles déterministes.
Avant tout fake, il refuse aussi par AWK toute déclaration groupée mono-ligne
dans les cinq scripts audités. C'est un garde-fou lexical, non un parseur
Bash : la revue de l'état présent vérifie en plus l'absence de déclaration
continuée par `\\`.

Le test atteint réellement :

- construction A/B/C, provenance et inspection d'images;
- smoke A → B → C, scorecards, manifestes, parités, récupérations et
  `smoke-proof.json`;
- full avec ordre latin C1 → A1 → B1, puis A2 → B2 → C2,
  B3 → C3 → A3;
- neuf scorecards, trois familles de rapports, hard-stop C1 et hard-stop du
  premier triplet;
- propagation d'un échec de `fair-ab` : B arrête le pilote, C n'est pas
  lancé et aucune preuve smoke n'est écrite;
- propagation d'un échec de `p2-report` : le premier contraste arrête le
  smoke, les deux suivants et la preuve ne sont pas écrits;
- hard-stop C1 rouge : arrêt avant A1/B1, donc avant leur coût;
- hard-stop du premier triplet rouge : arrêt avant A2;
- propagation d'un gate final rouge : les neuf runs ont eu lieu, mais le
  pilote sort en erreur et ne publie pas de `README.md`.

La fonction réelle `p2_validate_body_files` de `fair-ab.sh` est aussi
extraite et exécutée sous le `set -u` du test avec le replay optionnel non
ajouté. Une garde AWK versionnée impose la construction `awk_inputs` non
vide et interdit le vieux `replay_input`; elle fait échouer la régression si
le motif Bash 4.3 réapparaît, même sur un hôte Bash plus récent.

La reversion temporaire de la seule ligne fautive a fait échouer ce test avant
la première image avec `variant: unbound variable`; la correction a ensuite
été immédiatement restaurée.

Le test n'atteint volontairement pas un vrai build Docker, systemd/cgroup,
drop_caches, l'ingestion 1,36 M/28,9 M, ni les calculs réels de
`fair-ab.sh`, `p2-report.sh` ou `p2-gate.sh` depuis le pilote. La matrice
existante continue d'exécuter le vrai gate et le vrai rapport synthétique;
le smoke VM reste nécessaire pour les frontières système et le workload.

## Audit complémentaire set -u

Les paramètres optionnels relus utilisent des valeurs par défaut explicites
(`VAR:-`) avant lecture. Les fonctions à arguments positionnels du pilote,
du gate, du rapport et des helpers sont appelées avec leur arité contractuelle.
Audit exhaustif des tableaux et variadiques dans les cinq scripts demandés :

- `p2-campaign.sh` : `phases` a toujours six entrées et `SCHEDULE` trois
  (full) ou une (smoke) ; `P2_RUN_EXECUTION_IDS` n'est jamais développé par
  `[@]`, ses lectures utilisent `:-`.
- `fair-ab.sh` : `replay_input` était le seul tableau vide fatal et est
  remplacé par `awk_inputs` toujours non vide ;
  `p2_integrity_metric_names` contient toujours 12 métriques et
  `p2_index_ready_metric_names` en reçoit au moins une avant leur expansion.
- `p2-gate.sh` : les globs `RUN_SCORES`, `run_dirs`, `summaries` et les trois
  familles de paires sont d'abord contrôlés par cardinalité et refusés avant
  toute itération ; les deux expansions de clés associatives vides
  (`RUN_CANONICAL_PATHS`, `RUN_EXECUTION_IDS`) ont été reproduites sur Bash
  4.3 et sont sûres (zéro itération). Tous les accumulateurs numériques et
  diagnostics sont développés seulement après les trois paires obligatoires
  et les boucles qui les remplissent.
- `p2-report.sh` : `phases` est initialisé avec sept entrées ;
  `test-p3-harness.sh` ne développe aucun tableau Bash par `[@]`.
- `$@` vide a été reproduit sur Bash 4.3 : il est sûr. Les usages de
  `fair-ab.sh` ont des arguments construits, ceux du gate une arité imposée,
  et le dispatcher du nouveau test est sous `if`.

Les paramètres optionnels relus utilisent `VAR:-`; les fonctions à arguments
positionnels du pilote, gate, rapport et helpers sont appelées avec leur
arité contractuelle. Cette conclusion reste statique/locale sur les
frontières VM : elle ne remplace pas une exécution complète de `fair-ab.sh`.

## Vérifications

- `bash -n` sur les six scripts shell concernés : PASS.
- `bash deploy/bench-local/test-p3-harness.sh` : PASS.
- `P3_MATRIX_EXHAUSTIVE=1 bash deploy/bench-local/test-p3-harness.sh` :
  PASS (118 verdicts de fixture).
- `bash deploy/bench-local/test-p3-campaign.sh` : PASS.
- Bash 4.3 local : `bash -n` des six scripts et
  `bash deploy/bench-local/test-p3-campaign.sh` : PASS. La CI Ubuntu exécute
  un Bash moderne : cette preuve locale ne prétend pas être une matrice CI
  multi-version.
- jq 1.6 téléchargé depuis la release officielle et utilisé pour
  `test-p3-campaign.sh` et la matrice normale `test-p3-harness.sh` : PASS.
- Aucun `cargo build/check/test/clippy` local n'a été lancé.

## Risque restant avant un nouveau smoke

Le risque d'un arrêt immédiat sur une variable non liée est désormais réduit,
pas nul : l'audit élimine les déclarations groupées et le seul tableau vide
fatal reproduit sur Bash 4.3 ; le pilote, ses hard-stops et ses propagations
d'échec sont exercés en CI sans Docker/VM. Le risque qui demeure est réel et
circonscrit aux frontières non simulées : disponibilité des trois commits et
du build Docker, disque/data-root, droits `sudo`/`drop_caches`, cgroup/systemd,
image et corpus réels, ainsi que les calculs complets de fair-ab/rapport/gate
sur la charge. Un nouveau smoke est justifié seulement après CI verte de ce
commit; il doit toujours précéder la campagne longue.

FIX4_DONE
