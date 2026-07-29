# Mesure 4 axes — surch @713e75b vs Elasticsearch 8.6.1 — VERDICT

**Statut global : campagne 28 M NON EXÉCUTÉE. Arrêt au smoke, sur instruction.**
Le smoke 1,36 M a échoué (`rc=1`) pour une cause entièrement diagnostiquée et
étrangère aux quatre axes. Le protocole préparé n'est pas satisfaisable sur cette
machine. Ce document rapporte ce que le smoke a effectivement mesuré, ce qu'il
prouve, et surtout ce qu'il ne prouve pas.

- Date : 2026-07-29, 18h21 → 18h50 UTC
- VM Scaleway PRO2-M `8e7c72d6-204d-4c5c-8c0c-a9da99347053`, `fr-par-2`, `51.159.170.64`
- SHA mesuré : `713e75ba0abdff4a8f5a709042427f14686eb3f4` (C1 + C2 + D1 ; **sans D2**)
- Image : `ghcr.io/rhanka/surch:sha-713e75ba…` — ES `docker.elastic.co/elasticsearch/elasticsearch:8.6.1`
- Cadre : `CPUSET=0-7`, `MEM_LIMIT=6g`, `POSTINGS_DISK=1`, corpus décès 1 360 000 documents

---

## 0. Pourquoi la campagne 28 M n'a pas eu lieu

### Cause racine n°1 (corrigée) — noyau trop ancien

Le harnais mesure une cinquième série de latence, dite « froide », en forçant
l'éviction du cache de pages via le fichier cgroup v2 `memory.reclaim` avant
chacune des 50 requêtes. La VM tournait sous **Linux 5.15.0-173** ; or
`memory.reclaim` n'existe qu'à partir de **Linux 5.19**. Le fichier était donc
purement absent du cgroup du conteneur — ce n'était pas un défaut de droits.

Artefact : `ls /sys/fs/cgroup/system.slice/docker-<id>.scope/` ne listait aucun
`memory.reclaim`, alors que `memory.current`, `memory.stat`, `memory.max`, etc.
étaient bien présents (contrôleur mémoire actif, feature absente).

Correction appliquée — **elle ne touche pas au protocole de mesure** : installation
du noyau HWE `linux-generic-hwe-22.04` (**6.8.0-136**) et redémarrage. Après reboot :
`memory.reclaim` présent, `sudo -n` écrit dedans (chemin `reclaim_writer=sudo` prévu
par le harnais), montage `/var/lib/docker` (fstab, label `dockervol`) et corpus intacts.

### Cause racine n°2 (bloquante, non corrigée) — reclaim infaisable de façon fiable

Sur le noyau 6.8, le smoke a été rejoué à l'identique. Résultat :

| run | moteur | reclaims réussis | verdict |
|---|---|---|---|
| smoke (block) | ES | **10 / 50** — `memory_reclaim_write_failed_before_request_11` | invalide |
| smoke (block) | surch | 50 / 50 | **valide** |
| smoke-witness (doc) | surch | **27 / 50** — `memory_reclaim_write_failed_before_request_28` | invalide |

Le harnais écrit dans `memory.reclaim` une cible égale à `min(file_before, memory.current)`,
c'est-à-dire **la totalité du cache de pages du cgroup en une seule écriture**. Le noyau
renvoie `EAGAIN` dès qu'il ne peut pas récupérer ce montant intégral. Deux facteurs le
rendent aléatoire ici :

1. **L'hôte est sans swap** (`Swap: 0B`) : tout ce qui est anonyme est irrécupérable.
   Le cgroup ES porte ~3,7 Gio de tas JVM anonyme, donc le noyau ne peut servir la
   cible qu'avec des pages fichier, dont une part est à tout instant sale, en
   écriture différée ou mappée active.
2. La cible demandée est, par construction, **100 % du cache** — la limite exacte de
   ce qui est réalisable. Les audits `*.cold_reclaim.tsv` le montrent : les reclaims
   qui passent laissent 0,1 à 20 % de cache résiduel, donc la marge est nulle.

