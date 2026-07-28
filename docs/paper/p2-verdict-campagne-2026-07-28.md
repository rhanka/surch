# P2 — verdict complet de la campagne du 2026-07-28

## Résumé exécutable

**Verdict formel du gate de campagne : ÉCHEC P2.**

Ce verdict n'est ni un échec de la mesure primaire, ni la preuve que P2
ralentit le moteur. Les trois paires sont techniquement valides, P2 est
effectivement emprunté, et la mesure primaire passe largement :

- p95 moteur `took`, `bool.must size:10` : ratios B/A
  `0,5510 / 0,4118 / 0,4194`, médiane `0,4194`, soit environ
  **−58,1 %** ;
- p95 client de la même série : ratios
  `0,5440 / 0,4205 / 0,4248`, médiane `0,4248`, soit environ
  **−57,5 %** ;
- noyau `bool.must size:0` : médiane p95 `0,1724` et p99 `0,0462` ;
- bornes supérieures des trois IC95 bootstrap appariés :
  `0,6364 / 0,5102 / 0,6000`, toutes strictement sous `0,90`.

Le gate global échoue parce que le témoin obligatoire `random match` régresse
sur les trois paires : p95 `took` B/A
`1,1111 / 1,2222 / 1,2222`, hors de `[0,95 ; 1,05]`. Le p95 client confirme
le signal (`1,1864 / 1,2024 / 1,1171`) : ce n'est pas seulement un artefact
de l'arrondi entier de `took`.

Le plan dit qu'une régression témoin supérieure à 5 % annule la campagne et
ne doit pas être imputée à P2. Le sens exact du verdict est donc :

> **ÉCHEC formel du gate de campagne par régression du témoin non causal ;
> gain primaire P2 observé mais claim P2 causal non libérable en l'état.**

Le verdict n'est pas `INCOMPLET` : toutes les preuves obligatoires des quatre
phases chaudes sont présentes et évaluables. Le cold incomplet sur A2 et A3
est explicitement optionnel.

## 1. Périmètre, références et artefacts

Comparaison interne Surch :

- A, avant P2 :
  `961ade10ffb74d78156aee8148f1e5c6bbbe6ba2` ;
- B, avec P2 :
  `6ce390e55da3593242ec11e2b09d4dee1057726d` ;
- ordre contrebalancé observé dans `campagne.log` :
  A1→B1, B2→A2, A3→B3 ;
- racine des 356 Mio d'artefacts :
  `/home/antoinefa/.cache/fair-ab/p2-campagne-2026-07-28/` ;
- runs :
  `{a1,b1,b2,a2,a3,b3}/`.

Fichiers utilisés dans chaque run :

- `surch.json` : scorecard, image, configuration CPU, count, segments et
  validité ;
- `surch.p2.phase-status.jsonl` : routage, compteurs de blocs, steal et
  segments par phase ;
- `surch.p2.stats.jsonl` : quantiles produits pendant le run ;
- `surch.p2.{fixed,random,no_source}.{bool,match}.{client_s,took_ms,probe_ms}`
  selon les séries applicables : mesures brutes ;
- `surch.p2.responses.{warm,fixed,random,no_source}.canonical.ndjson` :
  réponses sans `took`, utilisées pour la parité.

Les scorecards attestent les mêmes entrées sur les six runs :

| Élément | Valeur attestée |
|---|---|
| Corpus | `28 917 511` documents, `count == indexed == expected`, zéro item error |
| SHA-256 bulk | `2a7e7aef6bb0c880b565bc175f1aebcd57ba347c22304fce96238d214a582c3a` |
| SHA-256 mapping | `036f12051e42889dba2388cb39baa7358a30f2c1a9f09372312c8f0ab9520588` |
| SHA-256 manifeste P2 | `dace1ab08cf5a037f9c6407d4c542a9de15ab21cfc00ffd74fa11b699b79c331` |
| Segments | exactement `12` dans les six runs et pendant chaque phase |
| Cap mémoire moteur | `6g` |
| CPU observés | `nproc=16`, moteur `0-7`, sonde `8,9,10,11,12,13,14,15` |
| Connexion chaude | `single_curl_next` pour fixed, random et no_source |
| Profil `_source` | désactivé |
| Steal chaud maximal par run | `0,009346 %`, très inférieur au plafond `1 %` |
| A image ID/digest | `sha256:287232d728766e597007f1085d170541aa44af608bfd7f99686080a156607515` |
| B image ID/digest | `sha256:dae6c80f9f4e2ec3af85979fff1dd2ca4ee060fa310aac8638db3593a3051af2` |

