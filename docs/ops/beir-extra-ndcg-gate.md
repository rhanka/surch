# Gate BEIR complémentaire : NFCorpus et FiQA

Le Job `beir-extra-ndcg` protège la qualité de recherche sur les deux
corpus BEIR qui ne sont pas couverts par `ndcg-gate` :

- `nfcorpus` (qrels gradués) ;
- `fiqa` (qrels binaires).

Le script générique `scripts/bench/beir-ndcg.sh` calcule `NDCG@10` avec le
gain gradué `2^rel-1`, et `Recall@10` à titre diagnostique. Il parcourt
toutes les requêtes de test ayant au moins un qrel positif. Un qrel sans
texte de requête, un corpus incomplet, une réponse de recherche mal formée
ou un nombre de requêtes traité différent du total fait échouer le Job.

`ndcg-gate` reste le Job séparé de SciFact et TREC-COVID. Le script peut
traiter ces quatre corpus, mais ce Job-ci exécute uniquement NFCorpus et
FiQA.

## Décision du gate

La baseline historique brute est le run `26476471207` du 2026-05-26 :

| Jeu | Plancher Surch NDCG@10 | Écart maximal au OpenSearch courant |
|---|---:|---:|
| NFCorpus | 0,3033 | 0,0010 |
| FiQA | 0,2294 | 0,0100 |

Ces seuils sont déclarés dans
`deploy/k8s/jobs/beir-extra-ndcg.yaml`, avec la date et le run de baseline. Un PASS
exige, pour chaque jeu :

1. les deux sorties Surch et OpenSearch non vides ;
2. les cardinalités brutes attendues (NFCorpus : 3 633 documents / 323
   qids de test positifs ; FiQA : 57 638 / 648) ;
3. une seule valeur numérique `NDCG@10` par sortie ;
4. toutes les requêtes à qrel positif traitées ;
5. `NDCG@10(Surch)` supérieur ou égal au plancher ;
6. `NDCG@10(Surch) - NDCG@10(OpenSearch courant)` supérieur ou égal à
   l'écart négatif maximal.

OpenSearch 2.17.1 est lancé dans le même Pod, sur les mêmes corpus : la
comparaison d'acceptation est donc courante et ne peut pas réussir contre
une référence OpenSearch périmée. Le plancher historique détecte en plus
une régression absolue. `Recall@10` est conservé dans le rapport, mais la
métrique de décision est bien `NDCG@10`.

Les valeurs sont transcrites dans
`docs/paper/etat-des-lieux-benchmark-2026-07-25.md`. Il n'existe pas
d'artefact BEIR K8s daté exactement du 2026-07-04 : le dernier vert de cette
date est le Job fonctionnel `b1-oracle-gate` (`28689787902`). L'artefact
K8s brut NFCorpus/FiQA qui sert de baseline est
`docs/ops/bench-reports/2026-05-26-F4-beir-nfcorpus-fiqa-K8s/`, run
`26476471207`. Ces sources sont conservées pour l'audit, pas utilisées
seules pour accepter un run courant.

## Relance

Après que les images `sha-<SHA>` et `bench-sha-<SHA>` du commit `main` ont
été publiées dans GHCR, relancer d'une seule commande :

```bash
gh workflow run ci-k8s.yml --ref main -f job=beir-extra-ndcg
```

Le workflow rend le manifeste avec le SHA du commit, vérifie les deux tags
GHCR avant `kubectl apply`, puis échoue si le Job, une métrique ou un seuil
échoue. L'absence des images est donc un échec explicite, pas un vert sans
mesure. L'artefact contient `summary.md` (les quatre sorties y sont
concaténées), `bench.json` et les logs du Job.