**Ce n'est donc pas une singularité d'Elasticsearch** : surch a échoué au 28ᵉ essai dans
le run témoin. Le gate est instable pour les deux moteurs sur cet hôte ; le 50/50 de
surch en mode `block` est un coup de chance, pas une propriété.

### Pourquoi cela interdit la campagne 28 M telle que préparée

1. Dans le chemin **non-P2** — celui qu'emprunte `run-bench.sh`, qui ne pose pas
   `P2_MEASURE=1` — la sonde froide est un **gate dur**, exigeant littéralement
   `50` reclaims et `50` lignes d'audit :

   ```
   if [ "$P2_MEASURE" != "1" ] \
      && { [ "$cold_reclaimed_requests" -ne 50 ] || [ "$cold_reclaim_audit_records" -ne 50 ] || [ "$cold_ok" != true ]; }; then
     measurement_valid=false
   ```

   Aucune variable d'environnement n'en sort : `COLD_PROBE=0` met le compteur à 0 et
   échoue le même test ; `COLD_PROBE_REQUESTS` n'est pas relu par le gate, qui code
   `50` en dur.

2. **Conséquence directe sur l'exigence n°1 de la mission** : quand un moteur obtient
   sa série froide et l'autre non, les deux ventilations par forme n'ont plus le même
   nombre d'enregistrements, et le calcul des ratios s'arrête net —
   `by_form_ratios : jq a échoué : effectifs de ventilation par forme differents entre moteurs`,
   `by-form-ratios.jsonl` fait 0 octet. **Le livrable central — les ratios par forme
   contre ES — est précisément ce que ce gate détruit.**

3. Un run 28 M coûte ~50 min par moteur et **le gate tombe à la toute fin**. Avec
   10/50 et 27/50 observés à petite échelle, et une pression d'écriture bien
   supérieure à 28 M, la probabilité que les deux moteurs franchissent 50/50 est
   faible. C'est exactement le scénario « invalidé au bout de 50 minutes » contre
   lequel la mission met en garde.

### Ce que je n'ai pas fait, et pourquoi

Trois issues existent. Aucune ne m'appartient :

- **`COLD_PROBE=0` sur les deux moteurs.** Rend les phases symétriques, les ratios
  calculables, et n'altère rien de la mesure — mais le harnais marquerait quand même
  les deux runs `measurement_valid=false`. C'est **redéfinir ce que le harnais
  certifie**. Les deux replis pré-autorisés par la mission étaient
  `DISK_VENTILATION_TOLERANCE_MIB` et `SURCH_C2_STREAM_EXPECT=off` ; `COLD_PROBE`
  n'en fait pas partie, et cette absence est une information.
- **Ajouter du swap à l'hôte.** Rendrait l'anonyme récupérable et ferait passer le
  reclaim — mais changerait le régime mémoire du banc, c'est-à-dire l'objet même de
  la mesure. Ce serait truquer.
- **Basculer `P2_MEASURE=1`.** C'est le chemin que le harnais lui-même a conçu pour ce
  cas : il y déclare le froid explicitement diagnostique (« cold dépend des droits de
  reclaim et reste diagnostique, sans pouvoir sauver ni invalider les phases
  causales ») et il produit nativement les cinq phases causales `warm_match`,
  `match_control`, `warm_bool`, `bool_size10`, `bool_size0` — soit exactement la
  ventilation par forme demandée. Plusieurs indices suggèrent que la préparation le
  visait : les budgets de `run-bench.sh` sont commentés « produisent les 12 segments
  de référence », or `P2_REQUIRED_SEGMENTS=12` n'est vérifié que sous `P2_MEASURE=1`.
  Mais P2 est une campagne (`p2-campaign.sh`, `p2-gate.sh`, `p2-report.sh`), pas un
  drapeau : il faudrait fixer `P2_VARIANT`, les entrées de sonde, les gates de
  segments. Ce serait refaire la préparation, ce que la mission m'interdit, et
  improviser un protocole est le plus court chemin vers un résultat plausible et faux.

