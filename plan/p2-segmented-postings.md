# P2 — parcours checked des postings par segment

## Finalité

Supprimer le parcours matérialisé des conjonctions disque et multi-segments
quand les postings sont lisibles, sans publier de résultat partiel en cas
d'erreur et sans modifier la réponse compatible OpenSearch.

## Track principal, branche et ownership

- Track principal : Track A — perf / optimisation.
- Branche : `main`.
- Ownership : SearchEngine.
- Fichiers : `crates/surch-index/src/postings.rs`,
  `crates/surch-index/src/document_index.rs` et
  `crates/surch-api/src/state.rs`.

## Scope

- Lecture checked qui distingue terme réellement absent, données lisibles et
  erreur `Io`, `Corrupt` ou `MissingCoverage`.
- Vue `SegmentedPostings` alignée sur tous les segments, avec `global_df`
  calculé sur tous les segments porteurs.
- Conjonction recall et scoring fusionné par segment pour les chemins disque,
  avec repli historique intégral sur erreur.
- Compteurs `blocks_read` et `blocks_total` publiés pour prouver les sauts.
- Le driver est choisi sur le `df` local, mais le score conserve le `df`
  global et l'ordre des clauses.

## Hors scope

- Migration des chemins historiques vers les API checked.
- Nouvelle erreur HTTP : une erreur P2 décline vers le chemin compatible.
- Toute conclusion de latence sans le protocole chiffré P2.

## Lots et gates

- [x] Lot S — ajouter les API checked sans retirer les wrappers historiques.
- [x] Lot 1 — construire `SegmentedPostings` RAM, disque et mixte, avec
  propagation de la première erreur et `global_df` global.
- [x] Lot 2 — parcourir les conjonctions recall segment par segment et
  reprendre le chemin matérialisé sur erreur.
- [x] Lot 3 — scorer les conjonctions fusionnées par segment, retirer le
  garde disque/multi-segment et conserver les fréquences capturées.
- [x] Lot 4 — exposer les compteurs de blocs, y compris après segment absent
  ou erreur d'avance avant repli.
- [x] Lot 5 — ajouter les tests P2 ciblés RAM/disque/mixte, segment illisible
  et saut de blocs, sans modifier les goldens de réponse.
  - [x] Correction de revue locale — une attestation canonique issue du
    payload refuse un répertoire P2 dont offset, count ou maximum diverge ;
    les cas maximum abaissé et haussé déclinent explicitement.
  - [x] Correction de revue locale — le scorer P1a décline avec `None` avant
    compteur/finalisation sur toute erreur checked ; les compteurs de blocs
    incluent tous les curseurs déjà ouverts, sans dépendre de l'ordre des
    clauses ; une injection tardive prouve que le préfixe temporaire n'est
    jamais finalisé.
  - [x] Correction de revue locale — parité forcé-générique étendue aux
    termes distincts, `global_df` asymétrique, RAM/disque/mixte,
    `min_score` et `from`/`size`.
  - [x] Correction 2 de revue locale — chaque bloc chargé atteste son count,
    ses bornes supérieure et inférieure et son ordre strict ; une divergence
    devient `DiskPostingsAdvance::Error` avant toute publication.
  - [x] Correction 2 de revue locale — les scalaires de payload de
    `TermEntry` sont attestés et les plages de lecture utilisent des additions
    checked ; les compteurs conservent les curseurs ouverts lors d'un échec de
    construction, et les tests de métriques sont sérialisés.
  - [x] Correction 2 de revue locale — le golden P2 couvre `min_score`
    effectivement filtrant et trois clauses scorées dans l'ordre demandé.
  - [x] Correction CI locale — `PostingsBlockSkipIter` est importé dans
    `document_index`, où P2 l'emploie pour le curseur RAM segmenté ; l'audit
    statique couvre les symboles ajoutés, retirés et rendus publics par P2.
  - [x] Correction CI locale — la fixture de parité disque type explicitement
    son compteur `usize` avant `is_multiple_of` ; le balayage statique des
    tests P2 et des modules `#[cfg(test)]` ne relève aucune autre ambiguïté
    numérique, méthode standard incompatible, import inutile ou symbole non
    résolu.
