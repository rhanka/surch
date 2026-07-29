# D2 / C5 — Postings bit-packés et fréquences constantes omises

Chantier **C5 / D2** du programme R&D (`.remote/rd-latence-programme.md`, §C5 ligne 393 et
§D2 ligne 501). Base : `main` @ `ae2c12d` (CI verte). Périmètre : `crates/surch-codec`,
`crates/surch-index`, et le câblage de jauges dans `crates/surch-api`.
Aucun fichier de `deploy/` ni de `.github/` n'a été touché.

---

## Synthèse en dix lignes

Le diagnostic R&D est **exact sur le mécanisme** : `encode_postings_blocked` écrivait bien un
varint LEB128 par delta ET un varint par fréquence, sans aucun bit-packing malgré le nom
`FOR_BLOCK_SIZE`. Il est **faux sur le chiffrage**, et l'écart est structurel, pas marginal :
la mesure sur le corpus réel montre que **seuls 50 % des 4 598 Mo de « postings » sont du
payload de postings** — 36 % sont la table `TermEntry` (28 o/terme) et 14 % le répertoire de
blocs (10 o/bloc). Le gain de −2 100 Mio annoncé supposait 100 % de payload : il n'est pas
atteignable par un changement de codec. Deuxième correction, mesurée elle aussi : sur ce
corpus un frame-of-reference **naïf est PLUS GROS que le varint** (+0,7 %), parce que les
listes sont groupées (runs denses + sauts énormes) et que le bloc moyen ne contient que 13
postings. Ce qui gagne vraiment, c'est **l'omission du canal des fréquences** : `tf = 1` sur
**99,95 %** des blocs. Gain total calculé : **−982 Mo (−937 Mio), soit −21,4 %** du segment.

---

## 1. Vérification de l'encodage actuel, fichier:ligne

Toutes les lignes ci-dessous sont celles de `ae2c12d`, **avant** ce commit.

### 1.1 « varint LEB128, pas de frame-of-reference » → **CONFIRMÉ**

| Fait | Emplacement |
|---|---|
| Un varint LEB128 par delta de `doc_id`, un varint par fréquence, **aucun en-tête de bloc** | `crates/surch-codec/src/postings_block.rs:285-311` (`encode_postings_blocked`) |
| Le premier `doc_id` de chaque bloc est écrit en **absolu** (delta depuis 0) — c'est ce qui rend un bloc décodable seul | `postings_block.rs:300-305` |
| Décodage symétrique | `postings_block.rs:318-342` (`decode_block_at`) |
| Écriture du varint | `postings_block.rs:135-141` (`write_varint_u32`) |
| Le nom `FOR_BLOCK_SIZE` est trompeur : c'est une taille de bloc, pas un codec FoR | `postings_block.rs:23` |

**Aucun morceau n'était déjà bit-packé.** Le seul « packing » du dépôt est le slot `_source`
de D1, qui ne concerne pas les postings.

### 1.2 Où les postings sont ÉCRITS

| Site | Emplacement |
|---|---|
| Construction, mode disque | `crates/surch-index/src/postings.rs:1219` |
| Construction, mode RAM (segment SHADOW) | `postings.rs:1337` |
| Merge de segments adjacents | `postings.rs:2022` (`FieldMergeAccumulator::encode_and_store`) |
| Un seul `append()` par CHAMP, jamais par terme | `postings.rs:1385` et `postings.rs:2057` |

Les `doc_id` écrits sont **GLOBAUX** (`document_index.rs:2859` passe `doc_id`, pas
`local_doc_id`), donc un segment de 2,4 M documents en couvre la plage
`[doc_base, doc_base + doc_count)`.

### 1.3 Métadonnées de bloc

| Fait | Emplacement |
|---|---|
| `BlockDirEntry { byte_offset_in_term_payload: u32, count: u16, max_doc_id: u32 }` | `postings_block.rs:400-411` |
| Sérialisé sur **10 octets** exactement, sans bourrage | `postings.rs:2603-2616` (`encode_block_dir_entry`) |
| `TermEntry` : `{postings_offset u64, postings_len u32, postings_count u32, block_dir_offset u64, block_dir_count u32}`, **28 octets** | `postings.rs:2530-2601` |
| Le répertoire était recalculé en **RE-DÉCODANT tout le payload** juste après l'avoir encodé | `postings.rs:1245-1249` puis `postings_block.rs:426-448` (`block_directory`) |

### 1.4 Chemins de lecture et contraintes qu'ils imposent au format

Ce sont ces quatre contraintes qui ont dicté le format retenu — elles ne sont pas négociables :

1. **Le payload doit être auto-descriptif en parcours séquentiel**, `block_directory`
   (`postings_block.rs:426`) et `decode_postings_blocked` (`postings_block.rs:369`) le
   balaient bloc par bloc en ne connaissant que `total_count`. ⇒ un en-tête par bloc est
   OBLIGATOIRE ; on ne peut pas loger la largeur de bit-packing uniquement dans le répertoire.