Les artefacts fournis n'attestent pas le nom du fournisseur de la VM, son
modèle commercial ni le type du volume hôte. Il serait donc incorrect
d'écrire « OVH b3-16 » ou « volume classic » dans un claim public sur la seule
base de cette copie. La configuration CPU effectivement attestée est par
ailleurs 8 CPU moteur et 8 CPU sonde, pas la configuration 3+1 du plan
historique.

## 2. Méthode de calcul

Les calculs suivent `deploy/bench-local/p2-report.sh` :

- `client` est capturé en secondes puis converti en millisecondes ;
- `took` est le temps moteur rendu par la réponse, en millisecondes entières ;
- `probe = client - took`, en millisecondes ;
- les p50/p95/p99 utilisent le rang le plus proche
  (`ceil(n × quantile)`) ;
- la phase random contient 1 000 bool et 1 000 match, conservés en séries
  distinctes ;
- la phase fixed contient 2 000 match ;
- la phase no_source contient 1 000 bool et 1 000 match ;
- le ratio d'une paire est le quantile B divisé par le même quantile A ;
- le résultat inter-paires est la médiane des trois ratios, sans concaténer
  les runs ;
- le bootstrap primaire rééchantillonne les 1 000 corps bool appariés
  10 000 fois, avec la graine déterministe `20260726`, et recalcule à chaque
  tirage le p95 A, le p95 B et leur ratio.

Les trois commandes suivantes ont été exécutées, avec des sorties temporaires
hors dépôt :

```text
deploy/bench-local/p2-report.sh --a .../a1 --b .../b1 --out <pair1>
deploy/bench-local/p2-report.sh --a .../a2 --b .../b2 --out <pair2>
deploy/bench-local/p2-report.sh --a .../a3 --b .../b3 --out <pair3>
```

Les quantiles du rapport ont été contre-vérifiés contre
`surch.p2.stats.jsonl` et les séries brutes. Les cardinalités attendues sont
exactes. La parité a été recalculée avec `cmp` sur les fichiers canoniques,
et non inférée de `measurement_valid`.

### Attention aux p95 mélangés

Les valeurs déjà relevées `35,37 → 23,82`, `40,94 → 23,55` et
`40,51 → 23,16 ms` sont les p95 **client du mélange random bool+match** dans
la scorecard générale. Elles ne sont pas la mesure primaire.

La mesure primaire est le p95 `took` des seuls bool `size:10` :
`49 → 27`, `68 → 28` et `62 → 26 ms`.

## 3. Mesure primaire — bool `size:10`

| Paire | A p95 took | B p95 took | B/A took | Gain | A p95 client | B p95 client | B/A client | IC95 bootstrap du ratio took |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| A1→B1 | 49 ms | 27 ms | 0,5510 | −44,9 % | 51,095 ms | 27,797 ms | 0,5440 | [0,3140 ; 0,6364] |
| B2→A2, remis A→B | 68 ms | 28 ms | 0,4118 | −58,8 % | 69,566 ms | 29,251 ms | 0,4205 | [0,2366 ; 0,5102] |
| A3→B3 | 62 ms | 26 ms | 0,4194 | −58,1 % | 63,452 ms | 26,956 ms | 0,4248 | [0,2889 ; 0,6000] |
| Médiane des ratios | — | — | **0,4194** | **−58,1 %** | — | — | **0,4248** | borne haute maximale **0,6364** |

Résultat des sous-gates :

- médiane p95 `took <= 0,70` : **PASS** ;
- médiane p95 client `<= 0,70` : **PASS** ;
- chacune des trois paires `took` dans le même sens et `<= 0,80` :
  **PASS** ;
- borne supérieure de chaque IC95 strictement `< 0,90` : **PASS**.

Le ratio médian `0,4194` correspond à un p95 moteur environ `2,38×` plus
rapide dans ce protocole interne. Cette formulation reste descriptive tant
que le témoin random n'est pas expliqué.