- [x] Lot 6 — harnais de mesure P2 local : corps `bool.must` mono-token
  attestés, empreintes SHA-256 gelées, snapshots Prometheus avant/après,
  réponses canoniques A/B, séries bool/match et bootstrap apparié. Le pilote
  `deploy/bench-local/p2-campaign.sh` impose le smoke puis les trois paires
  contrebalancées, sans lancer la campagne depuis le dépôt.
  - [x] Correction feeder — `FEEDER_TMP_DIR` monte un répertoire hôte dédié
    sous `OUT_DIR` dans `/tmp` du feeder ; préflight `bulk + 512 Mio`, refus
    tmpfs ou système de fichiers du data-root Docker, nettoyage sur succès,
    échec et signal.
  - [x] Correction protocole — P2 ne fige plus la taille totale de la
    machine : le moteur et la sonde restent disjoints, la scorecard conserve
    `nproc` et les deux cpusets observés, et le rapport refuse une paire A/B
    dont cette configuration diffère.
  - [x] Correction protocole — le contrôle `index_ready` exige seulement la
    jauge présente dès l'indexation (`surch_index_segment_count`) ; les
    compteurs Prometheus absents avant leur premier incrément sont lus comme
    zéro pour établir le delta initial, puis leur présence et leurs deltas de
    routage restent fail-closed à la fin de chaque phase `bool`.
  - [x] Assouplissement des gates non causaux — les quatre phases chaudes
    `warm/fixed/random/no_source` restent obligatoires ; cold est un
    diagnostic optionnel, le steal CPU est mesuré par phase, et le ratio de
    blocs devient un résultat `PASS/ÉCHEC P2` séparé de `measurement_valid`.
    Le pilote soustrait aussi la croissance volontaire des artefacts de
    campagne du contrôle de récupération disque sur FS partagé.
  - [x] Correction pilote de statistiques — l'émetteur des quantiles P2
    termine désormais sa ligne. Le `read` contrôlé par le pilote refusait à
    tort une série fixed complète à EOF sans nouvelle ligne ; les contrôles
    de cardinalité, format numérique, quantiles, routage et parité restent
    strictement identiques pour fixed, random, no_source et cold.
- [ ] P3 — attestation BLAKE3-256 par pages de 4 Kio des deux régions
  canoniques P2, sans copies résidentes des scalaires ni du répertoire.
  - [x] Le producteur unique seal/merge publie atomiquement deux descripteurs
    de région et une table de digests partagée, ou garde le fallback résident
    compté ; chaque reconstruction reçoit le budget restant de l'index, donc
    le total multi-segment ne dépasse jamais 32 Mio et P2 décline explicitement
    si une preuve complète ne tient pas.
  - [x] Le lecteur authentifie la page de `TermEntry`, borne le payload,
    déduit exactement `ceil(df/128)`, puis authentifie le répertoire avant
    tout saut ; il décline intégralement sur incohérence.
  - [x] Les jauges P3 exposent digests, pages, octets vérifiés, échecs de
    hash, déclins, champs fallback et les composantes Lot 0 `T/B/F`.
  - [x] La portée est explicite : l'attestation BLAKE3 protège seulement
    `disk_cursor_p2_checked`, le chemin P2 à sauts. Le repli générique
    historique (`term_entry` / `decode_from_segment` / merge), antérieur à
    P2 et inchangé par P3, repart de zéro sans revendiquer cette garantie.
  - [x] Le gate P3 lit les métriques agrégées de l'index :
    `p2_integrity_bytes <= 32 Mio`, `p2_fallback_fields == 0` et
    `p2_hash_failures == 0`; un dépassement multi-segment ou un fallback
    résident rend la campagne invalide avant le verdict de latence. Le
    lecteur P2 décline aussi intégralement avant d'ouvrir un curseur si ce
    plafond global ou cette absence de preuve est constaté.
  - [x] Le pilote conserve A=pré-P2 et B=P2, ajoute C=P3, vérifie la parité
    des trois paires A/B, B/C et A/C, applique les SLO principaux à C/A,
    puis refuse un coût moteur p95 C/B supérieur à `1,05`; l'intégrité P3
    n'est exigée que pour C.
  - [x] Le harnais P3 sélectionne maintenant trois ensembles mono-token
    disjoints sur `NOM` (bool, témoin match et chauffe), gèle les quatre
    corps causaux par SHA-256 et mesure le témoin match avant tout bool. Les
    snapshots JSONL couvrent P3, mémoire processus/jemalloc et cgroup à
    `index_ready` puis autour de chaque phase ; une métrique absente invalide
    le run. Le mix 50/50 historique ne subsiste que comme replay opt-in.
  - [x] Correctifs de première revue P3 B1-B9 et M1-M7 intégrés localement :
    JSON cgroup validé par `jq`, SHA A/B/C gelés, ordre latin et hard-stops,
    provenance/image, JSONL, dérivés mémoire/compaction et verdict non-PASS
    non nul. Cette ligne ne vaut pas preuve de campagne externe.
  - [x] Correctif de re-revue P3 : imposer la bijection exacte des neuf runs
    A1/A2/A3, B1/B2/B3 et C1/C2/C3 (scorecards, manifests,
    `pair-summary.json` et `parity.json` liés), compléter le schéma P3,
    versionner la matrice du gate et verrouiller le smoke v4. Le sélectionneur
    applique désormais la table awk `asciifolding` + lowercase sur les champs
    réellement interrogés ; les entrées non mono-token restent refusées. La
    matrice `test-p3-harness.sh` exécute le gate et couvre PASS/ÉCHEC/INVALIDE/
    REJOUER, dont la duplication N1.
  - [x] Fraîcheur jemalloc P3 : C reste volontairement pinné à `d0accd6`, qui
    n'embarque pas le refresh runtime. Les valeurs jemalloc sont donc retirées
    des dérivés et gates de récupération fraîche ; les diagnostics bruts ne
    portent aucune revendication. Dans HEAD, le refresh runtime est borné à
    une seconde et expose succès, âge et erreurs. Aucun protocole ultérieur ne
    doit repinner C sans preuve de coût de scrape concurrent.
  - [x] Les unités couvrent la parité de lecture, les scalaires `TermEntry`,
    les pages de répertoire, permutation/troncature, maximum abaissé/haussé,
    digest altéré et l'absence de `pread` variable avant l'échec scalaire.
  - [ ] Gate externe — exécuter les tests Rust ciblés, Clippy, les oracles de
    parité et la campagne mémoire/latence C ; interdits dans cette mission.
