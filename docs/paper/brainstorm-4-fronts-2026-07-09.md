# Brainstorm post-parité — les 4 fronts (2026-07-09, Fable 5)

Contexte : la parité de plancher 28M@4g est atteinte (commit `ea86930` — RSS 3,01 vs ES 3,24 GiB,
indexation 21,8k vs 24,5k doc/s, latence sonde 0,35/0,55/0,86 ms, disque 18,6 vs 12,6 GiB, oracle 0
divergence). Trois retours utilisateur déclenchent ce brainstorm :

1. **Vocabulaire figé** : « indexation (doc/s) » (jamais « débit » seul) ; « RSS conteneur » =
   Resident Set Size, mémoire physique réellement occupée (heap + page cache, cgroup
   `memory.current`) — à définir à la première occurrence de chaque rapport.
2. **« Habiter le budget »** : à un plancher de 4 GiB, n'en occuper que 3 = 1 GiB de cache
   potentiellement gaspillé (ES occupe 3,24/4).
3. **Sonde non-aléatoire** : le vrai test matchID (artillery) tire des noms aléatoires ; la sonde
   fair-ab répète UNE requête fixe (`match nom:MARTIN`) → latences best-case cache, non bankables.
   Corollaire : le RSS 3,01 est en partie un artefact de cette sonde (working set minuscule).

Brainstorm mené par Fable 5 (xhigh) sur la base du ledger complet réussites/échecs
(`design-segments-pic-borne-2026-07-05.md`, `local-fair-ab-2026-07-04.md`, lecture du code
`fair-ab.sh`, `artillery-replay.sh`, `state.rs::source_store`, cycle de vie des tempfiles).

---

## (a) Priorisation