2. **L'encodage doit être CANONIQUE.** `decode_postings_payload_checked`
   (`postings.rs:2643-2655`) décode puis **ré-encode et compare octet pour octet**. ⇒ le choix
   du mode de chaque bloc doit être une fonction déterministe et totale des valeurs.
3. **Un bloc doit rester décodable SEUL.** `DiskPostingsCursor::advance_to_with_status`
   (`postings.rs:4213-4256`) `pread` la seule plage d'octets du bloc visé et appelle
   `decode_block(bytes, count)`. ⇒ le premier `doc_id` d'un bloc doit rester ABSOLU. J'ai
   mesuré la variante « delta depuis le max du bloc précédent » : elle ne rapporte que
   **−0,8 %** du canal doc_id et casserait cette propriété. **Écartée.**
4. **La forme du répertoire est attestée.** `p2_directory_has_expected_shape`
   (`postings.rs:3713-3745`) exige `count == remaining.min(128)`, des offsets strictement
   croissants et des `max_doc_id` strictement croissants. ⇒ `BlockDirEntry` est inchangé,
   octet pour octet. L'attestation BLAKE3 P2 porte sur la table `TermEntry` et sur le
   répertoire, **jamais sur le payload** (`postings.rs:1666-1706`) : changer le codec ne
   touche à aucun digest.

---

## 2. Ventilation réelle, MESURÉE sur le corpus de production

### 2.1 Méthode

Le corpus réel est disponible localement (`~/surch-bench-data/deces-28M.ndjson`,
14,6 Go, 28 917 511 documents). J'ai **simulé l'encodeur existant**, octet pour octet, sur
**un segment complet** — les 2 409 792 premiers documents, soit exactement 1/12 du corpus,
la granularité de segment observée en production — avec un `doc_base` de segment médian
(14 458 752) pour que le coût du premier `doc_id` absolu soit représentatif.

La simulation reproduit la chaîne d'analyse réelle : champs indexés issus de
`indexed_fields_for_document` (`state.rs:4362`), analyseurs résolus comme
`analyze_document` (`document_index.rs:3442-3520`) — `Norm`/`Standard` pour les champs
`text`, `Simple` pour un `text` sans analyseur déclaré (qui ne produit **aucun** token sur
`DATE_NAISSANCE`, ses tokens étant purement numériques), `Keyword` pour les autres types —
et le fan-out des sous-champs `.raw`, qui produisent bien des postings dans le même segment
(`document_index.rs:3489-3500`).

Artefacts versionnés et ré-exécutables : `.remote/d2-ventilation.awk` (aucun Python, invocation
exacte en tête de fichier) et sa sortie brute `.remote/d2-ventilation-seg0.txt`, dont tous les
chiffres de ce rapport sont tirés.

**Validation du modèle** : le total simulé × 12 donne **4 461 972 228 octets** contre la jauge
réellement mesurée `disk_postings_bytes = 4 598 009 701`
(`.remote/p2-memoire-reduction.md:47-49`) — **97,04 %**. Le modèle est donc fidèle à 3 % près ;
les chiffres ci-dessous sont mis à l'échelle de la jauge.

### 2.2 La ventilation

| Poste | octets (1 segment) | × 12 | part | nature |
|---|---:|---:|---:|---|
| payload — canal `doc_id` | 118 914 823 | **1 427 Mo** | 32,0 % | deltas varint + premier id absolu |
| payload — canal `freq` | 67 834 558 | **814 Mo** | 18,2 % | 1 varint par posting |
| table `TermEntry` (28 o/terme) | 133 668 668 | **1 604 Mo** | 35,9 % | métadonnée par TERME |
| répertoire de blocs (10 o/bloc) | 51 412 970 | **617 Mo** | 13,8 % | métadonnée par BLOC |
| **total modélisé** | **371 831 019** | **4 462 Mo** | 100 % | (jauge réelle : 4 598 Mo) |

**C'est le résultat le plus important de ce chantier, et il contredit l'analyse R&D.**
Le §C5 du programme chiffre « 4 598 Mio → ~2 500 Mio, soit −45 % » en supposant que ces
octets sont du payload de postings. **La moitié n'en est pas.** Un chantier de codec, quelle
que soit sa qualité, ne peut agir que sur les 2 241 Mo de payload.

Structure sous-jacente : 4 773 881 termes et 67 834 558 postings par segment, soit **28,15
postings par document** et un **`df` moyen de 14,2**. La cardinalité est écrasée par les
champs `keyword` à haute cardinalité : `UID` fait 1 terme par document (2 409 792 termes,
tous à `df = 1`), `SOURCE_LINE` 408 883, `PRENOMS.raw` 612 540.

### 2.3 Les fréquences

