# P2 — diagnostic de la régression du témoin `random match`

Date : 2026-07-28  
Référence A : `961ade10ffb74d78156aee8148f1e5c6bbbe6ba2`  
Référence B : `6ce390e55da3593242ec11e2b09d4dee1057726d`  
Artefacts : `/home/antoinefa/.cache/fair-ab/p2-campagne-2026-07-28/`

## Verdict

Le témoin rouge n'est pas expliqué par un nouveau contrôle d'invariant exécuté
directement dans le chemin `match` simple. Dans B final, ce chemin appelle
toujours les wrappers historiques, non checked, et son top-K est inchangé.

Deux mécanismes, qui peuvent se cumuler, sont en revanche prouvés :

1. **P2 ajoute un coût mémoire partagé massif et réel.** B conserve
   `1 923 190 484` octets d'attestation de répertoires de postings en mémoire,
   soit `1,791 GiB` et `29,85 %` du quota de `6 GiB`. Le RSS processus augmente
   en moyenne de `1,817 GiB` et le cache fichier du cgroup diminue en moyenne
   de `1,574 GiB`. Ce coût affecte potentiellement toute requête, dont
   `match`, même si son chemin d'instructions n'appelle pas P2.
2. **Le témoin `random match` n'est pas indépendant de la requête P2.** Chaque
   `match NOM=x` suit immédiatement un `bool.must NOM=x AND PRENOMS=y`.
   A matérialise complètement les postings des deux termes dans son chemin
   générique multi-segment ; B ne lit qu'environ `18,9 %` des blocs avec P2.
   A préchauffe donc exactement le posting `NOM=x` que le témoin suivant va
   relire, beaucoup plus que B. Le protocole donne ainsi un avantage de cache à
   A juste avant chaque observation du témoin.

Les artefacts actuels ne séparent pas la part de ces deux mécanismes : le profil
`_source` est désactivé et les runs ne conservent ni
`workingset_refault_file`, ni fautes majeures, ni `io.stat`, ni PSI. La
conclusion rigoureuse n'est donc ni « bruit pur », ni « taxe CPU directe de
P2 », mais :

> **B porte un surcoût mémoire P2 réel, et le témoin est contaminé par un
> carry-over de la requête P2 précédente. Le partage de responsabilité entre
> pression mémoire et séquencement ne peut pas être identifié sans une paire
> discriminante.**

Le lot S constitue en outre un confondant de construction : contrairement à
l'hypothèse « S est dans les deux images », il est absent de A et présent dans
B. L'audit du code final écarte son coût checked comme cause CPU directe du
`match`, mais la campagne A/B n'isole formellement pas P2 du lot S.

## 1. Périmètre vérifié

Le verdict de campagne atteste :

- le gain primaire `bool.must size:10`, p95 `took`, B/A
  `0,5510 / 0,4118 / 0,4194`, médiane `0,4194` (`−58,1 %`) :
  `docs/paper/p2-verdict-campagne-2026-07-28.md:11-19` ;
- le témoin `random match`, p95 `took`, B/A
  `1,1111 / 1,2222 / 1,2222`, et client
  `1,1864 / 1,2024 / 1,1171` :
  `docs/paper/p2-verdict-campagne-2026-07-28.md:21-25` et `:171-187` ;
- les mêmes corpus, mapping, manifeste, 12 segments, quota `6g`, CPU et
  connexion chaude sur les six runs :
  `docs/paper/p2-verdict-campagne-2026-07-28.md:64-79` ;
- le routage P2 effectif uniquement pour les 1 000 bool et un ratio de blocs
  lus de `0,188927257` :
  `docs/paper/p2-verdict-campagne-2026-07-28.md:210-236` ;
- la parité fonctionnelle A/B exacte pour toutes les réponses chaudes :
  `docs/paper/p2-verdict-campagne-2026-07-28.md:254-268`.

Le diff analysé est exactement `git diff 961ade1..6ce390e`. Il contient
`2 454` insertions et `380` suppressions dans 11 fichiers. Aucun
`Cargo.toml`, `Cargo.lock`, `Dockerfile`, manifest de déploiement ou workflow
CI n'est modifié entre A et B. Les changements de production sont concentrés
dans :