| # | Front | Gain | Effort | Risque | Verdict |
|---|---|---|---|---|---|
| **1** | **Sonde aléatoire / latence honnête** | Bloquant épistémique : toute claim latence actuelle (2,5-3,4×) est invalide tant que la sonde est 1 requête fixe (best-case page cache, ~30 pages chaudes). Aucun gain moteur, 100 % gain de vérité. | **S** (1-2 j, bash pur, Sonnet-délégable) | Nul (mesure). Risque = découvrir que le p99 random s'inverse — c'est le but. | **PRIORITÉ 1 ABSOLUE** — préalable aux fronts 2 et 3 (on ne dimensionne ni cache ni disque à l'aveugle) |
| **2** | **« Habiter le budget » (cache adaptatif)** | Incertain AVANT la mesure #1 : le « 1 GiB gaspillé » est probablement un artefact de la sonde fixe (working set minuscule → le page cache n'a rien à remplir). Sous requêtes aléatoires, le noyau remplira SEUL le cap. | XS (fadvise-warm) à L (vrai cache) | **Élevé si cache applicatif** : re-créer du résident non-évictable = re-OOM, la maladie qu'on vient de tuer en 5 tranches S5 | **TRANCHÉ : PAS de cache applicatif.** Seul un warm `fadvise` XS retenu. Critère de réouverture chiffré ci-dessous. |
| **3** | **Disque 1,5× (18,6 vs 12,6 GiB)** | −30-40 % disque attendu (zstd `_source` ≈ 5-7 GiB de JSON brut) ET gain latence random indirect : moins d'octets à cacher = meilleur hit-rate sous cap. | **M** (ventilation 0,5 j ; zstd `_source` 3-5 j) | Faible-moyen (format `source.dat`, coût décompression au fetch ~10-20 µs/doc, borné par size=10) | **PRIORITÉ 2** — LA mitigation structurelle du thrash anticipé au front #1 |
| **4** | **NRT S4** | Dernier axe ES-gagnant (6× pire en refresh-par-chunk). Gain net et démontrable (REFRESH_EACH=1). | **L** (5-10 j : C2+tombstones+C4+C5+C7) | Élevé (concurrence merge background, contrat update C7) | **PRIORITÉ 4** — le plan S4 du doc reste juste et a RÉTRÉCI (C1 déjà livré). Amendement : gater C2+tombstones en merge SYNCHRONE d'abord, C4 (background) en dernier. |

**Ordre d'exécution : 1 → 3 → (2 si critère déclenché) → 4.** Le front 1 est le juge des fronts 2
et 3 ; le front 3 est la première mitigation du front 1 ; le front 2 n'existe qu'en cas d'échec
mesuré ; le front 4 vient quand la latence est bankable.

---

## (b1) Design front #1 — sonde aléatoire + artillery borné

### Diagnostic de la sonde actuelle (ancré code)

`fair-ab.sh:256-261` : `PROBE_REQUESTS=1000` × le MÊME body `{"match":{"nom":"MARTIN"}}`. Working
set ≈ 1 TermEntry + 1 directory + quelques blocs FoR + 10 slots `source.dat` — quelques dizaines de
pages, chaudes dès la requête 2. Best-case des DEUX côtés (ES en profite aussi), mais ça ne prouve
RIEN sur le régime matchID réel (noms aléatoires, `packages/deces-backend` test-perf-v1,
clients_test.csv).

### Design (4 pièces, toutes dans le harnais, zéro code moteur)

**P1 — Échantillon de requêtes déterministe, tiré du corpus.** Extraire du `$BULK` déjà construit
(lignes paires = docs JSON) un `probe_names.tsv` de N=10 000 paires `nom\tprenoms` par awk avec
échantillonnage déterministe (pas de `shuf` sans graine, pas de `$RANDOM` : 1 doc toutes les
`NDOCS/10000` lignes — reproductible ET identique pour ES et surch). Propriété clé :
**échantillonner le corpus = tirage pondéré par la fréquence** → MARTIN sort proportionnellement à
sa présence → distribution Zipf naturelle, exactement le trafic matchID. On n'invente pas une
distribution, on la copie.

**P2 — Sonde random dans `run_engine` (en PLUS de la fixe, pas à la place).** Même mécanique
(1 conteneur curl, boucle, `time_total` — le spawn est HORS mesure, contrairement à artillery-replay
où `t0` précède le fork curl). Mix 50/50 comme artillery-replay : `match nom:X` / `bool must [match
nom:X, match prenoms:Y]`, `size:10` (**obligatoire** : force le fetch `_source` → exerce
`source.dat` en accès aléatoire, le poste page-cache le plus gros). Séquence d'indices pré-générée
(même ordre pour les 2 moteurs). Sortie : `lat_rand_p50/p95/p99` à côté des `lat_p50/95/99` fixes
(continuité historique).

**P3 — Sonde « cold » par `memory.reclaim` (cgroup v2).** Surch n'a pas de persistance → impossible
de redémarrer le conteneur pour vider le cache. Mais cgroup v2 permet
`echo <bytes> > /sys/fs/cgroup/system.slice/docker-<cid>.scope/memory.reclaim` : éviction du page
cache DU conteneur seul (hôte intact, pas de `drop_caches` global). Séquence : sonde random warm →
reclaim agressif → re-sonde random → `lat_cold_p50/95/99`. Best-effort (skip si absent). Seul moyen
honnête de mesurer le cold-path pread des DEUX moteurs.

**P4 — Artillery borné (saturation, pas précision).** `artillery-replay.sh` existe et a la bonne
forme (phases 2→50 RPS, mix matchID) mais cible `deces_25k` en dur, échantillon d'un autre dataset,
et fork bash+curl par requête (dt_ms inclut le spawn ≈ ms → inutilisable en sub-ms, VALIDE pour la
saturation). Refactor : paramétrer `INDEX`/`URL`/`NAMES` (réutiliser `probe_names.tsv`), brancher
sur le réseau fair-ab post-indexation, p95 par phase + delta warm→50RPS. Rôles distincts assumés :
curl-loop = latence, artillery = tenue en charge.

**Honnêteté de la mesure RAM sous random** : le RSS-conteneur va MONTER vers le cap (page cache qui
se remplit) — comportement voulu, pas une régression. Gate mémoire = OOM/no-OOM ; ajouter la capture
`memory.stat` (anon vs file) au JSON pour distinguer résident applicatif et cache.

### Anticipation chiffrée (à infirmer par la mesure — c'est son rôle)

28M@4g : 18,6 GiB de fichiers, ~1-1,7 GiB de marge cache (cap 4 − anon ~2,3-3). Par requête random :
~2-5 TermEntry (1 page chacun) + 2-5 directories + blocs FoR du leapfrog (5-50 pages selon df) +
10 pages `source.dat` ≈ **20-80 pages touchées**. Grâce au tirage Zipf, le head (MARTIN/JEAN…) reste
chaud ; le tail paie des preads NVMe ~60-100 µs sérialisés. Prévision : **p50 ~0,4-0,7 ms
(l'avantage 2-3× tient), p95 ~1-2 ms, p99 ~3-8 ms (parité voire inversion possible vs ES)** — ES
joue au même jeu mais avec MOINS d'octets à cacher (d'où front #3 = mitigation n°1). Mitigations en
ordre : (1) réduire les octets (front #3), (2) warm fadvise hiérarchisé, (3) réduire les
preads/requête (inline du 1er BlockDirEntry dans TermEntry — écarté en S5b, à ne rouvrir QUE si le
profil montre les directories dominants), (4) pinning résident = dernier recours, jamais par défaut.
Le hot-set déjà résident (roaring + FST) couvre la résolution de terme mais PAS le payload ni
`_source` — il ne suffit probablement pas seul, et c'est précisément ce que P3 mesurera.

---

## (b2) Front #2 tranché — page cache vs cache applicatif : **le page cache gagne**

1. **Le page cache EST déjà le cache adaptatif cgroup-aware demandé.** En cgroup v2, le page cache
   des fichiers du conteneur est chargé à `memory.current` et **réclaimé par le noyau sous
   pression** — remplir-la-marge + s'évincer-sous-pression, c'est sa définition, gratuite, sans
   code, sans risque OOM. Le « 1 GiB gaspillé » n'existe que parce que la sonde fixe ne touche
   rien : sous trafic random, le noyau remplira seul le cap. **ES ne « remplit son budget » par
   magie non plus : sa moitié non-heap, c'est exactement du page cache mmap Lucene** — le même
   mécanisme que nous.
2. **Un cache applicatif est de l'anon non-réclaimable** : le noyau ne peut pas l'évincer, chaque
   octet réduit la marge du page cache d'autant, un dimensionnement raté re-OOM — on re-créerait la
   maladie B tuée en 5 tranches S5. Le ledger l'a déjà tranché une fois (« cache LRU RAM du
   directory : écarté — le page cache EST le cache », S5b).
3. **Ce qu'un cache applicatif pourrait théoriquement battre** : (i) la granularité — un TermEntry =
   28 o mais coûte une page de 4 KiB (amplification 146×) ; (ii) des objets décodés (FoR
   décompressé) = gain CPU. Or le CPU n'est pas le goulot (0,35 ms p50) et l'amplification de
   granularité se traite en CHAUFFANT les tables, pas en les dupliquant en anon.
4. **Seule action retenue (XS, évictable, sans risque)** : warm
   `posix_fadvise(POSIX_FADV_WILLNEED)` hiérarchisé post-quiescence — d'abord TermEntry + block
   directories (touchés par TOUTE requête, quelques centaines de Mo), puis les FoR par df
   décroissant, en s'arrêtant à `memory.max − memory.current − marge`. C'est « habiter le budget »
   version noyau : pages chauffées dans le cap, comptées, évincées gracieusement si la charge réelle
   en veut d'autres. Best-effort, 0 risque OOM, ~1 j.

**Critère de réouverture chiffré (engagement, pas une porte fermée)** : si après tranches 1+2,
`p95_random(surch) > p95_random(ES)` @28M@4g **ET** que le profil montre >50 % des misses sur les
tables métadonnées (pas le payload), alors — et seulement alors — cache TermEntry borné à watermark
(`memory.max − memory.current`, cap dur ~256 MiB, éviction sous `memory.pressure` PSI), design à
repasser en double consensus. Sinon le sujet est clos.

---

## (c) Plan d'exécution — 2 premières tranches (Sonnet-délégable)

### Tranche 1 — « latence honnête » (fair-ab v2) — ~1-2 j, bash uniquement, zéro code Rust

| Étape | Périmètre exact | Gate |
|---|---|---|
| 1a | `fair-ab.sh` : génération `probe_names.tsv` (10k paires nom/prenoms, échantillonnage déterministe pas-fixe depuis `$BULK`, pondération Zipf naturelle) + séquence d'indices pré-générée partagée ES/surch | fichier identique sur 2 runs (déterminisme) ; mêmes requêtes aux 2 moteurs |
| 1b | Sonde random 50/50 match/bool `size:10` dans `run_engine`, champs `lat_rand_*` ajoutés au JSON, sonde fixe CONSERVÉE | smoke 1,36M@1536m : les 2 moteurs produisent lat fixe + rand ; lat fixe ≈ historique (non-régression du harnais) |
| 1c | Sonde cold via `memory.reclaim` du scope docker (best-effort, skip propre si absent) + capture `memory.stat` anon/file dans le JSON | `lat_cold_* ≥ lat_rand_*` observé (sanity) ; skip silencieux documenté sinon |
| 1d | Refactor `artillery-replay.sh` : INDEX/URL/NAMES paramétrés, branchement réseau fair-ab, p95 par phase 2→50 RPS | run 1,36M : p95 par phase pour les 2 moteurs, pas d'erreurs HTTP |
| **1e** | **LE RUN VERDICT : 28M@4g, ES + surch, sonde fixe + random + cold + artillery** (réglage acquis : flush 256M, densify 1M, fanin 8, cap tier 7M) | tableau complet ; l'anticipation (b1) confirmée ou infirmée ; **aucune claim latence antérieure ne survit à ce run — c'est lui la nouvelle référence** |

Découpage délégation : 1a+1b un lot, 1c+1d un lot, 1e = run piloté (pas de code). Interdits hérités
du ledger : pas de `$RANDOM` non-seedé, pas de mesure incluant le spawn curl, jamais comparer anon
vs RSS-conteneur.

### Tranche 2 — disque (ventilation puis compression) — ~4-6 j

| Étape | Périmètre exact | Gate |
|---|---|---|
| 2a | Ventilation MESURÉE : `ls -la` du volume surch dans le rapport fair-ab (avant teardown) + croiser avec les gauges (`postings_segment_bytes`, `disk_subfield_values_bytes`, taille `source.dat`) ; **vérifier la réclamation** : nb de fichiers `surch-postings-*.dat` vivants == nb de segments vivants à quiescence (`Drop`→`remove_file` existe mais un `Arc<Segment>` retenu par un registre serait invisible sans ce comptage) | table de ventilation 28M@4g expliquant ≥90 % des 18,6 GiB ; verdict réclamation OK/KO |
| 2b | zstd `_source` par blocs : `source_store` (append-only non compressé aujourd'hui) → blocs compressés de docs adjacents (~64-128 KiB, zstd niveau 1-3 ; comparable ES = LZ4 blocs 16 KiB), fetch top-10 = ≤10 blocs décompressés ; flag `SURCH_SOURCE_COMPRESS`, off = format actuel bit-identique | CI ; oracle-local 0 div ; fair-ab 1,36M : disque en nette baisse, `lat_rand_p95` dégradé <10 % ; **28M@4g : disque ≤ ~13-14 GiB (≈ parité ES) SANS régression p95 random** — et re-mesurer lat_rand/cold : la baisse d'octets doit AMÉLIORER le hit-rate (lien front 3 → front 1 à démontrer) |
| 2c | (si 2a montre des reliquats) fix réclamation ciblé | fichiers orphelins = 0 à quiescence |

Chaque étape commit-able, gatée `fair-ab.sh` + oracle-local, flags `SURCH_*` réversibles, validation
via ci-k8s (jamais de cargo test local, jamais de gros corpus hors harnais docker pinné).

---

**Résumé décisionnel** : la sonde aléatoire d'abord (rien d'autre n'est défendable sans elle) ; le
disque ensuite (optimisation ET mitigation n°1 du thrash) ; « habiter le budget » est tranché — le
page cache le fait déjà, on l'aide d'un fadvise, on ne construit pas de résident ; S4 NRT reste
juste tel qu'écrit, allégé de C1 déjà livré, et vient quand la latence est devenue bankable.