## 4. Noyau — bool `size:0`

| Paire | A p95 took | B p95 took | B/A p95 | A p99 took | B p99 took | B/A p99 |
|---|---:|---:|---:|---:|---:|---:|
| A1→B1 | 30 ms | 5 ms | 0,1667 | 195 ms | 9 ms | 0,0462 |
| B2→A2, remis A→B | 29 ms | 5 ms | 0,1724 | 192 ms | 8 ms | 0,0417 |
| A3→B3 | 29 ms | 5 ms | 0,1724 | 192 ms | 10 ms | 0,0521 |
| Médiane des ratios | — | — | **0,1724** | — | — | **0,0462** |

Résultat :

- médiane p95 `<= 0,50` : **PASS** ;
- médiane p99 `<= 0,70` : **PASS**.

Le p95 `took` du témoin match `size:0` vaut zéro des deux côtés dans la
plupart des runs. Son ratio est donc mathématiquement indéfini et n'est pas
utilisé. Ce point n'affecte aucun critère : le noyau est défini sur bool, et
les témoins causaux du plan sont random match et fixed match.

## 5. Témoins match

| Paire | Random A p95 took | Random B p95 took | B/A took | Random B/A client | Fixed A p95 took | Fixed B p95 took | Fixed B/A took |
|---|---:|---:|---:|---:|---:|---:|---:|
| A1→B1 | 9 ms | 10 ms | **1,1111** | **1,1864** | 22 ms | 21 ms | 0,9545 |
| B2→A2, remis A→B | 9 ms | 11 ms | **1,2222** | **1,2024** | 20 ms | 21 ms | 1,0500 |
| A3→B3 | 9 ms | 11 ms | **1,2222** | **1,1171** | 20 ms | 21 ms | 1,0500 |
| Médiane | — | — | **1,2222** | **1,1864** | — | — | **1,0500** |

Résultat :

- fixed match p95 `took` dans `[0,95 ; 1,05]`, bornes incluses :
  **PASS** sur les trois paires ;
- random match p95 `took` dans `[0,95 ; 1,05]` :
  **ÉCHEC** sur les trois paires ;
- confirmation client random :
  **ÉCHEC** sur les trois paires.

La régression random match est répétée dans les deux ordres A/B. Le routage
P2 reste nul pour match ; cette régression est donc un signal de
non-comparabilité ou un effet secondaire commun à diagnostiquer, pas un gain
ou un coût directement attribuable au parcours P2.

## 6. Coût de sonde

Le gate automatisé utilise le p95 de la sonde pour le bool random primaire.

| Paire | A p95 probe | B p95 probe | B−A | \|B−A\| |
|---|---:|---:|---:|---:|
| A1→B1 | 2,014 ms | 1,460 ms | −0,554 ms | 0,554 ms |
| B2→A2, remis A→B | 2,099 ms | 1,498 ms | −0,601 ms | 0,601 ms |
| A3→B3 | 2,123 ms | 1,516 ms | −0,607 ms | 0,607 ms |

Les trois écarts absolus sont `<= 2 ms` : **PASS**.

Un audit plus large de toutes les séries chaudes donne un écart p95 absolu
maximal de `0,685 ms` (`no_source/bool`, paire 1), lui aussi sous 2 ms. La
baisse du temps client primaire ne peut donc pas être expliquée par la sonde.

## 7. Routage, sauts de blocs et segments

Les trois répétitions donnent exactement les mêmes deltas de routage :

| Variante | Phase | Bool | `direct_must_fused` | `generic` | Verdict |
|---|---|---:|---:|---:|---|
| A | warm | 100 | +0 | +100 | PASS attendu A |
| A | random | 1 000 | +0 | +1 000 | PASS attendu A |
| A | no_source | 1 000 | +0 | +1 000 | PASS attendu A |
| B | warm | 100 | +100 | +0 | PASS attendu B |
| B | random | 1 000 | +1 000 | +0 | PASS attendu B |
| B | no_source | 1 000 | +1 000 | +0 | PASS attendu B |
| A et B | fixed match | 0 | +0 | +0 | PASS témoin |

Compteurs de blocs B, identiques dans B1, B2 et B3 :