- `crates/surch-api/src/search.rs` ;
- `crates/surch-api/src/state.rs` ;
- `crates/surch-index/src/document_index.rs` ;
- `crates/surch-index/src/memory.rs` ;
- `crates/surch-index/src/postings.rs`.

L'image est compilée en release par `cargo build --release --locked`
(`Dockerfile:38-41`). Les `debug_assert!` de construction et de round-trip ne
sont donc pas une taxe de requête dans ces images.

## 2. Le lot S n'est pas dans les deux images

L'historique entre A et B est :

```text
e0fe2d7 [lecture S] distinguer absence et erreur des postings
8ba1131 [lecture S] corriger les lectures de postings vérifiées
3b2c172 [lecture S] corriger les invariants de lecture
03c11fd [lecture S] compléter la lecture RAM vérifiée
cb0ada8 [lecture S] préserver le compteur P1a au cache
1f57e3b [latence P2] parcourir les postings checked par segment
0c6a9e5 [latence P2] corriger les gardes de robustesse
3d33d8d [latence P2] verrouiller les lectures de postings
3a0b2d5 [latence P2] rétablir l'import du curseur de postings
55d3bcf [latence P2] typer le compteur de fixture
6ce390e [latence P2] correction clippy
```

`git merge-base --is-ancestor` donne :

```text
commit S    ancêtre de 961ade1    ancêtre de 6ce390e
e0fe2d7     non                    oui
cb0ada8     non                    oui
```

Le premier état S redirigeait effectivement les wrappers historiques vers les
variantes checked :

- `e0fe2d7:crates/surch-index/src/postings.rs:2437-2439` pour
  `decode_from_segment` ;
- `e0fe2d7:crates/surch-index/src/postings.rs:2511-2513` pour `disk_cursor`.

Mais `3b2c172` restaure les implémentations historiques :

- `3b2c172:crates/surch-index/src/postings.rs:2545-2554` ;
- `3b2c172:crates/surch-index/src/postings.rs:2588-2612`.

Ces corps restaurés sont encore ceux de B final :

- `6ce390e:crates/surch-index/src/postings.rs:2709-2717` ;
- `6ce390e:crates/surch-index/src/postings.rs:2748-2775`.

La variante checked est distincte et commence seulement à
`6ce390e:crates/surch-index/src/postings.rs:2778`. Son commentaire dit
explicitement que cette séparation évite de modifier le coût et la surface
d'erreur du wrapper historique (`:2748-2751`, `:2778-2781`).

Conclusion : **S est un confondant de révision, mais pas la taxe CPU directe
observée dans le `match` final.** Une mesure causale P2 devra néanmoins mettre S
des deux côtés, par exemple avec `cb0ada8` comme base S-only.

## 3. Audit du chemin `match` simple

### 3.1 Chemin final inchangé

Le top-K `match` est textuellement identique entre :

- A : `961ade1:crates/surch-api/src/search.rs:2367-2469` ;
- B : `6ce390e:crates/surch-api/src/search.rs:2430-2532`.

Dans B :

1. `topk_scored_documents_inner` appelle `match_hits_internal` :
   `crates/surch-api/src/search.rs:2449-2460` ;
2. le chemin disque appelle `match_hits_disk` :
   `crates/surch-api/src/state.rs:2875-2890` ;
3. un `match` mono-token matérialise les postings complets via
   `DocumentIndex::decode_from_segment` :
   `crates/surch-api/src/state.rs:2942-2952` ;
4. le contexte de scoring disque redécode les postings complets du terme dans
   l'arène :
   `crates/surch-api/src/search.rs:3363-3388` ;
5. `DocumentIndex::decode_from_segment` fusionne tous les segments avec le
   wrapper historique :
   `crates/surch-index/src/document_index.rs:2339-2383` ;
6. `size:10` hydrate les `_source` des gagnants :
   `crates/surch-api/src/search.rs:2532-2568`.

À l'inverse, le contrôle `size:0` retourne avant scoring final et hydratation :
`crates/surch-api/src/search.rs:2478-2481`.