| Mesure | Valeur |
|---|---|
| Postings à `freq == 1` | **99,97 %** (100,00 % sur tous les champs `keyword`) |
| Blocs à fréquence constante égale à 1 | 5 138 710 / 5 141 297 = **99,950 %** |
| Blocs à fréquence constante ≠ 1 | 11 |
| Blocs à fréquence variable | 2 576 |

**L'hypothèse du programme est vérifiée sur les données**, et même sous-estimée : elle vaut
non seulement pour `NOM` mais pour la quasi-totalité du corpus. Le canal `freq` passe de
67 834 558 à **79 883 octets** par segment, soit **−99,88 %**.

### 2.4 Le canal `doc_id` : le FoR naïf est PLUS GROS, mesuré

Distribution des blocs par largeur nécessaire au frame-of-reference (`bits(max delta)`) :

```
bits=0  : 3 401 310 blocs (66,2 %)  — blocs d'UN seul posting, aucun delta
bits=19 :   748 141        bits=20 :   321 946        bits=21 :   177 174
bits=18 :    65 759        bits=11 :    49 315        bits=16 :    42 862
bits=17 :    38 074        bits=12 :    35 736        bits=10 :    28 416
… (reste étalé de 1 à 22 bits)
```

Un FoR pur, avec 1 octet d'en-tête par bloc, donne **119 746 965** octets contre
**118 914 823** au varint : **+0,7 %**. Deux causes, toutes deux mesurées :

1. **Les blocs sont minuscules** : 5 141 297 blocs pour 67 834 558 postings, soit 13,2
   postings par bloc, et **seulement 368 044 blocs sur 5,1 M sont pleins (7,2 %)**. Le coût
   fixe par bloc (en-tête + premier identifiant absolu de 4 octets) écrase le gain de packing.
2. **Les listes sont GROUPÉES.** Le corpus arrive trié par année puis par commune de décès :
   sur `CODE_INSEE_*`, les postings d'un même code forment des runs denses (delta 1) séparés
   par un saut de l'ordre de plusieurs centaines de milliers de documents (le passage à
   l'année suivante). Un FoR prend la largeur du SAUT pour les 127 valeurs du bloc. Sur
   `CODE_INSEE_NAISSANCE`, le FoR pur fait **+63 %** (2 946 247 → 4 798 044 octets).

C'est exactement le cas d'usage du **PFor** (frame of reference *patché*), et c'est pourquoi
le format retenu le comporte.

---

## 3. Format retenu

### 3.1 Disposition

Taille de bloc **inchangée à 128** (`FOR_BLOCK_SIZE`). La faire varier ne sert à rien ici :
99,0 % des termes tiennent déjà dans UN bloc, donc le nombre de blocs est gouverné par la
cardinalité des termes, pas par la taille de bloc.

```text
pour chaque bloc de n <= 128 postings :
    1 octet  en-tête : [doc_mode:2][freq_mode:2][réservé:4 = 0]
    varint   doc_ids[0] ABSOLU                       (inchangé)
    canal doc_id, selon doc_mode :
      0 varint  : n-1 varints de delta                (le format antérieur)
      1 packed  : 1 octet de largeur w, puis n-1 deltas sur w bits
      2 patched : 1 octet w, 1 octet e, n-1 deltas sur w bits (les exceptions
                  y valent 0), puis e couples (1 octet de position,
                  varint de la valeur complète)
    canal freq, selon freq_mode :
      0 all-ones : ABSENT — aucun octet, toutes les fréquences valent 1
      1 constant : varint de la constante (!= 1)
      2 packed   : 1 octet de largeur, puis n fréquences bit-packées
      3 varint   : n varints (repli, uniquement s'il est plus court)
```

Bit-packing poids faibles d'abord, bits de bourrage du dernier octet à zéro.
`crates/surch-codec/src/postings_block.rs:320-378` (`pack_bits` / `unpack_bits`).

### 3.2 Choix du mode : par coût EXACT, jamais par heuristique

`plan_doc_channel` (`postings_block.rs:436-486`) construit l'histogramme des largeurs des
deltas du bloc (33 seaux, O(n)), puis évalue en O(33) le coût EXACT de chaque candidat :