| Phase | Blocs lus | Blocs totaux | Ratio | Cible |
|---|---:|---:|---:|---:|
| warm | 82 041 | 437 388 | 0,187570304 | `<= 0,25` |
| random | 698 943 | 3 699 535 | 0,188927257 | `<= 0,25` |
| no_source | 698 943 | 3 699 535 | 0,188927257 | `<= 0,25` |

`blocks_total > 0` et tous les ratios valent environ 18,8 % :
**PASS**.

Le compteur de segments reste à 12 avant et après toutes les phases, pour les
six runs : **PASS**.

### Défaut du rapporteur global, sans effet sur le calcul brut

`deploy/bench-local/p2-gate.sh` contient actuellement une erreur jq dans
l'objet de collecte des ratios de blocs :

```text
phase:$phase
```

`$phase` n'est pas défini dans ce filtre. Une exécution du gate global émet
donc trois erreurs de compilation jq, collecte `observations=[]`, puis marque
à tort le ratio de blocs en échec. Le sous-gate ci-dessus est néanmoins
entièrement évaluable depuis les neuf enregistrements B de
`surch.p2.phase-status.jsonl` et il est vert. Aucun correctif n'est apporté
ici, l'ownership de cette mission étant limité au présent verdict.

## 8. Parité et intégrité des réponses

Les réponses canoniques A/B sont identiques octet pour octet dans les trois
paires, pour toutes les phases chaudes :

| Phase | Lignes par run | SHA-256 canonique, identique dans les six runs |
|---|---:|---|
| warm | 200 | `1b067876bb72fa34d3a258d47181190e90d1b170fe57b28f588c061fbd8b8c13` |
| fixed | 2 000 | `4a0a79b2501a592d0ed521352fa74163eed80bb78c12edafb8868dc338b9fe94` |
| random | 2 000 | `78859632e903ad5db6a985df3f7227b0c309743d78c0ff2af6d77b237e3e66b3` |
| no_source | 2 000 | `763a74f634f986e375a10b8796b4293cbfbb67a24c4af0b6cad0bb4a0f9604c4` |

La canonicalisation retire `took` et conserve la réponse fonctionnelle
comparée par le pilote. La parité A/B chaude est donc **PASS**, avec zéro
divergence.

Cette preuve est une parité **Surch A contre Surch B** sur les corps P2. Elle
n'est ni une comparaison OpenSearch, ni une comparaison Elasticsearch, ni un
gate de pertinence BEIR.

## 9. Validité technique des runs

Les six scorecards ont :

- `measurement_valid:true` ;
- count/indexed/expected exacts à `28 917 511` ;
- `item_errors:0` ;
- quatre phases chaudes présentes et valides ;
- configuration CPU A/B identique ;
- steal chaud sous 1 % ;
- douze segments.

La phase cold est réussie pour A1, B1, B2 et B3. Elle s'arrête avant 50
requêtes pour A2 (`40/50`) et A3 (`4/50`) à cause d'un échec d'écriture de
reclaim. D'après le protocole et le code du pilote, cold est diagnostique et
optionnel : cette lacune ne rend pas les quatre phases chaudes incomplètes.
Aucun claim cold n'est produit.

## 10. Matrice formelle des critères