La lecture checked dédiée P2 se trouve dans une autre API,
`disk_cursor_p2_checked`, et compare l'attestation avant d'ouvrir le curseur :
`crates/surch-index/src/postings.rs:2820-2896`. Le routeur qui l'emploie est
limité au root `Bool` ou `FunctionScore`, puis à une conjonction `must`
réductible : `crates/surch-api/src/search.rs:2055-2073`.

### 3.2 Petits changements communs, non explicatifs

B ajoute un booléen interne `direct_must_fused` dans `SearchResponse`
(`crates/surch-api/src/search.rs:443-465`) et dans l'entrée du cache. Ce champ
est ignoré à la sérialisation. Le harnais appelle en outre
`_search?request_cache=false` (`deploy/bench-local/fair-ab.sh:872-893`) ;
`search_handler` rend alors la requête inéligible au cache
(`crates/surch-api/src/search.rs:868-881`).

Ce changement peut modifier marginalement un layout de structure ou le codegen,
mais il n'explique pas de façon crédible un déplacement répété de 1 à 3 ms.
Aucune donnée de profil ne lui attribue du temps.

## 4. Coût mémoire P2 partagé : preuve et magnitude

### 4.1 Structure introduite

P2 clone le répertoire de blocs avant son spill :

- construction initiale :
  `crates/surch-index/src/postings.rs:941-953` ;
- insertion des clones et payloads P2 dans `FieldPostings` :
  `crates/surch-index/src/postings.rs:965-982` ;
- même duplication lors d'un merge :
  `crates/surch-index/src/postings.rs:1523-1559`.

Les champs sont documentés comme restant résidents même lorsque le répertoire
de service est déversé :

- `p2_block_directory` ;
- `p2_block_dir_offsets` ;
- `p2_term_payloads` ;

à `crates/surch-index/src/postings.rs:1924-1935`.

Leur comptabilisation est explicite à
`crates/surch-index/src/postings.rs:2975-3016`, en particulier `:3012-3014`.
`git blame` attribue les deux premiers canaux à `0c6a9e5` et le payload par
terme à `3d33d8d`.

Le compteur redondant `postings_counts` ajouté par S
(`crates/surch-index/src/postings.rs:617-623`, `:1807-1814`) n'est pas retenu
en mode disque : il a une capacité nulle et n'est alimenté que dans la branche
RAM (`:691-756`, `:1417-1428`). Ce n'est donc pas l'explication des presque
2 GiB de B sur cette campagne.

### 4.2 Mesures sur les trois paires

Les snapshots `surch.p2.prom.index_ready.prom` donnent exactement :

```text
Métrique                               A, chaque run       B, chaque run
surch_index_postings_directory_bytes  0                   1 923 190 484
surch_index_total_bytes                2 482 870 160       4 406 060 644
Delta total comptabilisé                                   1 923 190 484
```

La hausse de `surch_index_total_bytes` est donc exactement égale au nouveau
répertoire P2.

```text
Paire   RSS processus B-A   cgroup anon B-A   cgroup file B-A
1       +1 947 004 928      +1 946 791 936     -1 686 597 632
2       +1 953 071 104      +1 906 929 664     -1 647 013 888
3       +1 952 759 808      +1 955 295 232     -1 737 723 904
Moy.    +1 950 945 280      +1 936 338 944     -1 690 445 141
```

En unités binaires, la moyenne est :

- RSS processus : `+1,817 GiB` ;
- cache fichier cgroup : `−1,574 GiB`.

Le RSS augmente de seulement `26,5 MiB` de plus que la taille comptabilisée de
l'attestation. La correspondance de magnitude est trop proche pour être
fortuite.

`surch_jemalloc_allocated_bytes` passe en moyenne de `3 047 466 693` à
`5 199 981 168` octets, soit `+2 152 514 475` octets. Le résidu par rapport à
la jauge P2 est environ `218,7 MiB`; les artefacts ne permettent pas de
l'attribuer proprement à un canal, à la fragmentation ou à un temporaire
retenu.

### 4.3 Pourquoi cela peut toucher seulement le témoin aléatoire

Le `match size:10` :

- lit et décode entièrement le posting aléatoire pour les candidats ;
- le redécode pour le scoring ;
- hydrate dix `_source` potentiellement dispersés.