- varint : Σ des longueurs de varint ;
- packed à la largeur maximale : `1 + ceil((n-1)·w/8)` ;
- patched à chaque largeur `w < w_max` : `2 + ceil((n-1)·w/8) + Σ_exceptions (1 + varint)`,
  les exceptions étant **toutes** les valeurs `>= 2^w` (définition totale, donc pas
  d'ambiguïté sur les ex æquo).

On parcourt les largeurs par ordre croissant et **on ne retient qu'une amélioration STRICTE**,
en partant du varint. Deux conséquences :

1. **le codage est canonique** — indispensable au contrôle de ré-encodage (§1.4, point 2) ;
2. **le codage n'est jamais plus gros que le varint historique**, à l'octet d'en-tête près.
   Le même raisonnement s'applique au canal `freq`, où le mode 3 (varint) existe uniquement
   pour fermer le dernier cas où le bit-packing aurait pu coûter plus cher.

Modes retenus sur le segment mesuré : varint **4 005 077** (77,9 %), packed **973 889**
(18,9 %), patché à 1 exception **101 882**, patché à 2 exceptions **60 449** (3,2 % au total).

**Nuance qui rend le chiffrage conservateur** : la simulation n'évalue le mode patché qu'avec
**0, 1 ou 2** exceptions (elle ne suit que les trois plus grands deltas d'un bloc), là où
l'encodeur livré explore **toutes** les largeurs et donc tous les décomptes d'exceptions. À
corpus égal, l'implémentation ne peut donc être que **meilleure ou égale** à ce que la
simulation annonce. Le gain du §5 est un **plancher**, pas un optimum.

### 3.3 Détection des fréquences constantes

Trois tests successifs, dans cet ordre (`postings_block.rs:514-534`) :
`toutes égales à 1` → mode 0, **zéro octet** ; sinon `toutes égales` → mode 1, un varint pour
tout le bloc ; sinon le moins cher de packed / varint. La détection porte sur **le bloc**, pas
sur le terme : un terme dont un seul bloc a des fréquences variables ne perd pas l'omission
sur ses autres blocs (testé par `d2_freqs_mixtes_par_bloc`).

### 3.4 Valeurs aberrantes

Le mode patché borne le nombre d'exceptions par le coût (une exception coûte 1 octet de
position + le varint de la valeur, donc au-delà de quelques-unes le varint pur redevient
gagnant). Position sur `u8` : légal parce qu'un bloc porte au plus 127 deltas.
Le décodeur **refuse** une exception qui ne dépasserait pas sa largeur, une position hors
bloc, une position non strictement croissante, ou un emplacement packé non nul
(`postings_block.rs:747-793`) — quatre invariants qu'un encodeur canonique respecte toujours.

### 3.5 Bonus non demandé : le répertoire de blocs devient gratuit

`encode_postings_blocked_with_directory` (`postings_block.rs:661-711`) rend le payload, le
répertoire ET les compteurs en **une seule passe**. Le chemin d'indexation appelait jusqu'ici
`encode_postings_blocked` puis `block_directory`, qui **re-décodait intégralement le payload
qui venait d'être écrit**, pour chaque terme. Ce second passage disparaît
(`postings.rs:1219-1258`, `postings.rs:2021-2039`). C'est ce qui finance le surcoût CPU du
nouvel encodeur (§6.3).

---

## 4. Compatibilité : **AUCUNE RÉINDEXATION, et pour une raison PLUS FORTE que pour `_source`**

**Décision : le format bascule d'un bloc, sans drapeau, sans lecteur de l'ancien format.**

La consigne demandait de ne pas recopier le raisonnement de D1. Je l'ai donc vérifié
indépendamment pour les postings, et la conclusion est **différente de celle de D1 sur un
point décisif** :

1. **Le segment de postings est process-local et détruit au `Drop`** —
   `PostingsSegment::try_new()` crée un fichier temporaire (`postings.rs:556-575`,
   `next_temp_path()`) et `impl Drop` fait `remove_file` (`postings.rs:699-703`). Aucun
   manifeste, aucun chemin de réouverture d'un segment existant : `grep` ne trouve **aucun**
   constructeur qui ouvrirait un fichier de postings préexistant. Il n'y a donc **aucun index
   à réindexer**, exactement comme pour `_source`.

2. **Mais, contrairement à `_source`, il n'existe PAS non plus de compatibilité INTRA-process
   à préserver.** C'est là que le raisonnement de D1 ne se transpose pas. Le format `_source`
   est gouverné par des variables d'environnement (`SURCH_SOURCE_COMPRESS`,
   `SURCH_SOURCE_COMPRESS_MODE`) et un même `SourceStore` peut légitimement contenir des
   blobs de trois codecs — d'où le tag par slot. Le format des postings, lui, **n'est
   gouverné par aucun drapeau** : `encode_postings_blocked` est appelée inconditionnellement
   par les trois seuls producteurs (`postings.rs:1219`, `:1337`, `:2022`), et le merge de
   segments **décode puis ré-encode** (`push_term` → `encode_and_store`), donc un segment
   fusionné est toujours au format du binaire courant. Un même processus ne peut pas produire
   deux formats.
   `SURCH_POSTINGS_DISK` (`postings.rs:1041`) ne choisit pas un CODEC, il choisit où vivent
   les tableaux de service ; les deux modes écrivent le même payload.

3. **Conséquence : ajouter un tag de codec ou un lecteur de l'ancien format serait du code
   mort par construction**, et coûterait de la surface d'attaque sur un chemin fail-closed.
   Je ne l'ai pas fait. C'est une décision assumée, pas un oubli.

