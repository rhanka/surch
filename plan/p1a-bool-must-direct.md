# P1a — bool.must direct single-pass exact

## Finalité

Supprimer, pour la forme stricte `bool.must` à deux `match` mono-token, le
double décodage, les `BTreeSet` de rappel et le second contexte de scoring sans
modifier la réponse OpenSearch-compatible.

## Track principal

Track A — perf / optimisation.

## Branche et ownership

- Branche : `main`.
- Ownership : SearchEngine, `crates/surch-index/src/postings.rs`,
  `crates/surch-api/src/{search,state}.rs` et les tests ciblés de
  `crates/surch-api/tests/`.

## Scope

- [x] Router uniquement la racine `bool.must` exacte à deux `match`, sans
  `should`, `filter`, `must_not` ni boost.
- [x] Exécuter P1a sur le chemin RAM mono-segment avec un score `must` qui
  conserve chaque contribution BM25, y compris `1.0`; décliner vers le
  générique sur disque ou multi-segment tant que leur lecture reste ambiguë.
- [x] Distinguer la fin normale des postings d'une erreur `pread` ou de
  décodage dans le seul scorer `must`, qui décline alors vers le générique.
- [x] Conserver `reduce_deces_conjunction_into`, le chemin `should`, la
  finalisation TopN, `min_score`, `from`, `size`, `max_score` et les totaux.
- [x] Ajouter une référence générique forçable et les matrices RAM/disque,
  mono/multi-segment et doublon de clause, avec scores binaires, `max_score`
  et relation du total.

## Hors scope

- Mono-`match` single-pass P1b.
- Curseur multi-segment, format disque, `should` et P2.
- Harnais P0, stockage `_source` et artefacts de mesure.

## Lots et gates

- [x] Lot 1 — route directe et scoring `must` fusionné.
- [x] Lot 2 — goldens générique/rapide : ordre, bits de score, `max_score` et
  total avec `min_score`, pagination et modes `track_total_hits`; la matrice
  vérifie la référence sur RAM, disque, mono-segment et multi-segment.
- [x] Correction de revue — l'erreur tardive du curseur disque décline le
  chemin P1a au lieu de finaliser un préfixe scoré.
- [x] Correction 2 de revue — Option A : P1a décline avant tout accès disque
  ou multi-segment ambigu ; les matrices disque et multi-segment vérifient la
  parité générique et l'absence d'incrément du compteur rapide.
- [x] Lot 3 — `cargo fmt`, `cargo fmt --check` et `git diff --check`.
- [ ] Gate externe — tests Rust ciblés, clippy et CI : interdits dans cette
  mission, à rejouer par le conducteur.
- [ ] Gate externe — oracle B1 Elasticsearch 8.6.1, 30/30, zéro skip et zéro
  divergence à ce HEAD.
- [ ] Gate externe — trois paires 28,9 M / 6 Gio et verdict P1a.

## Preuves et statut de merge

- Preuves locales : diff relu, format et whitespace vérifiés; les tests Rust,
  clippy et les gates externes restent volontairement non exécutés.
- Preuves attendues : B1, CI et rapport de latence promu avec le compteur
  `surch_bool_direct_must_fused_total`.
- Statut de merge : `e4f505e` puis son correctif de revue sont locaux et non
  poussés; aucune preuve externe ne doit être inférée des vérifications
  locales.