Il dépend donc du page cache postings et `_source`. Une réduction d'environ
`1,57 GiB` du cache fichier disponible est une cause crédible d'une queue plus
longue.

Les observations sont cohérentes avec cette hypothèse :

- le témoin fixe `MARTIN`, fortement réutilisé, reste dans le gate sur les
  trois paires (`0,9545 / 1,0500 / 1,0500`) ;
- le `random match size:0`, sans scoring final ni hydratation, a des p95 client
  B/A mixtes (`0,8998 / 1,2404 / 0,9421`), pas trois régressions concordantes ;
- le témoin `size:10` aléatoire, qui change de working set, est rouge trois
  fois.

Il manque cependant les refaults et l'I/O par phase pour transformer cette
cohérence en attribution causale complète.

## 5. Contamination du témoin par la requête précédente

Le harnais construit chaque paire dans cet ordre :

```text
bool.must NOM=x AND PRENOMS=y
match NOM=x
```

La preuve est dans `deploy/bench-local/fair-ab.sh:507-523`, notamment :

- même `$2` (`NOM`) dans `bool10` et `match10` à `:518` ;
- écriture bool puis match à `:519` ;
- positions impaires bool, paires match à `:521`.

Le rapport ne fait que séparer a posteriori ces positions
(`deploy/bench-local/fair-ab.sh:1200-1218`). Cela ne supprime pas leur état de
cache partagé.

Dans A :

- P1a décline volontairement le disque/multi-segment :
  `961ade1:crates/surch-api/src/state.rs:4647-4653` ;
- le chemin générique résout la conjonction multi-segment en décodant
  complètement chaque terme :
  `961ade1:crates/surch-api/src/state.rs:3023-3028` et `:3064-3090` ;
- le contexte de scoring redécode encore les postings complets :
  `961ade1:crates/surch-api/src/search.rs:3300-3325`.

Dans B :

- P2 prend le chemin direct :
  `6ce390e:crates/surch-api/src/search.rs:2070-2085` ;
- il ouvre des curseurs checked segmentés et ne charge que les blocs
  nécessaires ;
- les compteurs observés indiquent environ `18,9 %` de blocs lus, contre une
  matérialisation intégrale en A.

Le `match NOM=x` suivant retrouve donc en A un posting `NOM=x` intégralement
touché, alors que B n'en a probablement touché qu'une fraction. Le « témoin »
mesure autant le carry-over du bool précédent que son propre chemin.

Cette contamination est un **artefact pour une claim de non-régression du
chemin `match` autonome**. Elle peut rester un signal produit valide si le
trafic réel alterne précisément ces formes et ces mêmes termes, mais ce n'est
alors plus un témoin négatif : c'est un workload mixte.

## 6. Forme statistique de la régression

Les distributions client du `random match` sont :

```text
Run  moyenne ms   p95 ms   p99 ms   max ms
A1      4,938       9,56    15,22     30,95
B1      6,510      11,34    23,43    500,36
A2      6,033      10,23    23,39    392,20
B2      5,975      12,30    23,03     56,63
A3      6,246      10,42    22,42    239,06
B3      5,784      11,64    21,09     41,25
```

La médiane des différences appariées B−A est
`+0,366 / +0,292 / +0,155 ms`. B est plus lent sur
`680 / 675 / 581` requêtes sur 1 000, et le p95 des différences appariées vaut
`+3,280 / +3,175 / +3,501 ms`.

Le signal client est donc réel et large ; l'arrondi entier de `took` amplifie
`9→10/11 ms`, mais ne crée pas le phénomène.

En revanche :

- la moyenne B n'est pire que dans la paire 1 ;
- le p99 B est égal ou meilleur dans les paires 2 et 3 ;
- A porte les plus gros maxima dans les paires 2 et 3.

Ce n'est donc pas la signature d'une taxe CPU uniforme de `+11–22 %` sur
chaque `match`. C'est un déplacement de forme de distribution autour du p95,
compatible avec cache/page faults et avec le carry-over du protocole.

## 7. Causes candidates classées

### 1 — Très vraisemblable : pression mémoire P2 et éviction du page cache