**Ce que la décision ne couvre pas.** Le jour où les segments deviendront persistants (P2,
manifeste atomique), il faudra un numéro de version de format. Le format D2 s'y prête : l'octet
d'en-tête réserve **4 bits à zéro**, et le décodeur **refuse** tout octet dont un bit réservé
est posé (`postings_block.rs:728-730`). Une future révision peut donc se signaler sans
ambiguïté, et un binaire D2 rejettera proprement un segment plus récent au lieu de le
mal-décoder. C'est le point de version que l'ancien format n'avait pas.

---

## 5. Gain disque attendu — **c'est un CALCUL, pas une mesure de surch**

| Poste | avant (× 12) | après (× 12) | delta |
|---|---:|---:|---:|
| canal `freq` | 814 Mo | 1 Mo | **−813 Mo** |
| canal `doc_id`, codage | 1 427 Mo | 1 225 Mo | −202 Mo |
| canal `doc_id`, en-tête de bloc (1 o × 5 141 297 blocs) | 0 | 62 Mo | **+62 Mo** |
| table `TermEntry` | 1 604 Mo | 1 604 Mo | 0 |
| répertoire de blocs | 617 Mo | 617 Mo | 0 |
| **total modélisé** | 4 462 Mo | 3 509 Mo | −953 Mo |
| **mis à l'échelle de la jauge** | **4 598 Mo** | **3 616 Mo** | **−982 Mo** |

**Gain annoncé : −982 Mo, soit −937 Mio, soit −21,4 % du segment de postings.**
Le facteur exact appliqué à la jauge est le ratio mesuré `292 391 640 / 371 831 019 = 0,7864`.
C'est un **plancher** : la simulation borne le mode patché à 2 exceptions, l'encodeur livré ne
l'est pas (§3.2).

**À comparer honnêtement au −2 100 Mio du programme R&D : je livre 45 % de l'objectif**, et la
mesure explique pourquoi le reste n'était pas atteignable par un codec (§2.2).

**Réserve d'unités, à ne pas laisser passer.** La jauge vaut 4 598 009 701 **octets**, soit
4 598 Mo (10⁶) mais **4 385 Mio** (2²⁰). Le tableau du programme R&D
(`rd-latence-programme.md:476`) porte « 4 598 » dans une colonne libellée « Mio ». J'ai tout
exprimé en Mo décimaux, comme la jauge, et je donne la conversion à chaque fois. La ventilation
disque du verdict 28M ne boucle d'ailleurs pas non plus avec cette ligne
(13 824 + 4 598 + 1 047 = 19 469 pour un total annoncé de 18 568) — c'est un point pour D3.

### Ce qui reste sur la table, et qui est maintenant CHIFFRÉ

Le prochain gain disque des postings n'est pas dans le codec, il est dans les métadonnées :

1. **Répertoire de blocs des termes mono-bloc : 567 Mo strictement redondants.**
   **4 727 035 termes sur 4 773 881 (99,02 %) tiennent dans UN seul bloc.** Pour eux,
   `BlockDirEntry` porte `byte_offset = 0` (constant), `count = postings_count` (déjà dans
   `TermEntry`) et `max_doc_id` (utile seulement pour sauter un bloc… qu'on charge de toute
   façon). Supprimer ces entrées ramènerait le répertoire de 617 à 50 Mo.
   **Je ne l'ai pas fait** : cela change la sémantique de `block_dir_count == 0` (aujourd'hui
   « pas de répertoire persisté, recalcule »), la garde `p2_directory_has_expected_shape`, et
   le contrat de `decoded_block_matches_directory` — trois points du chemin fail-closed, pour
   un chantier annoncé « codec ». C'est le meilleur candidat suivant.
2. **Table `TermEntry` : 1 604 Mo, soit 36 %.** `block_dir_offset` est un `u64` absolu et
   `postings_offset` aussi, alors qu'un segment fait ~370 Mo. Un enregistrement compacté à 16
   octets rendrait ~687 Mo. Périmètre S5b, pas D2.

Cumulés à D2, ces deux leviers ramèneraient les postings sous 2 400 Mo.

---

## 6. Effets sur les trois autres axes

### 6.1 Latence — favorable, mais moins que le programme ne l'annonce

Ce qui est **certain** : le volume d'octets à `pread` par bloc baisse d'environ 43 %, et le
canal `freq` — qui était décodé varint par varint à chaque bloc chargé, y compris par le
chemin C2 qui lit `cursor.freq()` à chaque posting — devient un `vec![1; n]`, donc **zéro
décodage**. Sur un bloc plein de 128 postings à `tf = 1`, cela retire 128 lectures de varint
branchantes et les remplace par un remplissage mémoire.