| Critère | État | Valeur | Preuve |
|---|---|---|---|
| Trois paires techniquement valides | PASS | 6/6 `measurement_valid`, 4 phases chaudes/run | `{run}/surch.json`, `{run}/surch.p2.phase-status.jsonl` |
| Noyau bool size:0 p95 | PASS | médiane B/A `0,1724 <= 0,50` | séries no_source bool took, `p2-report.sh` |
| Noyau bool size:0 p99 | PASS | médiane B/A `0,0462 <= 0,70` | mêmes séries |
| Produit bool size:10 p95 took | PASS | médiane B/A `0,4194 <= 0,70` | séries random bool took |
| Produit bool size:10 p95 client | PASS | médiane B/A `0,4248 <= 0,70` | séries random bool client |
| Chaque paire produit | PASS | `0,5510 / 0,4118 / 0,4194`, toutes `<= 0,80` | rapports par paire |
| IC95 primaire | PASS | hauts `0,6364 / 0,5102 / 0,6000`, tous `< 0,90` | bootstrap apparié 10 000, graine `20260726` |
| Témoin fixed match p95 took | PASS | `0,9545 / 1,0500 / 1,0500` | séries fixed match took |
| Témoin random match p95 took | **ÉCHEC** | `1,1111 / 1,2222 / 1,2222` | séries random match took |
| Confirmation témoin random client | **ÉCHEC** | `1,1864 / 1,2024 / 1,1171` | séries random match client |
| Sonde primaire | PASS | écarts absolus `0,554 / 0,601 / 0,607 ms` | séries random bool probe |
| Routage A | PASS | direct +0, generic +1 000 par phase bool mesurée | phase-status A1/A2/A3 |
| Routage B | PASS | direct +1 000, generic +0 par phase bool mesurée | phase-status B1/B2/B3 |
| Match sans compteur P2 | PASS | delta nul | phases fixed et cardinalité exacte des phases mixtes |
| Blocs P2 | PASS brut | random/no_source `0,188927 <= 0,25` | phase-status B1/B2/B3 |
| Count | PASS | `28 917 511` exact, 6/6 | scorecards |
| Segments | PASS | 12 exacts, 6/6 et par phase | scorecards et phase-status |
| Parité A/B chaude | PASS | zéro divergence, quatre SHA canoniques identiques | `cmp`, réponses canoniques |
| Cold | HORS GATE | A2/A3 incomplets | scorecards ; protocole cold optionnel |
| Fournisseur/type de disque | INÉVALUABLE | non attesté dans la copie | ne pas inférer du plan |
| Parité OpenSearch/ES | INÉVALUABLE | aucun moteur de référence dans la campagne | A/B interne seulement |
| Qualité NDCG/Recall | INÉVALUABLE | non mesurée | aucun artefact BEIR frais |

Puisque toutes les preuves obligatoires sont évaluables, `INCOMPLET` ne
s'applique pas. Puisque le témoin random obligatoire échoue, `PASS P2` ne
s'applique pas. La sémantique du gate automatisé (`paires valides` + au moins
un sous-gate requis rouge) donne **ÉCHEC P2**. La clause du plan sur le témoin
précise que cet échec annule la campagne et ne constitue pas une réfutation
du mécanisme P2.

## 11. Claim défendable pour l'état des lieux

Formulation insérable dans
`docs/paper/etat-des-lieux-benchmark-2026-07-25.md` :

> Sur une campagne A/B interne Surch au corpus deces complet
> (28 917 511 documents), cap moteur 6 Gio, douze segments et configuration
> CPU observée identique dans chaque paire (`nproc=16`, moteur CPU 0–7,
> sonde CPU 8–15), la version B, qui emprunte le parcours P2 de postings
> segmentés, présente un p95 moteur `took` inférieur sur les requêtes
> `bool.must size:10` : ratios B/A
> 0,5510 / 0,4118 / 0,4194, médiane 0,4194 (environ −58,1 %).
> Le noyau `size:0` atteint une médiane B/A de 0,1724 au p95 et 0,0462 au
> p99. Les trois IC95 bootstrap appariés ont une borne haute au plus égale à
> 0,6364 et le routage P2 ainsi que les sauts de blocs sont attestés.
> Toutefois, le gate de campagne est formellement en échec : le témoin
> `random match` régresse au p95 `took` sur 3/3 paires
> (B/A 1,1111 / 1,2222 / 1,2222), confirmé côté client. Ce résultat associé
> à B reste donc un signal A/B interne, non libérable comme claim causal P2
> sans diagnostic et nouvelle campagne au témoin vert.

Conditions à conserver avec cette formulation :

- A `961ade10...`, B `6ce390e5...`, images et digests cités plus haut ;
- mêmes bulk, mapping, manifeste et ordre de corps ;
- trois paires contrebalancées ;
- requêtes séquentielles, 1 000 bool et 1 000 match random par run ;
- `request_cache=false` selon le protocole du harnais ;
- `took` séparé du temps client et de la sonde ;
- résultat relatif à ce corpus, ces corps et cette configuration observée.

## 12. Limites et non-claims

Ne pas affirmer :

- `PASS P2` ;
- que la régression match est causée par P2 ;
- une supériorité face à Elasticsearch 8.6.1 ou OpenSearch 2.17.1 : aucun
  moteur de référence frais n'a été exécuté dans cette campagne ;