**Type : vrai surcoût P2 partagé.**

Preuve forte :

- structure créée par les commits P2 ;
- `1 923 190 484` octets exactement comptabilisés dans B et zéro dans A ;
- delta RSS de même magnitude ;
- perte moyenne de `1,690 Go` de cache fichier ;
- forme aléatoire `size:10` rouge, forme fixe chaude verte.

Ce coût doit être inclus dans le bilan P2, même si une partie de la régression
du témoin disparaît après isolation.

### 2 — Très vraisemblable : avantage de préchauffage donné à A

**Type : artefact du témoin pour une mesure autonome ; possible effet réel
d'un workload séquentiel.**

Preuve forte :

- alternance déterministe bool puis match ;
- même terme `NOM` dans les deux requêtes ;
- A matérialise tout le posting ;
- B ne lit qu'environ 18,9 % des blocs.

La magnitude exacte n'est pas identifiable dans les runs existants.

### 3 — Faible comme cause directe : lot S checked

**Type : confondant de version, coût direct écarté.**

S est seulement dans B, mais `match` appelle les wrappers historiques restaurés
dès `3b2c172`. Les contrôles checked de S/P2 ne sont pas exécutés par ce chemin
final. Les `debug_assert!` de build sont absents de l'image release.

### 4 — Très faible : layout de `SearchResponse`, cache, codegen

B ajoute un booléen interne commun, mais le cache est désactivé et le champ
n'est pas sérialisé. Un changement de placement du binaire reste
théoriquement possible, sans preuve et sans magnitude crédible face aux
`1,923 Go` mesurés.

### 5 — Faible : dérive hôte, CPU ou stockage

La fréquence CPU, PSI et les fautes de page ne sont pas conservées, donc cette
famille n'est pas réfutable à 100 %. Elle est toutefois affaiblie par :

- l'ordre contrebalancé A1→B1, B2→A2, A3→B3 ;
- un steal maximal inférieur à `0,01 %` ;
- le même corpus, nombre de segments, CPU sets et quota ;
- un témoin fixe vert.

### Causes écartées

- différence fonctionnelle : réponses canoniques identiques ;
- cache de réponses HTTP : `request_cache=false` ;
- surcoût de sonde : signal confirmé par `took`, et gate de sonde vert ;
- changement de corpus, mapping ou segmentation : SHA et compteurs identiques ;
- contrôle checked exécuté directement par `match` : appel final au wrapper
  historique.

## 8. Bilan global selon le mix de trafic

### 8.1 Mesure directe disponible : mix 50/50

La série random complète est exactement 50 % bool et 50 % match. Son p95
client mesuré, sans moyenne artificielle de quantiles, donne :

```text
Paire   A p95 ms   B p95 ms   B/A      gain
1       35,372     23,825      0,6736   -32,6 %
2       40,942     23,546      0,5751   -42,5 %
3       40,510     23,158      0,5717   -42,8 %
Médiane                         0,5751   -42,5 %
```

Même avec le témoin rouge, le workload exact 50/50 de la campagne a donc une
queue globale nettement meilleure.

### 8.2 Repondération empirique des distributions

Pour estimer d'autres proportions, les 1 000 échantillons bool et les 1 000
échantillons match de chaque run ont été repondérés dans une CDF empirique :
poids `q/1000` pour chaque bool et `(1-q)/1000` pour chaque match, quantile au
rang le plus proche, puis ratio B/A par paire et médiane des trois ratios.

```text
Part bool q   Ratio p95 client médian B/A   bilan
0 %           1,1864                         +18,6 %
1 %           1,2217                         +22,2 %
5 %           1,0334                          +3,3 %
5,6 %         0,9905                          -0,9 %
10 %          0,8422                         -15,8 %
25 %          0,6572                         -34,3 %
50 %          0,5751                         -42,5 %
75 %          0,5304                         -47,0 %
90 %          0,4841                         -51,6 %
100 %         0,4248                         -57,5 %
```

Dans cette repondération :

- la médiane inter-paires passe sous `1,0` vers `5,6 %` de bool ;
- les trois paires passent sous `1,0` vers `8,3 %` de bool.