- [ ] Gate externe — exécuter les tests Rust ciblés, clippy et CI ; interdits
  dans cette mission.
- [ ] Gate externe — vérifier les goldens forcé-générique/P2, les compteurs
  de couverture et la parité avant toute conclusion de performance.
- [ ] Gate externe — comparer A=`961ade1`, B=`6ce390e` et C=commit P3 livré
  sur 28 917 511 documents, 6 Gio, douze segments et trois triplets
  contrebalancés.
- [ ] Gate externe — valider p95 `took` C/A <= 0,50, p99 C/A <= 0,70,
  coût p95 moteur C/B <= 1,05, couverture compteur +500, contrôle `match`
  <= 1,05x et parité A/B + B/C verte ; rapporter séparément le ratio
  `blocks_read / blocks_total` (cible <= 25 %) comme résultat P2, sans
  invalider la mesure.
- [ ] Décision de poursuite — abandonner P2 après trois paires valides si le
  saut est prouvé mais que le p95 gagne moins de 30 % ; autoriser une seule
  itération de profilage entre 30 et 50 % de gain.

## Preuves

- Preuves locales : diff relu, `cargo fmt --all`, `cargo fmt --all -- --check`
  et `git diff --check` vérifiés ; les tests P2 restent non exécutés par
  contrainte de mission.
- Preuves externes attendues : CI, goldens de parité, compteurs de routage et
  rapport chiffré conservant `took`, p95/p99, ratio de blocs, contrôle
  négatif et le coût C/B de l'authentification par curseur. La formule P3
  `32 × Σf(ceil(28Tf / 4096) + ceil(10Bf / 4096)) + 64F` estime uniquement
  les digests et descripteurs ; ses bornes corpus déduites restent 12–17 Mio,
  et la mesure corpus reste obligatoire pour inclure allocations, tables,
  compteurs et allocateur.
- Risque principal : utiliser le `df` local pour l'IDF et modifier score,
  `max_score`, ordre ou `min_score` malgré des ids exacts.

## Statut de merge

- [x] Développement livré localement sur `main` dans le commit
  `[latence P2] parcourir les postings checked par segment`.
- [x] P2 est poussé jusqu'à `3d33d8d` sur `main` ; la correction de
  compilation CI reste locale et non poussée.
- [ ] Gates externes non validées ; aucune preuve de performance ou de parité
  ne doit être inférée des contrôles locaux.