---

## 1. Axe latence — VENTILÉE PAR FORME (1,36 M docs, jamais agrégée)

Ces chiffres sont **valides pour surch** (`measurement_valid=true`) et, pour ES, portent
sur des séries chaudes complètes ; seule la série froide d'ES manque. Ils tranchent la
petite échelle et **rien d'autre** : ni le p95 ni le RSS à 28 M.

Métrique `client` = bout en bout vu de la sonde (surcoût de sonde inclus), en ms,
p50 / p95 / p99. Les ratios sont recalculés depuis `es.by-form.jsonl` et
`surch.by-form.jsonl`, le harnais ayant refusé de les produire (cf. §0).

### Requêtes ALÉATOIRES — les seules à valeur de latence

| forme | n | ES | surch | surch/ES |
|---|---|---|---|---|
| **`match` mono-terme** | 500 | 1,41 / 2,58 / 3,85 | 0,54 / 0,79 / 1,48 | **0,38 / 0,31 / 0,38** |
| **`bool.must`** | 500 | 1,66 / 3,51 / 5,62 | 0,89 / 3,76 / 6,01 | **0,54 / 1,07 / 1,07** |
| **témoin autonome (`_source:false`) — `match`** | 500 | 0,80 / 1,28 / 1,88 | 0,29 / 0,40 / 0,68 | **0,36 / 0,31 / 0,36** |
| **témoin autonome (`_source:false`) — `bool`** | 500 | 1,09 / 1,91 / 2,89 | 0,82 / 3,89 / 6,13 | **0,75 / 2,04 / 2,12** |

### Sonde fixe — best-case cache, à ne PAS citer comme latence

| forme | n | ES | surch | surch/ES |
|---|---|---|---|---|
| `match` mono-terme, sonde fixe | 1000 | 1,98 / 3,47 / 4,72 | 0,60 / 0,79 / 0,95 | 0,30 / 0,23 / 0,20 |

### Lecture

**La ventilation par forme change le signe de la conclusion.** À 1,36 M :

- sur `match` mono-terme, surch est **2,6 à 3,3× plus rapide** qu'ES ;
- sur `bool.must`, surch est à **parité au p50 et 7 % plus lent aux p95/p99** ;
- sur le témoin autonome `bool`, surch est **2,0 à 2,1× PLUS LENT** qu'ES aux queues.

Un mélange 50/50 de ces formes fabriquerait un ratio agrégé situé entre un gain de 3×
et une perte de 2×, qui ne décrirait aucune requête réelle. **C'est la démonstration
directe du chiffre faux cité pendant des semaines sur ce projet.** Aucun agrégat n'est
publié ici.