Ce qui est **incertain, et je le dis** : le §C5 attend « `codec_decode_us` en baisse d'au moins
40 % à `df >= 100k` » et une implémentation « vectorisée ». **La mienne ne l'est pas** :
`unpack_bits` est un déballage scalaire bit à bit. Elle est probablement plus rapide que le
varint (pas de dépendance de donnée entre valeurs, pas de branchement par octet), mais je n'ai
mesuré aucun temps. Par ailleurs 78 % des blocs restent en mode varint — le mode le moins
cher en octets sur les blocs minuscules —, donc le gain de décodage porte surtout sur les
blocs pleins, c'est-à-dire sur les termes à fort `df`, c'est-à-dire précisément la queue de
latence. C'est le bon endroit, mais l'amplitude n'est pas connue.

Aucun invariant de C1 ni de C2 n'est touché : `scored_pair_ordering` n'existe qu'en un seul
exemplaire (`search.rs`), le chemin `single_term_match_topk_streamed` lit ses postings par le
même curseur, et l'ordre de parcours (`doc_id` croissant, par segment de `doc_base` croissant)
est inchangé. **Aucune ligne de `surch-search` ni du chemin de scoring n'a été modifiée.**

### 6.2 Mémoire — inchangée, aucun cache ajouté

**Aucun cache de blocs décodés n'a été ajouté**, donc rien à chiffrer de ce côté. Ce qui
change, à la marge et dans le bon sens :

| Poste | Effet |
|---|---|
| `Vec<BlockDirEntry>` résident / spillé | **inchangé** (même struct, même 10 o sur disque) |
| Tampons transitoires de l'encodeur | `scratch: Vec<u32>` (≤ 512 o) et `exceptions_scratch` (≤ 640 o), **alloués une fois par TERME**, réutilisés d'un bloc à l'autre |
| Bloc décodé par le curseur | inchangé (≤ 128 postings), mais le `Vec` de fréquences est maintenant rempli sans décodage |
| Compteurs de codec | 4 × `u64` par `TermDictionary` = 32 octets par segment, soit **384 octets** pour les 12 segments |

Le tampon `field_payload` conserve sa réservation historique de 3 octets par posting
(`postings.rs:1161`) ; comme le format écrit désormais ~1,6 o/posting, la réservation est plus
large que nécessaire mais **strictement identique à avant** — je n'ai pas voulu changer le
comportement d'allocation de l'indexation dans le même lot.

### 6.3 Indexation — le piège du dépôt est traité de front

Le piège rappelé par la consigne (un `pwrite` par token = −40 %) ne se rejoue pas : **le
nombre et la taille des écritures ne changent pas** — toujours un `append()` par champ
(`postings.rs:1385`), désormais avec ~43 % d'octets en moins.

Ce que l'encodeur **ajoute** par bloc : une passe O(n) pour construire l'histogramme des
largeurs (33 seaux sur la pile) et une boucle O(33) pour choisir le mode. Ce qu'il
**supprime** : **le décodage intégral du payload de chaque terme**, que `block_directory`
imposait juste après l'encodage (`postings.rs:1245-1249` avant ce commit). Ce décodage lisait
`2 × df` varints et allouait deux `Vec<u32>` par terme.

Le solde est donc structurellement **favorable** : on remplace un décodage complet par un
comptage. Mais **je ne l'ai pas mesuré**, et c'est le risque résiduel principal du chantier
sur l'axe indexation.

---

## 7. Tests écrits

**Aucun n'est vert : rien n'a été compilé ni exécuté** (§9). Ils sont ÉCRITS et versionnés.

### 7.1 Codec — `crates/surch-codec/src/postings_block.rs`, module `tests`

Un utilitaire central, `assert_strict_round_trip`, impose sur chaque corpus **quatre**
propriétés à la fois : identité stricte des `doc_id` ET des fréquences par le décodeur
complet ; **canonicité** (ré-encoder le décodé rend les mêmes octets) ; égalité du répertoire
rendu par l'encodeur avec celui que `block_directory` recalcule ; et décodage **isolé** de
chaque bloc depuis son offset.

