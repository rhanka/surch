# P5 — Audit et remise en état des garde-fous BEIR

Date d'audit : 2026-07-28.

## État des lieux factuel

`scripts/bench/beir-ndcg.sh` est générique : il accepte un jeu BEIR au
layout `corpus.jsonl`, `queries.jsonl`, `qrels/test.tsv`. Il indexe
`title` et `text`, interroge un `multi_match` et calcule, sur toutes les
requêtes de test qui ont un qrel positif :

- `NDCG@10`, gain gradué `2^rel - 1` ;
- `Recall@10`, diagnostic conservé dans la sortie.

Le Job `deploy/k8s/jobs/beir-extra-ndcg.yaml` utilise ce script sur
NFCorpus et FiQA, contre Surch et OpenSearch 2.17.1 dans le même Pod.
SciFact et TREC-COVID relèvent du Job séparé `ndcg-gate`; le script peut
également les traiter mais ils ne sont pas lancés par le Job extra.

Avant ce lot, `beir-extra-ndcg` ne contenait aucun seuil : son propre
commentaire le définissait comme une première observation. Les quatre
sorties étaient concaténées, mais aucune absence de métrique, absence de
corpus, requête ignorée ou régression NDCG ne pouvait faire échouer le
driver après un résultat partiel.

La référence historique parfois attribuée au « 2026-07-04 » est transcrite
dans `docs/paper/etat-des-lieux-benchmark-2026-07-25.md` : NFCorpus
`0,3033`/`0,3034` et FiQA `0,2294`/`0,2389` (Surch/OpenSearch). L'audit
établit qu'il n'existe pas de rapport BEIR K8s brut daté exactement du
04/07. La baseline réellement traçable est le run GitHub `26476471207` du
2026-05-26, sous
`docs/ops/bench-reports/2026-05-26-F4-beir-nfcorpus-fiqa-K8s/`.

## Cause réelle de la péremption

La cause est l'absence de relance, non un Job BEIR démontré cassé :

- `ci-k8s.yml` ne se déclenche que par `workflow_dispatch`;
- la liste GitHub des exécutions depuis le 2026-07-04 contient seulement
  les runs `28689243205`, `28689521392`, `28689787902` (04/07) et
  `28750129816` (05/07), tous des Jobs `b1-oracle-gate`;
- le run vert `28689787902` est donc un oracle fonctionnel B1, pas un
  nDCG BEIR;
- aucune exécution `beir-extra-ndcg` n'a été redispatchée après
  `26476471207` du 26/05.

Le blocage actuel pour une mesure HEAD est distinct et vérifié : le cluster
`poc-979c11ad-9f84-4847-a334-c42a5e797976` et le namespace `surch` sont
joignables, mais les deux tags GHCR correspondant au HEAD audité avant ce
lot répondaient `404 MANIFEST_UNKNOWN`. Le gate ne doit pas être lancé sur
une image d'un autre SHA : il échouerait avant la mesure. Le commit de ce
lot exige donc à son tour ses tags `sha-<SHA>` et `bench-sha-<SHA>` avant
relance.

## Correctifs livrés

- `beir-ndcg.sh` accepte `BEIR_REQUIRE_LOCAL_DATA=1` : sur le Job K8s, un
  corpus incomplet échoue au lieu de télécharger silencieusement. Il refuse
  aussi l'absence de qrel positif, une requête manquante, une réponse
  `_search` mal formée, un `_bulk` dont `errors` n'est pas strictement
  `false`, un `_count` différent du corpus source et un total de requêtes
  traité incomplet.
- `00b-init-beir-extra.yaml` ne considère plus `queries.jsonl` seul comme
  preuve de présence : les trois fichiers non vides et au moins un qrel
  positif sont requis, sinon le PVC est réhydraté et un échec est fatal.
- Les deux chemins vérifient les cardinalités attendues : NFCorpus
  3 633 documents / 323 qids de test positifs ; FiQA 57 638 / 648.
- `beir-extra-ndcg.yaml` porte une baseline brute versionnée (26/05,
  run `26476471207`) et des seuils : NFCorpus
  `NDCG@10 >= 0.3033`, FiQA `>= 0.2294`; il exige aussi une comparaison
  vivante avec OpenSearch dans le même Pod (écart maximal respectivement
  `0.0010` et `0.0100`). Les sorties, métriques et comptes de requêtes sont
  tous obligatoires. `bench.json` passe au schéma
  `surch.bench.ndcg_gate.v2` et inclut les seuils.
- `docs/ops/beir-extra-ndcg-gate.md` documente périmètre, références,
  critères et relance. La documentation d'état des lieux corrige
  l'attribution erronée du vert du 04/07.

La décision ne repose donc pas sur une référence OpenSearch ancienne : la
comparaison OpenSearch est recalculée dans le Pod courant. Les valeurs
historiques ne servent qu'au plancher absolu de non-régression.

## Vérifications exécutées

- `bash -n scripts/bench/beir-ndcg.sh` : succès.
- Avec `BEIR_REQUIRE_LOCAL_DATA=1` et un corpus absent : échec attendu,
  code 1 et message explicite.
- Rendu du manifeste avec `SURCH_SHA=$(git rev-parse HEAD)`, puis
  `kubectl apply --dry-run=client -f -` : succès.
- Extraction du script driver rendu et `/bin/sh -n` : succès.
- `git diff --check` : succès.

Aucun `cargo build`, `cargo check`, `cargo test` ou `cargo clippy` local
n'a été exécuté. Aucun run K8s n'a été lancé : il n'existe pas encore
d'image SHA exacte pour le HEAD à mesurer.

## Relance et résultat attendu

Après publication des deux images du commit `main`, lancer :

```bash
gh workflow run ci-k8s.yml --ref main -f job=beir-extra-ndcg
```

Le workflow vérifie les tags GHCR avant `kubectl apply`; une image absente,
un Job en échec, une sortie absente ou un seuil non tenu échoue donc sans
faux vert. Les résultats NDCG HEAD contre la baseline du 26/05 et
OpenSearch courant restent à mesurer après cette publication.

BEIR_DONE
