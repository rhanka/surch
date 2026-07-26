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
- [ ] Gate externe — exécuter les tests Rust ciblés, clippy et CI ; interdits
  dans cette mission.
- [ ] Gate externe — vérifier les goldens forcé-générique/P2, les compteurs
  de couverture et la parité avant toute conclusion de performance.
- [ ] Gate externe — comparer `961ade1` et P2 sur 28 917 511 documents,
  6 Gio, douze segments et trois paires contrebalancées.
- [ ] Gate externe — valider p95 `took` P2/baseline <= 0,50, p99 <= 0,70,
  ratio `blocks_read / blocks_total` <= 25 %, couverture compteur +500,
  contrôle `match` <= 1,05x et parité verte.
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
- [ ] Commit non poussé et non validé par les gates externes ; aucune preuve
  de performance ou de parité externe ne doit être inférée des contrôles
  locaux.