| Test | Cas verrouillé |
|---|---|
| `d2_freqs_toutes_a_un_disparaissent_du_payload` | le cas NOMINAL : `freq_bytes == 0`, tout le payload est du canal doc_id |
| `d2_freq_constante_non_unitaire_tient_en_un_varint_par_bloc` | 3 blocs ⇒ 3 octets de fréquences au total |
| `d2_freqs_variables_restent_exactes` | canal freq réellement écrit et exact |
| `d2_freqs_mixtes_par_bloc` | les trois modes de fréquence **dans le même terme** |
| `d2_freqs_tres_dispersees_retombent_sur_le_varint` | le repli qui garantit « jamais plus gros » |
| `d2_df_de_un_et_df_de_deux` | `df = 1`, `doc_id = 0`, `doc_id = u32::MAX`, `tf != 1` sur un posting unique |
| `d2_bloc_partiel_en_fin_de_terme` | 3 blocs pleins + un bloc partiel de 17 |
| `d2_df_tres_grand_et_dense` | 100 000 postings ; exige un payload > 4× plus petit qu'avant |
| `d2_valeurs_aberrantes_ne_font_pas_exploser_le_bloc` | **le motif réel du corpus** : 8 runs denses séparés par des sauts de 900 000 ; exige de rester sous le varint |
| `d2_jamais_significativement_plus_gros_que_le_varint` | **propriété structurante** : sur 4 dispersions (10 → 10⁷), payload ≤ payload antérieur + 1 octet/bloc |
| `d2_corpus_seede_multi_segments` | 3 bases de `doc_id` disjointes et croissantes |
| `d2_bit_packing_round_trip_sur_toutes_les_largeurs` | `pack_bits`/`unpack_bits` pour **les 32 largeurs**, 133 valeurs (bourrage non aligné) |
| `d2_bit_packing_refuse_une_entree_tronquee` | `UnexpectedEof`, pas de complétion par des zéros |
| `d2_en_tete_a_bit_reserve_est_une_corruption` | fail-closed, et point d'accroche d'un futur numéro de version |
| `d2_mode_de_canal_inconnu_est_une_corruption` | mode doc 3 réservé |
| `d2_largeur_de_bit_packing_invalide_est_une_corruption` | largeurs 0, 33, 255 |
| `d2_freq_constante_egale_a_un_est_une_corruption` | canonicité imposée AU DÉCODAGE |
| `d2_payload_tronque_est_un_eof` | 4 points de coupe |
| `d2_repertoire_de_blocs_est_gratuit_et_exact` | le répertoire « gratuit » est identique au recalculé |
| `d2_compteurs_se_cumulent_sans_deborder` | `merge` saturant |

Les tests préexistants du module (`blocked_roundtrip`, `blocked_roundtrip_empty`,
`blocked_roundtrip_exact_block_boundary`, `block_directory_matches_chunk_boundaries_and_max_doc_id`,
`block_directory_rejects_truncated_payload`, `blocked_rejects_*`) sont **conservés inchangés**
et restent la référence d'API.

### 7.2 Index — `crates/surch-index/tests/postings.rs`

`d2_postings_bit_packes_relus_a_l_identique` passe par le **vrai** `PostingsBuilder`, en mode
RAM **et** en mode disque, et vérifie sur six formes de terme (tf=1 sur 4 blocs, tf variable,
`df = 1`, liste groupée à sauts, trois bases de `doc_id` disjointes) que **les deux chemins de
lecture** rendent la suite identique : le décodage intégral (`decode_from_segment`) et le
**curseur à sauts** (`disk_cursor` → `advance_to` → `freq()`), c'est-à-dire celui qui `pread`
bloc par bloc. Il vérifie aussi que le nombre de blocs annoncé par les compteurs est exact,
qu'un terme absent reste absent, et que `postings_segment_skipped_terms() == 0`.

### 7.3 Régression ajustée

