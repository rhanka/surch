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
    compté ; le plafond de 32 Mio refuse toute dérive silencieuse.
  - [x] Le lecteur authentifie la page de `TermEntry`, borne le payload,
    déduit exactement `ceil(df/128)`, puis authentifie le répertoire avant
    tout saut ; il décline intégralement sur incohérence.
  - [x] Les jauges P3 exposent digests, pages, octets vérifiés, échecs de
    hash, déclins, champs fallback et les composantes Lot 0 `T/B/F`.
  - [x] Les unités couvrent la parité de lecture, les scalaires `TermEntry`,
    les pages de répertoire, permutation/troncature, maximum abaissé/haussé,
    digest altéré et l'absence de `pread` variable avant l'échec scalaire.
  - [ ] Gate externe — exécuter les tests Rust ciblés, Clippy, les oracles de
    parité et la campagne mémoire/latence C ; interdits dans cette mission.
- [ ] Gate externe — exécuter les tests Rust ciblés, clippy et CI ; interdits
  dans cette mission.
- [ ] Gate externe — vérifier les goldens forcé-générique/P2, les compteurs
  de couverture et la parité avant toute conclusion de performance.
- [ ] Gate externe — comparer `961ade1` et P2 sur 28 917 511 documents,
  6 Gio, douze segments et trois paires contrebalancées.
- [ ] Gate externe — valider p95 `took` P2/baseline <= 0,50, p99 <= 0,70,
  couverture compteur +500, contrôle `match` <= 1,05x et parité verte ;
  rapporter séparément le ratio `blocks_read / blocks_total` (cible <= 25 %)
  comme résultat P2, sans invalider la mesure.
- [ ] Décision de poursuite — abandonner P2 après trois paires valides si le
  saut est prouvé mais que le p95 gagne moins de 30 % ; autoriser une seule
  itération de profilage entre 30 et 50 % de gain.

## Preuves

- Preuves locales : diff relu, `cargo fmt --all`, `cargo fmt --all -- --check`
  et `git diff --check` vérifiés ; les tests P2 restent non exécutés par
  contrainte de mission.
- Preuves externes attendues : CI, goldens de parité, compteurs de routage et
  rapport chiffré conservant `took`, p95/p99, ratio de blocs et contrôle
  négatif.
- Risque principal : utiliser le `df` local pour l'IDF et modifier score,
  `max_score`, ordre ou `min_score` malgré des ids exacts.

## Statut de merge

- [x] Développement livré localement sur `main` dans le commit
  `[latence P2] parcourir les postings checked par segment`.
- [x] P2 est poussé jusqu'à `3d33d8d` sur `main` ; la correction de
  compilation CI reste locale et non poussée.
- [ ] Gates externes non validées ; aucune preuve de performance ou de parité
  ne doit être inférée des contrôles locaux.
