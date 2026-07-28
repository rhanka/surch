# P3 — campagne décisive

## Décision expérimentale

Variantes figées :

- **A** `961ade10ffb74d78156aee8148f1e5c6bbbe6ba2` : avant P2 ;
- **B** `6ce390e55da3593242ec11e2b09d4dee1057726d` : P2, attestation
  résidente de `1 923 190 484` octets ;
- **C** `d0accd6e4809bc7340a6cd55cef0a94fcb6c062d` : P2 + attestation P3
  BLAKE3 par pages.

**A/C suffit pour décider si C est livrable**, car ce contraste teste les trois
résultats produit : mémoire RSS/RssAnon revenue près de A, gain `bool.must`
produit conservé et témoin autonome non régressé. Il ne suffit pas pour
attribuer les effets ni pour affirmer que le gain historique précis est
conservé.
La campagne finale emploie donc **les trois variantes dans les mêmes
triplets**, sans campagnes de paires séparées :

- **A/B** rejoue P2 avec le témoin corrigé et la télémétrie manquante ; il
  sépare la vraie pression mémoire de l'ancien carry-over ;
- **B/C** isole l'effet net de P3 : mémoire récupérée et coût du hachage ;
- **A/C** porte seul le verdict final et la claim publiable.

Les anciens runs A/B restent un contrôle historique, pas une répétition :
le témoin et la télémétrie changent. Ordre latin pré-engagé, avec C en premier
pour permettre l'arrêt mémoire le moins cher :
`C1-A1-B1`, `A2-B2-C2`, `B3-C3-A3`. Chaque run sert aux trois contrastes ;
il faut neuf runs, pas dix-huit.

## Témoin réellement indépendant

Modifier le harnais benchmark, avant tout full :

1. sélectionner déterministement trois ensembles de termes mono-token :
   `1 000` couples `(NOM, PRENOMS)` pour les bool, `1 000` `NOM` **uniques**
   pour le témoin match et `200` termes de chauffe ; les trois ensembles sont
   disjoints sur `NOM` ;
2. produire et geler séparément, avec SHA-256 dans le manifeste :
   `bool-size10`, `bool-size0`, `match-control-size10` et les chauffes ;
3. après indexation, chauffer uniquement le chemin match avec les `200` termes
   tiers, puis exécuter **avant tout bool** la phase autonome de
   `1 000 match NOM=x size:10` ;
4. chauffer ensuite le bool sur des termes tiers, puis exécuter les
   `1 000 bool.must size:10` et `1 000 bool.must size:0` ;
5. supprimer l'alternance `bool NOM=x` puis `match NOM=x`. Garder l'ancien mix
   50/50 seulement comme replay produit séparé, jamais comme témoin causal.

Le témoin autonome exige `request_cache=false`, `hits.total.value > 0`, zéro
delta des compteurs P2 et zéro divergence canonique A/B/C. Le témoin fixe
`MARTIN` reste un contrôle secondaire. Un éventuel `match size:0` utilise un
quatrième ensemble disjoint ou reste diagnostique : il ne doit pas être
préchauffé par son jumeau `size:10`.

## Mesures à conserver

À `index_ready`, puis avant/après chaque phase chaude :

- latence, séparée par forme : client, `took` et `client-took`,
  p50/p95/p99/max, séries brutes, ratios par triplet et bootstrap apparié
  `10 000` tirages ;
- routage/parité : direct, generic, blocs lus/totaux, segments, réponses
  canoniques, count et steal CPU ;
- P3 :
  `surch_postings_p2_integrity_{bytes,pages}`,
  `verified_bytes`, `hash_failures`, `fallbacks`, `fallback_fields`,
  `term_occurrences`, `blocks`, `fields`, `term_payload_bytes`, `csr_bytes`
  et `directory_bytes` ;
- mémoire moteur :
  `surch_index_postings_directory_bytes`, `surch_index_total_bytes`,
  RSS/RssAnon/VmHWM et jemalloc allocated/active/resident/retained ;
- cgroup/hôte : `memory.current`; dans `memory.stat`, `anon`, `file`,
  `workingset_refault_file`, `workingset_activate_file`, `pgmajfault` ;
  deltas `io.stat`, PSI mémoire/IO et `cpu.stat` (`nr_throttled`,
  `throttled_usec`).

Dérivés obligatoires par triplet :

- compaction `directory_bytes(C)/directory_bytes(B)` ;
- récupération RSS
  `(RSS_B-RSS_C)/(RSS_B-RSS_A)` ;
- récupération cache fichier
  `(file_C-file_B)/(file_A-file_B)` ;
- mêmes calculs sur RssAnon ;
- refaults, lectures disque et PSI **de la seule phase match autonome**.

### Ratification M3 — protocole mémoire réduit

La campagne conserve les quatre jauges jemalloc brutes comme **diagnostic**,
mais ne décide plus sur `jemalloc resident` et ne revendique aucune fraîcheur
atomique de ces jauges pour le pin C. La claim mémoire publiable est réduite à
la compaction de `directory_bytes` et au retour **vu du noyau** de RSS,
RssAnon et cache fichier. Elle ne permet pas de conclure sur les allocations
vivantes, la fragmentation, les pages retained/dirty ou le retour de
`jemalloc resident`. Cette réduction de portée est volontaire et visible : un
repin vers une image qui prétendrait rétablir une claim jemalloc exige un
protocole de jauges fraîches, atomiques et concurrentiellement prouvées.

## Gate pré-engagé après trois triplets valides

