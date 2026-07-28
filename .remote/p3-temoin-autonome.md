# P3 — témoin match autonome

## Changement livré

Le harnais `deploy/bench-local/fair-ab.sh` ne construit plus une séquence
alternée `bool.must(NOM=x, PRENOMS=y)` puis `match(NOM=x)`. Cette succession
préchargeait systématiquement le posting du témoin et le faisait différemment
selon A, B ou C.

Le protocole gelé est maintenant :

1. chauffe `match` sur 200 termes tiers ;
2. témoin autonome : 1 000 `match NOM=x`, `size:10` ;
3. chauffe `bool.must` sur les mêmes 200 termes tiers, mais après le témoin ;
4. 1 000 `bool.must size:10`, puis 1 000 `bool.must size:0` ;
5. sonde fixe `MARTIN`, conservée après les phases causales comme contrôle
   secondaire.

Le replay de l'ancien mélange 50/50 est disponible seulement avec
`P2_REPLAY_MIX_5050=1`. Il produit sa phase et son corps propres, après les
phases causales, et ne sert à aucun témoin ni gate causal.

## Disjonction et gel des entrées

La sélection parcourt le corpus dans son ordre naturel, sans `shuf`,
`$RANDOM` ni graine cachée. Un ordonnanceur pondéré répartit les candidats
mono-token ASCII sur l'ensemble du corpus, puis complète dans le même ordre
si nécessaire. Il produit :

- 1 000 couples `(NOM, PRENOMS)` à `bool.must` ;
- 1 000 `NOM` uniques pour `match-control` ;
- 200 couples de chauffe.

Les `NOM` sont uniques dans chaque ensemble, exclus de `MARTIN`, et les trois
ensembles sont vérifiés explicitement deux à deux. Une ligne non mono-token,
un cardinal incomplet, un doublon ou une intersection arrête le run : il n'y
a pas de repli silencieux.

Le manifeste SHA-256 gèle les sélections et les quatre corps causaux :
`bool-size10`, `bool-size0`, `match-control-size10` et le fichier de chauffe
(200 `match`, puis 200 `bool`). Le corps du replay et celui de `MARTIN` sont
aussi attestés lorsqu'ils existent. Un manifeste d'une autre version ou un
mode replay différent est refusé.

## Garde-fous du témoin

Toutes les requêtes passent par `_search?request_cache=false`. Chaque réponse
doit contenir un `hits.total.value` numérique strictement positif et chaque
réponse est canonisée. Le pilote de campagne compare les réponses canoniques
A/B, B/C et A/C pour `warm_match`, `match_control`, `warm_bool`,
`bool_size10`, `bool_size0` et `fixed_martin` : une divergence est fatale.

Avant et après `match_control`, les compteurs direct, generic,
`blocks_read` et `blocks_total` doivent tous avoir un delta nul. Pour C, les
octets P3 vérifiés doivent aussi rester stables pendant tout `match`; ils
doivent au contraire augmenter durant chaque phase bool. Les échecs de hash,
fallbacks et champs fallback non nuls sont invalidants.

## Télémétrie conservée

`surch.p2.telemetry.jsonl` reçoit un enregistrement à `index_ready`, puis
avant et après chaque phase. Il contient les jauges P3 demandées, la mémoire
du moteur (`postings_directory`, total, jemalloc allocated/active/resident/
retained), RSS/RssAnon/VmHWM, et les états cgroup. Ceux-ci comprennent
`memory.current`, les cinq champs `memory.stat` demandés, `io.stat` et son
delta de phase, PSI mémoire/IO, ainsi que `nr_throttled` et
`throttled_usec`.

Toute métrique attendue absente, illisible ou mal formée marque la mesure
INVALIDE. Les scripts de campagne, rapport et gate ont été alignés sur les
cinq phases causales, le contrôle `MARTIN` et le nouveau JSONL.

TEMOIN_DONE