`curseur_disque_refuse_la_borne_inferieure_corrompue_entre_blocs` (`postings.rs:5062`)
concaténait deux blocs encodés séparément et supposait que le résultat tenait dans la longueur
canonique. Ce n'est plus vrai : le codec D2 choisit le mode de chaque bloc par coût, donc un
bloc corrompu n'a aucune raison de coûter exactement autant que le bloc licite qu'il remplace.
Le test vérifie désormais que le PREMIER bloc est byte-identique au canonique (ce qui valide
l'offset lu dans le répertoire) et ouvre le curseur sur la longueur réelle du payload
corrompu. **L'assertion de fond est inchangée** : la borne inférieure incohérente doit rendre
`DiskPostingsAdvance::Error`, jamais une omission silencieuse.

### 7.4 Contrat de scrape

`crates/surch-api/tests/stats.rs` exige désormais la présence des quatre nouvelles jauges dans
le corps `/_prometheus_metrics`.

---

## 8. Compteurs ajoutés — **noms exacts à scraper**

```
surch_index_postings_codec_blocks               {index="…"}
surch_index_postings_codec_doc_id_bytes         {index="…"}
surch_index_postings_codec_freq_bytes           {index="…"}
surch_index_postings_codec_freq_omitted_blocks  {index="…"}
```

Publiés dans `crates/surch-api/src/stats.rs:468-484`, alimentés par
`TermDictionary::postings_codec_stats()` → `DocumentIndex::postings_codec_stats()` →
`AppState::index_postings_codec_stats()`. Ce sont des **jauges** (photo cumulée des segments
scellés), pas des compteurs de requêtes, et elles sont labellisées par index comme toutes les
jauges `surch_index_*`.

**Comment les lire, et le piège à éviter.** Le gain D2 est **réalisé** si et seulement si :

- `surch_index_postings_codec_freq_omitted_blocks / surch_index_postings_codec_blocks` ≈
  **0,999** (attendu 0,9995 sur deces) ;
- `surch_index_postings_codec_freq_bytes` est **négligeable** devant `..._doc_id_bytes`
  (attendu ~0,07 % sur deces).

Si le ratio d'omission s'effondre, le gain de 813 Mo n'existe pas, **et rien d'autre ne le
signalera** : `disk_postings_bytes` baisserait quand même un peu grâce au canal doc_id, ce qui
donnerait l'illusion d'un succès partiel. C'est exactement le scénario « gain non réalisé passé
inaperçu » déjà vécu sur ce dépôt.

`..._doc_id_bytes` inclut l'en-tête de bloc et le premier identifiant absolu : il ne doit
baisser que **modestement** (−14 % attendu). Une baisse spectaculaire y serait suspecte.

---

## 9. Ce que je ne peux PAS garantir

À lire comme la partie la plus importante du rapport.

1. **RIEN N'EST COMPILÉ NI EXÉCUTÉ.** L'interdiction de `cargo build/check/test/clippy` en
   local est absolue ; seul **`cargo fmt --check` est passé, et il est VERT**. Le codec et les
   21 tests sont **ÉCRITS, PAS VERTS**. La CI est le seul juge
   (`cargo clippy --workspace --all-targets --locked -- -D warnings`, `.github/workflows/ci.yml:51`) :
   un lint Clippy suffit à tout casser.
   **Aucune relecture indépendante n'a abouti.** Une avait été lancée en sous-agent ; elle
   n'a jamais rendu de conclusion, et j'ai terminé la vérification moi-même. Les cinq
   derniers ajustements du commit (doc de `finish` remise à jour, doc de
   `postings_codec_stats` corrigée sur le périmètre des segments,
   `const _: () = assert!(FOR_BLOCK_SIZE <= 256)` pour figer l'invariant des positions
   d'exception sur un octet, deux `clone()` inutiles retirés) sont **les miens**, pas ceux
   d'un relecteur. Aucun second regard n'atteste donc ce code : la relecture reste à faire,
   et la CI reste le seul juge.
2. **Aucun chiffre disque de ce rapport ne vient de surch qui tourne.** La ventilation du §2
   est produite par une **simulation en awk** de l'encodeur, sur le corpus réel mais **hors
   du moteur**. Elle est validée à 97,04 % contre la seule mesure moteur disponible
   (`disk_postings_bytes`), ce qui est une corroboration forte, **pas une mesure**. Le
   −982 Mo est un **CALCUL**.
3. **La simulation reproduit ma lecture de la chaîne d'analyse, pas la chaîne elle-même.**
   Si `analyze_document` produit des tokens que je n'ai pas reproduits (un analyseur mal
   attribué, un champ que j'ai cru non indexé), la ventilation se décale. Les 3 % d'écart avec
   la jauge sont peut-être exactement cela, plutôt que l'inégalité des segments.
4. **Aucune mesure de latence, ni de débit d'indexation.** Les §6.1 et §6.3 sont des
   raisonnements. En particulier, le §C5 exigeait `codec_decode_us` en baisse d'au moins 40 % :
   **je n'ai pas ce compteur et je ne l'ai pas mesuré**, et mon implémentation n'est pas
   vectorisée — sur ce critère précis, C5 n'est pas satisfait.
5. **Le gain d'indexation n'est pas mesuré.** L'argument (suppression d'un décodage complet
   par terme) est structurel, mais le corpus 28,9 M n'a pas été rejoué.
6. **Je n'ai pas testé l'injection d'une corruption sur le PAYLOAD à travers le curseur de
   production.** Les tests de corruption du codec passent par `decode_postings_blocked`
   directement. Le comportement de `DiskPostingsCursor::advance_to_with_status` face à un
   payload D2 corrompu est **raisonné** (toute erreur de `decode_block` rend
   `DiskPostingsAdvance::Error`, et `decoded_block_matches_directory` reste en aval), **pas
   testé de bout en bout**. C'est la même réserve que celle de C2 (§7.3 de son rapport).
7. **Le harnais de bench n'a pas été touché** (interdit par la consigne). Pour observer le
   gain, son propriétaire doit scraper les quatre jauges du §8 en plus de
   `surch_index_disk_postings_bytes`. **Sans cela, le §8 ne sert à rien.**
8. **Le format n'est pas versionné explicitement.** Les 4 bits réservés le permettront
   (§4), mais aujourd'hui aucun numéro de version n'est écrit. Tant que les segments sont
   process-local, c'est sans conséquence ; le jour où P2 les persistera, ce sera un prérequis.
9. **Je n'ai pas vérifié l'effet sur BEIR.** Le choix de mode est fait par coût exact, donc il
   ne peut pas dégrader la taille sur un autre corpus ; mais la répartition des modes (et donc
   le gain de décodage) y sera différente, et le gain de fréquences y sera **beaucoup plus
   faible** puisque `tf > 1` y est courant. **Le −21,4 % est un chiffre deces, pas un chiffre
   universel.**

---

D2_DONE