Validité technique : mêmes images construites dans la même session, mêmes
SHA-256 d'entrées, CPU/quota `6 Gio`, exactement `28 917 511` documents et
`12` segments, steal `<= 1 %`, couverture complète des phases et zéro
divergence canonique A/B/C. Une absence de métrique, un delta de routage
partiel ou une parité rouge donne **INVALIDE**, pas un résultat de performance.

**PASS P3** seulement si tous les critères suivants sont vrais :

1. **Mémoire / intégrité**
   - chaque C : `0 < integrity_bytes <= 17 825 792` (`17 Mio`),
     `hash_failures=0`, `fallbacks=0`, `fallback_fields=0` ;
   - `verified_bytes` augmente sur chaque phase bool C et reste stable pendant
     le témoin match ;
   - chaque `directory_bytes(C)/directory_bytes(B) <= 0,0100`
     (réduction d'au moins `99,0 %`) ;
   - récupération médiane RSS, RssAnon et cache fichier `>= 90 %`, aucune
     répétition `< 80 %`.
2. **Gain P2 conservé, C/A**
   - bool `size:0` : médiane p95 `took <= 0,50`, p99 `<= 0,70` ;
   - bool `size:10` : médiane p95 `took <= 0,70` et client `<= 0,70` ;
     chacune des trois répétitions `took <= 0,80` ;
   - borne haute de chaque IC95 bootstrap p95 `took size:10 < 0,90`.
   - **conservation du gain historique** (rapportée séparément, sans modifier
     le verdict produit) : médiane C/A p95 `took size:10 <= 0,50` et borne
     haute de chaque IC95 bootstrap `<= 0,50`. Le rapport porte explicitement
     `CONSERVE` ou `NON CONSERVE`; `NON CONSERVE` avec le critère produit vert
     reste une information publiable, jamais une réussite silencieuse.
3. **Coût propre de P3, C/B**
   - bool `size:10` et `size:0` : médiane p95 `took <= 1,05` et aucune
     répétition `> 1,10`.
4. **Témoins**
   - match autonome `size:10`, C/A : p95 `took` de chaque répétition dans
     `[0,95 ; 1,05]`; p95 client médian `<= 1,05` et aucun `> 1,10` ;
   - fixed match : p95 `took` de chaque répétition dans `[0,95 ; 1,05]` ;
   - écart de sonde p95 C-A en valeur absolue `<= 2 ms`.
5. **Chemin exercé**
   - B et C : `direct +1000`, `generic +0`, `blocks_total > 0`,
     `blocks_read/blocks_total <= 0,25` par phase bool ;
   - A : `direct +0`, `generic +1000` ; match : delta P2 nul.

Le plafond logiciel `32 Mio` reste une sécurité : `17 < integrity_bytes <=
32 Mio` est une campagne valide mais un **ÉCHEC de la cible P3**. Plus de
`32 Mio`, un fallback ou un hash failure rend C techniquement invalide.

Avec neuf runs techniquement valides, toute violation d'un seuil PASS donne
**ÉCHEC P3**. En particulier :

- médiane p95 `took size:10` C/A `> 0,70` : moins de `30 %` de gain produit,
  estimation réfutée, aucun nouveau tuning avant décision ;
- médiane ou IC95 historique C/A `size:10 > 0,50` : verdict produit inchangé,
  mais le résumé doit publier **NON CONSERVE** pour signaler la régression par
  rapport au ratio historique `0,42` ;
- témoin autonome C/A p95 `took > 1,05` ou client médian `> 1,05` :
  régression globale non résolue ;
- récupération mémoire médiane `< 90 %`, compaction `> 1,00 %` ou
  `integrity_bytes > 17 Mio` : coût mémoire P2 non fermé ;
- médiane C/B `> 1,05` : P3 consomme plus de `5 %` du p95 moteur.

Un ratio témoin `< 0,95` n'est pas une régression, mais casse le contrôle de
comparabilité : vérifier télémétrie et ordre, puis rejouer la répétition
concernée au lieu de créditer ce gain à P3.

## Coût et arrêts anticipés

1. **Smoke 1,36 M, A/B/C** : trois runs, `100` bool et `100` match autonomes,
   au moins trois segments. Il tranche scripts, disjonction des termes,
   manifeste, routage, parité, métriques, hash/fallback et cohérence de la
   formule d'extrapolation ; il ne tranche ni p95 ni RSS full. Budget :
   environ `30–60 min`, hors builds.
2. **C1 full d'abord** : arrêt après environ `50 min` si count/segments,
   parité interne, fallback/hash ou `integrity_bytes <= 17 Mio` échouent.
3. **Premier triplet C1-A1-B1** : il compte comme répétition 1. Arrêt de
   présélection après environ `2 h 30` de runs si C/A produit `> 0,80`,
   C/B `> 1,10`, témoin C/A `> 1,10` ou récupération mémoire `< 80 %`.
   Ces seuils sont des hard-stops de coût ; aucun PASS n'est publié sur une
   seule répétition.
4. **Campagne finale** : si le triplet passe, ajouter les six runs restants.
   Total full : `9 × 50 min = 450 min = 7 h 30`. Ajouter environ `45 min`
   de récupération hôte (5 min entre runs), puis builds/préflight/smoke :
   réserver **9–10 VM-heures**.

Le mode A/C seul coûterait six runs, soit `5 h`, mais économiserait seulement
`2 h 30` en supprimant précisément les contrastes qui distinguent pression
mémoire, contamination du témoin et coût BLAKE3. Il n'est donc retenu que pour
une simple décision de livraison, pas pour la campagne causale demandée.

PLAN_C_DONE