- une latence absolue transportable vers une autre machine ou un autre
  stockage ;
- que la VM est un modèle OVH précis ou que son volume est `classic`, faute
  d'attestation dans les artefacts fournis ;
- une propriété sous concurrence, charge multi-client ou NRT : les requêtes
  sont séquentielles ;
- une amélioration de NDCG@10, Recall@10 ou de la qualité de pertinence ;
- un résultat cold : A2 et A3 sont incomplets ;
- un gain RSS, disque ou indexation causal de P2 ;
- une parité OpenSearch/Elasticsearch : la parité établie est A/B interne.

Les anciens `11,5 / 58,8 / 108,1 ms` d'Elasticsearch, mesurés sous d'autres
conditions, ne peuvent pas être comparés causalement aux `26–28 ms` de p95
`took` P2 observés ici.

## 13. Prochain chantier le plus rentable

### Déblocage obligatoire avant toute nouvelle claim

Diagnostiquer d'abord la régression `random match`, parce qu'elle apparaît sur
3/3 paires, dans les deux ordres, et qu'elle est confirmée par le temps client.
La reproduction doit conserver les mêmes 1 000 corps match, vérifier de
nouveau que les compteurs P2 restent nuls, et séparer au minimum :

- coût moteur match ;
- scheduling et pression mémoire du processus ;
- cache/page-cache ;
- coût de sonde ;
- stabilité des termes random par rapport au fixed `MARTIN`.

Une campagne P2 libérable exige ensuite trois paires au témoin
`[0,95 ; 1,05]`.

### Axe d'optimisation après déblocage

Le meilleur front mesuré n'est plus le noyau P2 : B est déjà à `5 ms` de p95
sur `size:0`, contre `26–28 ms` sur `size:10`. L'écart d'environ
`21–23 ms` situe la majorité de la queue résiduelle après le noyau, dans la
finalisation produit : top-K, hydratation `_source`, sérialisation ou leur
interaction.

Le plan autorise précisément une paire profilée lorsque le gain `size:0` et
le gain `size:10` diffèrent de plus de dix points. Ici les ratios médians
`0,1724` et `0,4194` diffèrent d'environ 24,7 points. Le prochain lot le plus
rentable est donc :

1. une paire diagnostique unique avec profil `_source`, sans la compter dans
   le verdict ;
2. une décomposition actuelle du delta `size:0 → size:10` entre top-K,
   hydratation et sérialisation ;
3. seulement ensuite, optimisation du poste dominant — vraisemblablement la
   finalisation top-K/matérialisation avant tout nouveau tuning du curseur P2.

Le coût de sonde p95 étant autour de `1,5 ms` côté B et stable à moins de
`0,7 ms` entre variantes, l'optimiser ne peut pas expliquer ni récupérer la
queue résiduelle de `21–23 ms`.

## 14. Traçabilité de l'analyse

Contrôles effectués sans build ni test Rust :

- lecture de `AGENTS.md`, `PLAN.md`, `plan/p2-segmented-postings.md`,
  `.remote/p2-mesure-plan.md`, `.remote/p2-verdict.md`,
  `deploy/bench-local/p2-report.sh`,
  `deploy/bench-local/p2-gate.sh`,
  `deploy/bench-local/p2-campaign.sh`,
  `docs/paper/etat-des-lieux-benchmark-2026-07-25.md` et du ledger Track A ;
- `git status --short --branch`, worktrees et branches ;
- dernier `ci` main : run `30311962457`, succès sur `987d83a` ;
- dernier `ci-k8s` vert observé : `28689787902` sur `69668db` ; il ne prouve
  pas cette campagne locale ;
- trois exécutions de `p2-report.sh` à 10 000 rééchantillonnages ;
- inspection `jq` des six scorecards, stats et phase-status ;
- comparaison `cmp` et SHA-256 des réponses canoniques chaudes ;
- recalcul des ratios, médianes, différences de sonde et ratios de blocs ;
- reproduction en répertoire temporaire du défaut d'extraction de blocs de
  `p2-gate.sh`.

Aucun `cargo build`, `cargo check`, `cargo test` ou `cargo clippy` n'a été
exécuté. Aucun code moteur, plan ou autre fichier du dépôt n'a été modifié.

VERDICT_COMPLET_DONE