Côté moteur (`took`, granularité 1 ms d'ES), surch rend 0 ms aux trois quantiles sur
`match` : le temps moteur est sous le millimètre et non résolu par cette métrique. Sur
`bool` au contraire, surch monte à 3 ms au p95 (témoin : 3 ms contre 1 ms pour ES) —
la queue `bool` est bien du temps moteur, pas du bruit de sonde.

---

## 2. Axe RSS conteneur

**RSS conteneur** = empreinte mémoire du cgroup du conteneur moteur relevée par le
harnais en fin de phase chaude, sous une limite de 6 Gio.

| moteur / mode | RSS conteneur | anonyme | fichier |
|---|---|---|---|
| Elasticsearch 8.6.1 | **3,708 GiB** | 3,94 Go | 0,77 Go |
| surch `_source`=block | **185,2 MiB** | 0,17 Go | 0,54 Go |
| surch `_source`=doc | **170,9 MiB** | — | — |

Écart ≈ **20×** en faveur de surch à 1,36 M. À prendre pour ce que c'est : à cette
échelle, l'index entier tient confortablement sous la limite pour les deux moteurs.
C'est à 28 M que cet axe devient discriminant, et **28 M n'a pas été mesuré**.

---

## 3. Axe indexation (doc/s)

| moteur / mode | indexation (doc/s) | documents | erreurs d'items |
|---|---|---|---|
| Elasticsearch 8.6.1 | 12 081 | 1 360 000 / 1 360 000 | 0 |
| surch `_source`=block | **13 677** | 1 360 000 / 1 360 000 | 0 |
| surch `_source`=doc | 13 030 | 1 360 000 / 1 360 000 | 0 |

surch est **+13,2 %** devant ES. Le mode `block` de D1 **n'a pas coûté** d'indexation :
il en gagne 5 % contre le mode `doc` (13 677 vs 13 030), ce qui est cohérent avec un
regroupement des écritures de `_source` par blocs de 16 Kio.

---

## 4. Axe disque — VENTILÉ PAR COMPOSANT, avec et sans témoin

Ventilation certifiée par le harnais (`disk_ventilation_valid=true` dans les **deux**
runs ; la réconciliation a bouclé, le seuil `max(1 %, 16 Mio)` n'a pas été frôlé à
cette échelle — il reste **non confronté à 12 Gio**).

| composant | `doc` (témoin) | `block` (D1) | delta |
|---|---|---|---|
| `_source` | 402 657 280 o (384,00 Mio) | **134 217 728 o (128,00 Mio)** | **−268 435 552 o, −66,67 %** |
| postings | 353 419 264 o (337,04 Mio) | 353 419 264 o (337,04 Mio) | **0 octet** |
| subfields | 65 073 152 o (62,06 Mio) | 65 073 152 o (62,06 Mio) | **0 octet** |
| dictionnaire / fst-merge | 0 | 0 | 0 |
| autres | 0 | 0 | 0 |
| **total** | **821 149 696 o (783,11 Mio)** | **552 710 144 o (527,11 Mio)** | **−32,69 %** |
| fichiers / segments | 9 / 5 | 9 / 5 | identiques |

Pour mémoire, ES : **648 Mio** au total — le harnais ne ventile par composant que surch.

### Lecture

**Le témoin fait son travail et l'effet D1 est isolé sans ambiguïté.** Entre les deux
runs, postings et subfields sont identiques **à l'octet près**, ainsi que le nombre de
fichiers et de segments : la seule variable ayant bougé est le mode d'écriture du
`_source`. La compression par blocs de 16 Kio divise le `_source` par exactement 3
(−66,67 %) et le volume total par 1,49 (−32,7 %). Sans ce témoin, on aurait attribué
à D1 un −32,7 % dont on n'aurait pas su quelle part venait de `_source`.

Réserve d'échelle : à 1,36 M l'index fait 527 Mio, les postings pèsent 64 % du total
et `_source` 24 %. À 28 M, la référence historique donne un rapport inverse
(`_source` 7 248 Mio contre postings 4 598 Mio) : **l'effet relatif de D1 y sera
nettement plus grand, et ce document ne le mesure pas.**

---

## 5. Compteurs d'activation — le nouveau chemin a-t-il servi ?

### C2 — lecture streamée du `match` mono-terme : **PROUVÉ ACTIF**

`SURCH_C2_STREAM_EXPECT=stream` était exigé, donc le harnais aurait invalidé le run si
le chemin n'avait pas servi. Il a servi, et le compteur `surch_dbg_c2_single_term_stream_total`
tombe dans la borne attendue à chaque phase (`c2_stream_checked=true`, `c2_stream_valid=true`) :

| phase | requêtes `match` | borne mono-token | delta observé | attendu | valide |
|---|---|---|---|---|---|
| fixed | 1000 | 1000 | **1000** | `1000` (exact) | oui |
| random | 500 | 464 | **464** | `[464;500]` | oui |
| no_source | 500 | 464 | **464** | `[464;500]` | oui |
| **cumul** | 2000 | — | **1928** | — | — |

Résultat identique au bit près dans le run témoin `doc` : les sondes sont déterministes.
**La mesure porte donc bien sur le nouveau code, pas sur l'ancien.**

### C1 — terminaison anticipée : actif en élagage, **jamais en terminaison anticipée**

Relevés depuis les scrapes Prometheus complets `out/smoke/surch.c2.<phase>.after.prom`.
Attention : deux des quatre noms cités dans la mission n'existent pas sous cette forme
dans le code — les noms réels sont donnés ci-dessous.

| compteur (nom réel) | fin `fixed` | fin `random` | fin `no_source` | fin `cold` |
|---|---|---|---|---|
| `surch_dbg_c1_stream_docs_pruned_total` | 4 811 000 | 4 876 841 | 4 876 841 | 4 881 586 |
| `surch_dbg_c1_stream_docs_scored_total` | 17 000 | 20 746 | 20 746 | 20 926 |
| `surch_dbg_c1_maxscore_blocks_skipped_total` | *(absent)* | 227 | 227 | 331 |
| `surch_dbg_c1_maxscore_early_stop_total` | *(absent)* | *(absent)* | *(absent)* | *(absent)* |

- Correspondance de noms : `surch_dbg_c1_scored_total` → **`surch_dbg_c1_stream_docs_scored_total`** ;
  `surch_dbg_c1_early_stop_total` → **`surch_dbg_c1_maxscore_early_stop_total`**.
- **Élagage massif et réel** : 4 876 841 documents élagués pour 20 746 scorés, soit
  **99,58 % du flux écarté sans scoring**. C1 travaille.
- **La terminaison anticipée n'a jamais été déclenchée.** Le compteur Prometheus n'est
  créé qu'au premier incrément ; son absence de tous les scrapes signifie donc zéro
  déclenchement sur 3 000 requêtes. Le saut de blocs maxscore, lui, s'est déclenché
  331 fois — c'est-à-dire très peu.
- **Anomalie que je signale sans l'expliquer** : entre la fin de `random` et la fin de
  `no_source`, les trois compteurs C1 sont rigoureusement inchangés alors que C2 a
  progressé de 464 et que 500 requêtes `bool` ont été jouées dans l'intervalle. Soit
  la phase `no_source` n'emprunte pas le chemin instrumenté C1, soit l'instrumentation
  ne couvre pas ce cas. Je n'ai pas d'artefact pour trancher, donc je ne tranche pas.

---

## 6. Temps, coût, état de la machine

| poste | valeur |
|---|---|
| VM allumée depuis | ~14h32 UTC (préparation par la session précédente) |
| Ma session de mesure | 18h21 → 18h50 UTC, soit **~29 min** |
| Dont : diagnostic noyau + install HWE + reboot | ~10 min |
| Dont : smoke (5,15, échoué) | ~6 min |
| Dont : smoke (6.8) + smoke-witness | ~13 min |
| Total VM à l'heure de ce rapport | **~4 h 20** |
| Coût | **estimation, pas un artefact** : à ~0,25–0,30 €/h HT pour un PRO2-M, de l'ordre de **1,1 à 1,3 €** pour les 4 h 20, dont ~0,15 € pour ma session. Je n'ai pas accès à la facturation réelle. |

**État final de la machine : ALLUMÉE ET INACTIVE.** Aucun processus `fair-ab`/`run-bench`,
aucun conteneur, aucun volume `fairab-*`. Racine 53 G libres, `/var/lib/docker` 60 G
libres. Noyau désormais **6.8.0-136-generic**. Corpus `deces-28M.ndjson` (14 666 285 980 o,
28 917 511 documents) et `deces-1.36M.ndjson` intacts, images Docker en cache.

> **La VM tourne à vide à tes frais. Elle est prête à reprendre immédiatement si tu
> arbitres l'une des issues du §0 ; sinon, détruis-la.** Le seul actif qu'elle porte et
> qui coûterait cher à reconstituer est le corpus 28 M (14,6 Go transférés à la main).

---

## 7. Ce que ces chiffres permettent — et ne permettent PAS — de conclure

### Ils permettent de conclure

1. **La ventilation par forme est indispensable et le prouve par l'exemple.** Sur le même
   run, `match` mono-terme donne 0,31–0,38× et le témoin `bool` donne 2,04–2,12×. Tout
   ratio agrégé sur ce mélange est un artefact arithmétique. C'est établi, à 1,36 M.
2. **C2 sert effectivement** : 1928 requêtes mono-terme sont passées par la lecture
   streamée, dans les bornes exigées, sur deux runs indépendants. Ce qui est mesuré est
   bien le nouveau code.
3. **C1 élague massivement mais ne termine jamais par anticipation** : 99,58 % des
   documents parcourus sont écartés sans scoring, et 0 terminaison anticipée sur 3 000
   requêtes. Le levier « terminaison anticipée » de C1 est, à cette échelle, inopérant.
4. **D1 fait exactement ce qu'il annonce, et le témoin le prouve** : `_source` divisé
   par 3 tout pile, postings et subfields inchangés à l'octet, total −32,7 %, sans coût
   d'indexation (+5 % en réalité).

### Ils NE permettent PAS de conclure

1. **Rien sur les 28 917 511 documents.** Ni latence, ni p95, ni RSS, ni disque. À
   1,36 M l'index de surch fait 527 Mio sous une limite de 6 Gio : **aucun des deux
   moteurs n'habite son budget mémoire**, donc l'axe RSS n'est pas sollicité et le
   rapport 20× observé ne préjuge de rien à pleine échelle. C'est précisément le point
   que la campagne 28 M devait trancher, et il reste ouvert.
2. **Aucun « beat ES »**, dans aucun sens. Le seul écart net et favorable porte sur
   `match` mono-terme à petite échelle ; sur `bool.must`, surch est déjà en retard aux
   queues à 1,36 M, et la tendance à 28 M est inconnue.
3. **Rien sur D2** (bit-packing des postings), absent de ce SHA — et les postings sont
   ici 64 % du volume disque, donc le poste le plus lourd n'a reçu aucun traitement.
4. **Rien sur le comportement à froid** : c'est la série qui a fait échouer le run. Or
   c'est elle qui mesure ce qui se passe quand le cache de pages ne couvre plus l'index
   — soit exactement le régime disk-backed à 28 M. Son absence n'est pas un détail de
   plomberie : **c'est le trou qui reste dans la thèse.**
5. **Rien sur la tolérance de réconciliation disque à 12 Gio.** Le seuil
   `max(1 %, 16 Mio)` n'a jamais été mis sous tension : à 527 Mio il est large. Le
   risque signalé par la préparation reste entier et non testé.

### Verdict

**Aucun résultat 28 M n'est publiable, parce qu'aucun n'a été produit.** Le smoke est
vert sur ce qu'il tranche (scripts, disjonction des termes, manifeste, routage, parité,
métriques, ventilations, preuve d'activation C2) et ne tranche ni le p95 ni le RSS à
pleine échelle — je n'en tire donc aucune conclusion de performance à 28 M. Le blocage
n'est pas dans le moteur : il est dans une sonde de diagnostic dont le gate est, sur un
hôte sans swap, plus exigeant que ce que le noyau peut garantir. Lever ce blocage
suppose un arbitrage de protocole qui ne m'appartient pas.

---

## Artefacts (sur la VM, `ubuntu@51.159.170.64`)

| chemin | contenu |
|---|---|
| `~/out/smoke/` | smoke noyau 6.8 : `es.json`, `surch.json`, `*.by-form.jsonl`, `*.c2-stream.jsonl`, `*.cold_reclaim.tsv`, `surch.c2.*.prom` (scrapes Prometheus complets, source des compteurs C1) |
| `~/out/smoke-witness/` | témoin D1 `_source=doc`, mêmes familles d'artefacts |
| `~/out/smoke-echec-kernel515/` | premier smoke, conservé comme preuve de la cause n°1 |
| `~/logs/smoke.log`, `~/logs/smoke-witness.log`, `~/logs/smoke-echec-kernel515.log` | journaux complets |
| `~/out/smoke/by-form-ratios.jsonl` | **0 octet** — preuve que le calcul des ratios a avorté |