Ces seuils sont bas parce qu'un bool A coûte environ six fois le p95 d'un match
A. Ils sont **conditionnels aux distributions contaminées observées** et ne
remplacent pas un replay de trafic réel.

La moyenne raconte un signal moins défavorable au `match` : ses ratios B/A par
paire sont `1,318 / 0,990 / 0,926`, médiane `0,990`. La perte est donc avant
tout une perte de queue p95, pas une hausse moyenne systématique.

### 8.3 Conséquence pour une claim

Si la paire autonome confirme un vrai surcoût `match`, la claim doit donner :

- le gain bool ;
- la perte match ;
- le mix de trafic auquel le bilan est calculé ;
- les `+1 923 190 484` octets résidents ;
- une distribution ou un replay, pas une moyenne pondérée naïve de p95.

Une phrase « P2 gagne 58,1 % » sans ces éléments ne serait pas défendable.

## 9. Expérience discriminante la plus économe

Ne pas relancer immédiatement trois paires identiques : le même témoin
contaminé laisserait la même ambiguïté.

Faire d'abord **une seule paire diagnostique** avec les images existantes, le
même corpus, 12 segments, quota `6 GiB`, CPU sets et 1 000 noms :

1. exécuter `random match size:10` dans une phase autonome, sans bool
   intercalé ;
2. exécuter son jumeau `size:0` ;
3. soit utiliser des conteneurs frais par phase, soit contrebalancer
   `match→bool` et `bool→match`, sans réutiliser le même `NOM` immédiatement ;
4. activer le profil `_source` ;
5. capturer avant/après chaque phase :
   - `memory.stat`: `anon`, `file`, `workingset_refault_file`,
     `workingset_activate_file`, `pgmajfault` ;
   - `io.stat` ;
   - PSI mémoire/IO et throttling CPU ;
   - RSS processus et `surch_index_postings_directory_bytes` ;
   - `took`, client et probe ;
6. en ablation diagnostique seulement, donner environ `2 GiB` de headroom
   supplémentaire à B. Une disparition de la régression avec ce headroom
   pointerait fortement vers la pression mémoire ; ce run ne doit pas servir de
   claim A/B formelle parce que les quotas diffèrent.

Interprétation :

```text
Match autonome vert
  => artefact de séquence démontré pour le témoin.
  => garder séparément le résultat du workload mixte.
  => le coût mémoire P2 reste réel et doit être publié ou corrigé.

Match autonome rouge + refaults/IO B plus élevés
  => surcoût collatéral mémoire P2 démontré.
  => compacter/supprimer la duplication d'attestation, puis re-mesurer.

Match autonome rouge sans signal mémoire/IO
  => mesurer S-only puis bisecter cb0ada8, 1f57e3b, 0c6a9e5, 3d33d8d
     avec profil CPU/cache.
```

## 10. Recommandation de libération de claim

La marche à suivre la plus économique est :

1. **une paire discriminante match-only avec télémétrie mémoire/IO** ;
2. **si elle reste rouge, corriger la représentation résidente avant toute
   campagne complète** ;
3. **pour la campagne causale finale, mettre S des deux côtés**, avec une base
   S-only telle que `cb0ada8`, puis faire les trois paires contrebalancées ;
4. **isoler le témoin match du bool** et publier séparément un replay mixte.

Une simple reformulation sans nouvelle paire n'est pas recommandée : une
structure P2 qui consomme `29,85 %` du quota mémoire est un coût produit trop
important pour être traité comme bruit.

Si aucune correction ni nouvelle mesure n'est faite, la seule claim honnête
est descriptive et étroite :

> Sur cette campagne interne, P2 réduit le p95 `bool.must size:10` de `58,1 %`
> côté moteur et `57,5 %` côté client. Le mix 50/50 réduit le p95 client médian
> de `42,5 %`. En contrepartie, le `random match` perd `11–22 %` au p95 moteur
> et `11,7–20,2 %` au p95 client, tandis que B conserve `1,923 Go`
> (`1,791 GiB`) de métadonnées supplémentaires. Le témoin étant séquencé juste
> après un bool portant le même terme, la causalité entre pression mémoire et
> carry-over reste à isoler.

TEMOIN_DIAG_DONE
