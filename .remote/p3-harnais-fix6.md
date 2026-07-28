# P3 harnais — correctif du bug jq smoke2, audit exhaustif des liaisons jq,
# nouvelle couverture du VRAI fair-ab.sh, message d'erreur corrigé, deux
# notes consignées

Répond aux six tâches demandées après le verdict INVALIDE de
`.remote/p3-smoke2-verdict.md` (variante C invalide avant toute phase de
mesure, VM détruite, 1 h 12, 0,90 EUR). Contrat de seuils
`.remote/p3-campagne-plan.md` inchangé.

## 1. Correctif

`deploy/bench-local/fair-ab.sh`, fonction `p2_metric_bundle_json` : le
filtre jq du bundle `p3_integrity` référençait `$directory` alors que la
valeur était déclarée sous `--argjson directory_bytes`. Corrigé en
`directory_bytes:$directory_bytes`.

En corrigeant, j'ai aussi éliminé la cause structurelle qui a permis au bug
de passer inaperçu : `p2_metric_bundle_json` était appelée via une
substitution de commande (`metric_json=$(p2_metric_bundle_json ...)`), donc
dans un sous-shell. Une variable de diagnostic assignée à l'intérieur de la
fonction ne pouvait pas survivre au retour de ce sous-shell — c'est
exactement ce qui empêchait de distinguer un bug jq d'une métrique absente
(tâche 4). La fonction publie maintenant son résultat et sa cause d'échec
dans deux globales (`P2_METRIC_BUNDLE_JSON`, `P2_METRIC_BUNDLE_REASON`), et
son unique point d'appel (`p2_capture_telemetry`) l'invoque directement,
sans `$(...)`. Un commentaire au-dessus de la fonction documente ce contrat
pour empêcher qu'un futur appel la re-enveloppe dans une substitution de
commande.

Vérification faite en conditions réelles, pas seulement en théorie : j'ai
réintroduit le bug exact dans `fair-ab.sh` (sed sur le fichier réel, pas une
copie), constaté l'échec de `test-p3-telemetry.sh` avec le même message
`jq: error: $directory is not defined` que celui du smoke2, puis restauré le
correctif et vérifié par `diff` que le fichier restauré est strictement
identique à l'état corrigé (voir tâche 3 pour le détail des logs).

## 2. Audit exhaustif des liaisons jq

Outil : `deploy/bench-local/check-jq-bindings.awk`, un scanner caractère par
caractère en awk POSIX (testé sous mawk 1.3.4 en local, attendu sous gawk
5.1 en CI — aucune construction gawk-only, pas d'`ENDFILE`, pas de
`gensub`). Pour chaque invocation `jq` détectée dans un script, il compare :
- l'ensemble des noms déclarés via `--arg`/`--argjson`/`--slurpfile`/`--rawfile` ;
- l'ensemble des `$identifiants` référencés dans le texte du filtre (le
  contenu entre apostrophes uniquement — jamais la valeur d'un `--arg`, en
  général entre guillemets doubles bash).

Il signale dans les deux sens : `DECLARED_NOT_USED` (déclaré, jamais
référencé dans le filtre) et `USED_NOT_DECLARED` (`$x` référencé, jamais
déclaré — la classe exacte du bug du smoke2). Il traite spécifiquement,
pour ne pas les confondre avec un `--arg` manquant :
- les liaisons locales `as $x`, `as [$a,$b]`, `as {k:$a}` ;
- les paramètres de valeur d'une définition `def f($x): ...` (sucre
  syntaxique jq pour `x as $x`) ;
- les variables automatiques `$ENV`, `$ARGS`, `$__loc__`, `$__prog_name__`.

Test versionné : `deploy/bench-local/test-jq-bindings.sh`, 11 assertions
(le bug historique, un cas sain, une déclaration inutile, une liaison
`as $x`, un paramètre `def f($x)`, un filtre multi-ligne avec positional de
fichier, un `$(jq ...)` imbriqué dans un `--argjson`, une non-régression du
bug de fusion de mots — voir plus bas —, un guillemet déséquilibré
fail-closed, et la réintroduction du bug sur une copie de `fair-ab.sh`
réel). Ajouté au job CI `p3-harness`.

### Deux bugs de scanner trouvés et corrigés pendant la construction de
l'outil (documentés pour la revue, pas cachés)

Le premier essai de l'outil a produit un résultat FAUSSEMENT rassurant :
98 invocations « saines » alors que 12 des 12 invocations réelles de
`fair-ab.sh` manquaient à l'appel (2 invisibles, 10 à des numéros de ligne
qui ne correspondaient à rien). Diagnostiqué par bissection dichotomique
(fichier tronqué à N lignes, état final imprimé) :

1. **Fusion de mots en fin de ligne.** Le mot en cours de construction
   n'était flushé qu'en rencontrant un espace/guillemet/opérateur, jamais en
   fin de ligne hors invocation. Résultat : `}` (fin d'une ligne) suivi de
   `fi` (ligne suivante) devenait le mot `}fi`. Quand ce mot glué
   chevauchait un `#` de commentaire, `word == ""` était faux et le
   commentaire n'était plus reconnu comme tel — une apostrophe française
   dans ce commentaire (`l'overlay`) togglait alors un faux guillemet
   simple, et le scanner restait bloqué en état « guillemet double » pour
   le reste du fichier. Corrigé en flushant le mot en fin de ligne NONE
   (continuation d'antislash exceptée), qu'une invocation soit active ou
   non. Couvert par le scénario 9 de `test-jq-bindings.sh`.
2. **`(` `)` `;` `|` `&` non traités comme frontières de mot hors
   invocation.** `$(jq -cn ...)` s'écrivait `integrity=$(jq` : la
   parenthèse ouvrante n'étant frontière de mot que si une invocation était
   déjà active, elle restait collée au mot précédent et `jq` seul
   n'apparaissait jamais — les deux appels de `p2_metric_bundle_json`
   (justement ceux du bug corrigé) disparaissaient purement et simplement
   de l'audit. Corrigé en traitant ces métacaractères comme frontières de
   mot inconditionnellement ; le suivi de profondeur de parenthèses et la
   terminaison d'invocation restent, eux, conditionnés à une invocation
   active.

Après ces deux corrections : 170 invocations détectées sur les six
scripts, 170 saines, 0 problème.

### Limite assumée, documentée, non mécanisée

Le scanner ne modélise pas la réinitialisation de contexte de citation bash
à l'intérieur d'un `$(...)` imbriqué. Quand un appel jq est imbriqué dans la
valeur `--argjson` d'un appel jq englobant (`p2-gate.sh:766`, qui regroupe
l'appel englobant et 3 appels imbriqués en un seul bassin
déclaré=34/utilisé=36 ; `test-p3-harness.sh:225`), les noms déclarés et
utilisés des deux appels sont regroupés dans un même bassin plutôt que
distingués par invocation. Revue manuelle de ces cas : sains. Cette limite
ne peut masquer que le cas rare d'un nom réutilisé entre l'appel englobant
et l'appel imbriqué ; elle ne peut jamais masquer la classe de bug qui a
invalidé le smoke2 (un `$x` qui n'apparaît nulle part comme déclaré, dans
tout le bassin).

### Liste exhaustive (170/170 saines, 0 problème)

```
fair-ab.sh        : 12 invocations (lignes 1064,1070,1374,1493,1508,1584,
                     1826,2886,2962,2963,2974,2988)
p2-campaign.sh    : 46 invocations (lignes 56–613)
p2-gate.sh        : 66 invocations (lignes 26–782)
p2-report.sh      :  6 invocations (lignes 37,53,68,291,319,324)
test-p3-harness.sh: 34 invocations (lignes 106–751)
test-p3-campaign.sh: 6 invocations (lignes 92,97,104,141,403,419)
TOTAL             : 170 invocations, 170 saines, 0 problème
```

Détail ligne par ligne (`declared=N used=M` ; M ≥ N est normal, une variable
peut être référencée plusieurs fois dans un filtre) :

```
OK	fair-ab.sh:1064	declared=0 used=0
OK	fair-ab.sh:1070	declared=0 used=0
OK	fair-ab.sh:1374	declared=0 used=0
OK	fair-ab.sh:1493	declared=12 used=12
OK	fair-ab.sh:1508	declared=7 used=7
OK	fair-ab.sh:1584	declared=22 used=22
OK	fair-ab.sh:1826	declared=0 used=0
OK	fair-ab.sh:2886	declared=0 used=0
OK	fair-ab.sh:2962	declared=0 used=0
OK	fair-ab.sh:2963	declared=0 used=0
OK	fair-ab.sh:2974	declared=0 used=0
OK	fair-ab.sh:2988	declared=0 used=0
OK	p2-campaign.sh:56	declared=4 used=4
OK	p2-campaign.sh:68	declared=1 used=1
OK	p2-campaign.sh:73	declared=0 used=0
OK	p2-campaign.sh:74	declared=0 used=0
OK	p2-campaign.sh:76	declared=0 used=0
OK	p2-campaign.sh:77	declared=0 used=0
OK	p2-campaign.sh:79	declared=0 used=0
OK	p2-campaign.sh:83	declared=1 used=1
OK	p2-campaign.sh:86	declared=1 used=1
OK	p2-campaign.sh:87	declared=5 used=7
OK	p2-campaign.sh:98	declared=0 used=0
OK	p2-campaign.sh:162	declared=9 used=9
OK	p2-campaign.sh:185	declared=4 used=4
OK	p2-campaign.sh:205	declared=4 used=4
OK	p2-campaign.sh:213	declared=3 used=3
OK	p2-campaign.sh:223	declared=0 used=0
OK	p2-campaign.sh:224	declared=0 used=0
OK	p2-campaign.sh:225	declared=0 used=0
OK	p2-campaign.sh:226	declared=0 used=0
OK	p2-campaign.sh:258	declared=3 used=5
OK	p2-campaign.sh:274	declared=1 used=2
OK	p2-campaign.sh:278	declared=1 used=4
OK	p2-campaign.sh:350	declared=0 used=0
OK	p2-campaign.sh:351	declared=0 used=0
OK	p2-campaign.sh:353	declared=7 used=7
OK	p2-campaign.sh:372	declared=0 used=0
OK	p2-campaign.sh:373	declared=0 used=0
OK	p2-campaign.sh:386	declared=7 used=7
OK	p2-campaign.sh:423	declared=0 used=0
OK	p2-campaign.sh:424	declared=0 used=0
OK	p2-campaign.sh:425	declared=0 used=0
OK	p2-campaign.sh:426	declared=0 used=0
OK	p2-campaign.sh:427	declared=0 used=0
OK	p2-campaign.sh:446	declared=3 used=3
OK	p2-campaign.sh:454	declared=0 used=0
OK	p2-campaign.sh:464	declared=0 used=0
OK	p2-campaign.sh:466	declared=1 used=1
OK	p2-campaign.sh:508	declared=4 used=4
OK	p2-campaign.sh:511	declared=0 used=0
OK	p2-campaign.sh:528	declared=0 used=0
OK	p2-campaign.sh:530	declared=0 used=0
OK	p2-campaign.sh:540	declared=6 used=6
OK	p2-campaign.sh:551	declared=0 used=0
OK	p2-campaign.sh:553	declared=1 used=2
OK	p2-campaign.sh:567	declared=3 used=3
OK	p2-campaign.sh:613	declared=7 used=7
OK	p2-gate.sh:26	declared=1 used=1
OK	p2-gate.sh:46	declared=0 used=0
OK	p2-gate.sh:63	declared=2 used=2
OK	p2-gate.sh:89	declared=0 used=0
OK	p2-gate.sh:90	declared=0 used=0
OK	p2-gate.sh:92	declared=0 used=0
OK	p2-gate.sh:96	declared=3 used=3
OK	p2-gate.sh:148	declared=3 used=3
OK	p2-gate.sh:178	declared=0 used=0
OK	p2-gate.sh:180	declared=0 used=0
OK	p2-gate.sh:192	declared=4 used=4
OK	p2-gate.sh:202	declared=0 used=0
OK	p2-gate.sh:252	declared=0 used=0
OK	p2-gate.sh:253	declared=0 used=0
OK	p2-gate.sh:261	declared=0 used=0
OK	p2-gate.sh:262	declared=0 used=0
OK	p2-gate.sh:263	declared=0 used=0
OK	p2-gate.sh:266	declared=0 used=0
OK	p2-gate.sh:267	declared=0 used=0
OK	p2-gate.sh:268	declared=7 used=9
OK	p2-gate.sh:272	declared=2 used=2
OK	p2-gate.sh:291	declared=0 used=0
OK	p2-gate.sh:292	declared=0 used=0
OK	p2-gate.sh:294	declared=0 used=0
OK	p2-gate.sh:299	declared=2 used=3
OK	p2-gate.sh:364	declared=4 used=4
OK	p2-gate.sh:371	declared=0 used=0
OK	p2-gate.sh:396	declared=0 used=0
OK	p2-gate.sh:445	declared=0 used=0
OK	p2-gate.sh:446	declared=0 used=0
OK	p2-gate.sh:447	declared=0 used=0
OK	p2-gate.sh:449	declared=0 used=0
OK	p2-gate.sh:457	declared=0 used=0
OK	p2-gate.sh:461	declared=2 used=2
OK	p2-gate.sh:471	declared=0 used=0
OK	p2-gate.sh:472	declared=0 used=0
OK	p2-gate.sh:473	declared=0 used=0
OK	p2-gate.sh:477	declared=0 used=0
OK	p2-gate.sh:497	declared=0 used=0
OK	p2-gate.sh:501	declared=2 used=2
OK	p2-gate.sh:506	declared=0 used=0
OK	p2-gate.sh:510	declared=2 used=2
OK	p2-gate.sh:520	declared=0 used=0
OK	p2-gate.sh:521	declared=0 used=0
OK	p2-gate.sh:522	declared=0 used=0
OK	p2-gate.sh:526	declared=0 used=0
OK	p2-gate.sh:544	declared=0 used=0
OK	p2-gate.sh:545	declared=1 used=1
OK	p2-gate.sh:559	declared=0 used=0
OK	p2-gate.sh:560	declared=0 used=0
OK	p2-gate.sh:647	declared=0 used=0
OK	p2-gate.sh:648	declared=0 used=0
OK	p2-gate.sh:649	declared=0 used=0
OK	p2-gate.sh:650	declared=0 used=0
OK	p2-gate.sh:651	declared=0 used=0
OK	p2-gate.sh:652	declared=0 used=0
OK	p2-gate.sh:659	declared=0 used=0
OK	p2-gate.sh:668	declared=3 used=3
OK	p2-gate.sh:712	declared=0 used=0
OK	p2-gate.sh:714	declared=0 used=0
OK	p2-gate.sh:727	declared=0 used=0
OK	p2-gate.sh:731	declared=1 used=2
OK	p2-gate.sh:735	declared=0 used=0
OK	p2-gate.sh:765	declared=0 used=0
OK	p2-gate.sh:766	declared=34 used=36  (bassin regroupé : 1 appel englobant + 3 imbriqués via $(jq ...))
OK	p2-gate.sh:782	declared=0 used=0
OK	p2-report.sh:37	declared=0 used=0
OK	p2-report.sh:53	declared=0 used=0
OK	p2-report.sh:68	declared=0 used=0
OK	p2-report.sh:291	declared=18 used=18
OK	p2-report.sh:319	declared=6 used=6
OK	p2-report.sh:324	declared=6 used=6
OK	test-p3-harness.sh:106	declared=0 used=0
OK	test-p3-harness.sh:175	declared=3 used=3
OK	test-p3-harness.sh:182	declared=0 used=0
OK	test-p3-harness.sh:208	declared=11 used=15
OK	test-p3-harness.sh:215	declared=8 used=16
OK	test-p3-harness.sh:220	declared=9 used=17
OK	test-p3-harness.sh:225	declared=7 used=9  (bassin regroupé : appel englobant + 1 imbriqué via $(jq ...))
OK	test-p3-harness.sh:240	declared=0 used=0
OK	test-p3-harness.sh:241	declared=0 used=0
OK	test-p3-harness.sh:242	declared=6 used=7
OK	test-p3-harness.sh:244	declared=4 used=4
OK	test-p3-harness.sh:311	declared=5 used=6
OK	test-p3-harness.sh:329	declared=5 used=6
OK	test-p3-harness.sh:340	declared=1 used=1
OK	test-p3-harness.sh:354	declared=2 used=5
OK	test-p3-harness.sh:374	declared=2 used=2
OK	test-p3-harness.sh:386	declared=2 used=2
OK	test-p3-harness.sh:399	declared=2 used=2
OK	test-p3-harness.sh:414	declared=3 used=3
OK	test-p3-harness.sh:432	declared=0 used=0
OK	test-p3-harness.sh:433	declared=0 used=0
OK	test-p3-harness.sh:434	declared=4 used=4
OK	test-p3-harness.sh:437	declared=4 used=4
OK	test-p3-harness.sh:457	declared=1 used=1
OK	test-p3-harness.sh:523	declared=0 used=0
OK	test-p3-harness.sh:551	declared=0 used=0
OK	test-p3-harness.sh:552	declared=0 used=0
OK	test-p3-harness.sh:553	declared=0 used=0
OK	test-p3-harness.sh:585	declared=1 used=1
OK	test-p3-harness.sh:609	declared=0 used=0
OK	test-p3-harness.sh:627	declared=0 used=0
OK	test-p3-harness.sh:662	declared=0 used=0
OK	test-p3-harness.sh:668	declared=0 used=0
OK	test-p3-harness.sh:751	declared=1 used=1
OK	test-p3-campaign.sh:92	declared=3 used=3
OK	test-p3-campaign.sh:97	declared=4 used=4
OK	test-p3-campaign.sh:104	declared=11 used=13
OK	test-p3-campaign.sh:141	declared=1 used=1
OK	test-p3-campaign.sh:403	declared=0 used=0
OK	test-p3-campaign.sh:419	declared=0 used=0
```

Reproductible : `awk -f deploy/bench-local/check-jq-bindings.awk deploy/bench-local/fair-ab.sh deploy/bench-local/p2-campaign.sh deploy/bench-local/p2-gate.sh deploy/bench-local/p2-report.sh deploy/bench-local/test-p3-harness.sh deploy/bench-local/test-p3-campaign.sh`.

## 3. Casser le motif : couverture du VRAI fair-ab.sh

`deploy/bench-local/test-p3-telemetry.sh`. Contrairement à
`test-p3-campaign.sh` (qui remplace `fair-ab.sh` entier par `fake-fair-ab`)
et à `test-p3-harness.sh` (qui n'extrayait jamais les fonctions de
construction du bundle P3), ce test extrait par `awk` — depuis le VRAI
`fair-ab.sh`, jamais un mock — les fonctions réellement exécutées :
`p2_metric_present`, `p2_snapshot_metrics`, `p2_metric_value`,
`p2_cgroup_directory`, `p2_cgroup_stat_value`, `p2_cgroup_io_json`,
`p2_cgroup_io_delta_json`, `p2_psi_json`, `p2_metric_bundle_json`,
`p2_counter_value`, `p2_number_equal`, `p2_number_le`,
`p2_segment_value_valid`, `p2_cpu_steal_percent`, et `err`. Il les `source`
et les appelle directement contre une sortie Prometheus synthétique
(valeurs reprises telles quelles du snapshot C réel du smoke2 :
`integrity_bytes=2592256`, `integrity_pages=79648`,
`directory_bytes=103278432`, etc.), sans Docker, sans moteur, sans VM.

Huit assertions :
- **T1** : bundle `p3_integrity` réel, variante C nominale — exerce
  précisément la branche qui a échoué en smoke2 ;
- **T2** : variante A, `p3_integrity` reste `null` sans exiger les
  métriques P3 (elles ne sont jamais publiées par A/B) ;
- **T3** : métrique réellement absente (`directory_bytes` retiré du
  snapshot) → raison `prometheus_metric_missing_surch_postings_p2_directory_bytes` ;
- **T4** : réintroduction mécanisée du bug ($directory au lieu de
  $directory_bytes) dans une COPIE des helpers extraits (jamais le fichier
  commité) → raison `prometheus_bundle_jq_error`, message contenant la
  sortie jq brute (« is not defined ») ;
- **T5** : `p2_counter_value` — absence de série = zéro documenté,
  présence = vraie valeur ;
- **T6** : `p2_number_equal`/`p2_number_le` — comparaisons y compris zéro
  (piège maison « 0 est vrai en jq », vérifié ici côté awk) ;
- **T7** : `p2_segment_value_valid` — gate `exact` et `minimum` ;
- **T8** : `p2_cpu_steal_percent` — delta correct et rejet fail-closed d'un
  delta négatif.

### Preuve que le test mord, sur le fichier réel (pas seulement une fixture)

```
$ sed -i 's/directory_bytes:\$directory_bytes}/directory_bytes:$directory}/' deploy/bench-local/fair-ab.sh
$ bash deploy/bench-local/test-p3-telemetry.sh
[1;31m[fair-ab][0m p2_metric_bundle_json : jq a échoué en construisant p3_integrity
(bug de construction, pas une métrique manquante) : jq: error: $directory is
not defined at <top-level>, line 1, column 283: ...
jq: 1 compile error
[test-p3-telemetry] ECHEC (T1 — bundle p3_integrity, variante C nominale) :
T1: p2_metric_bundle_json a échoué en nominal (raison=prometheus_bundle_jq_error)
EXIT_WITH_BUG=1
```

Puis restauration, vérifiée par `diff` (identique octet pour octet à l'état
corrigé) et par une nouvelle exécution PASS de `test-p3-telemetry.sh`,
`test-p3-harness.sh` (normal et `P3_MATRIX_EXHAUSTIVE=1`) et
`test-p3-campaign.sh`.

Note de méthode : ma première tentative de restauration a utilisé
`git checkout -- fair-ab.sh`, qui a effacé le correctif ENTIER (non commité
à ce stade), pas seulement le bug réintroduit. Détecté immédiatement (le
`diff` avec ma sauvegarde de travail ne matchait plus), le correctif complet
a été reconstruit à l'identique (vérifié par `diff` byte-à-byte avec la
sauvegarde antérieure) avant de continuer. Rapporté ici par probité : aucune
fermeture n'a été déclarée avant cette vérification.

## 4. Message d'erreur corrigé

Avant : toute défaillance de `p2_metric_bundle_json` — qu'une métrique soit
vraiment absente du snapshot Prometheus OU que jq échoue à construire le
JSON avec des métriques par ailleurs toutes présentes — remontait sous le
même motif générique `${phase}_${boundary}_prometheus_metric_missing`. C'est
ce motif exact qui a produit le diagnostic trompeur du smoke2
(`index_ready_snapshot_prometheus_metric_missing`) alors que les 18
métriques P3 attendues étaient bien dans le snapshot.

Après : chaque échec de lecture d'une métrique individuelle
(`p2_metric_value`) fixe une raison nommée précisément
(`prometheus_metric_missing_<nom_de_métrique>`) ; chaque échec de
construction jq (l'un des deux `jq -cn` de la fonction) fixe la raison
`prometheus_bundle_jq_error` et journalise la sortie d'erreur jq brute via
`err(...)`, capturée dans un fichier temporaire dédié (`2>"$jq_err_file"`,
supprimé après lecture) plutôt que d'être laissée se mélanger silencieusement
au flux du script. Les deux causes ne peuvent plus jamais partager le même
motif. Vérifié par T3 (vraie absence → motif nommant la métrique) et T4
(bug jq → motif `prometheus_bundle_jq_error` + sortie jq brute dans le log)
de `test-p3-telemetry.sh`, et par la reproduction réelle en tâche 1/3
ci-dessus.

## 5. Contrainte consignée — stockage containerd (sans implémentation)

Ajoutée dans `.remote/p3-campagne-plan.md`, section « Contrainte consignée
pour le prochain essai full — stockage containerd » : Docker 29 utilise le
snapshotter `io.containerd.snapshotter.v1`, dont les couches résident dans
`/var/lib/containerd` sur la racine `/dev/sda1` (`7 385 217 699` octets
constatés, ≈ 7,4 Gio), pas sur le volume dédié `/var/lib/docker`
(`/dev/sdb`, 64 Gio) même quand `DockerRootDir` pointe correctement dessus.
Le dimensionnement disque d'un prochain full ne peut donc pas présumer que
les 64 Gio du volume Docker suffisent. Non implémenté, comme demandé : la
décision (déplacer `data-root` containerd, agrandir la racine, ou
revalider la marge actuelle) reste à trancher avant le prochain essai.

## 6. Fait de mesure consigné, sans conclusion

Ajouté dans `.remote/p3-campagne-plan.md`, section « Fait de mesure
consigné, sans conclusion — ratio de blocs de B » : la variante B a terminé
techniquement valide au smoke v4 avec un ratio de blocs
`0,278468388` puis `0,253467300` sur ses deux phases bool, l'un et l'autre
au-dessus de la cible `0,25` du gate P3. Reporté comme information brute à
vérifier à pleine échelle (12 segments réels contre 3 au smoke), sans tirer
de conclusion sur le p95 ou le RSS.

## Vérifications exécutées avant ce commit

```
bash -n deploy/bench-local/fair-ab.sh                              -> OK
bash -n deploy/bench-local/test-jq-bindings.sh                     -> OK
bash -n deploy/bench-local/test-p3-telemetry.sh                    -> OK
awk -f deploy/bench-local/check-jq-bindings.awk /dev/null          -> OK (parse)
bash deploy/bench-local/test-p3-harness.sh                         -> PASS
P3_MATRIX_EXHAUSTIVE=1 bash deploy/bench-local/test-p3-harness.sh  -> PASS
bash deploy/bench-local/test-p3-campaign.sh                        -> PASS
bash deploy/bench-local/test-jq-bindings.sh                        -> PASS (11 assertions)
bash deploy/bench-local/test-p3-telemetry.sh                       -> PASS (8 assertions)
```

Aucun `cargo build/check/test/clippy` lancé en local. Aucun Docker, aucune
VM, aucun gros workload. Toutes ces commandes sont des scripts bash/awk
autorisés.

## Évaluation honnête du risque qu'un 4e essai échoue encore

**Ce qui est fermé par une preuve versionnée et rejouable par un tiers :**
- le bug exact du smoke2 (variable) — corrigé, reproduit puis re-corrigé en
  conditions réelles, deux mécanismes de détection indépendants (audit
  statique `check-jq-bindings.awk` + exécution réelle
  `test-p3-telemetry.sh`) le referment chacun séparément si quelqu'un le
  réintroduit ;
- le message trompeur — corrigé et testé sur les deux branches (T3/T4) ;
- l'audit couvre la totalité des invocations jq du harnais (170/170), pas
  seulement la fonction fautive — deux bugs de scanner découverts en cours
  de route (fusion de mots, `(`/`)` non-frontières) montrent que la
  vérification a été faite avec un outil qui a lui-même été mis en défaut
  et corrigé avant d'être fiable, pas accepté au premier succès apparent.

**Ce qui reste un risque réel pour un 4e essai, honnêtement :**
- **Le smoke complet A/B/C n'a jamais été rejoué de bout en bout sur une
  VM** depuis ce correctif. Tout ce qui a été vérifié ici l'a été SANS
  Docker et SANS moteur réel (contrainte explicite de cette tâche). Le
  chemin réseau Docker → curl → Prometheus → `p2_snapshot_metrics` (le
  `docker run curlimages/curl` qui produit le fichier `.prom` consommé par
  `p2_metric_bundle_json`) n'est testé nulle part dans ce lot : seule la
  fonction qui CONSOMME un fichier `.prom` déjà produit est exercée. Un bug
  dans la production de ce fichier (format Prometheus inattendu du moteur
  réel, latence de scrape, etc.) resterait invisible ici.
- **La limite du bassin regroupé pour `$(jq ...)` imbriqué** (`p2-gate.sh:766`,
  `test-p3-harness.sh:225`) est une zone angle mort documentée mais non
  mécanisée : un futur bug localisé spécifiquement dans l'un des appels
  imbriqués, où le nom fautif coïnciderait avec un nom valide déclaré par
  l'appel englobant, ne serait pas détecté par cet audit. Risque jugé
  faible (2 occurrences sur 170, toutes revues manuellement saines) mais
  non nul.
- **Le containerd/disque (tâche 5) et le ratio de blocs B (tâche 6) ne sont
  pas résolus**, volontairement : ce sont des contraintes/faits consignés
  pour trancher AVANT le prochain full, pas des blocages levés. Un 4e essai
  qui ignorerait la note containerd pourrait échouer sur l'espace disque
  racine, indépendamment de tout ce qui est corrigé ici.
- **La classe de bug plus large** (un script shell qui compile/s'exécute
  différemment de ce qu'un relecteur humain suppose) n'est couverte par cet
  audit que pour la liaison `--arg`/`--argjson` ↔ `$var` jq. D'autres
  classes similaires (ex. une variable bash mal nommée dans un `awk -v`,
  une incohérence entre le nom d'un fichier temporaire produit et celui
  attendu ailleurs) ne sont pas auditées par cet outil et resteraient à
  découvrir de la même manière malchanceuse qu'aux essais 1 à 3, sauf
  extension future du même principe (source réel + fixture synthétique)
  à d'autres frontières du harnais.

Verdict : le point précis qui a fait échouer le smoke2 est fermé,
doublement gardé, et vérifié en conditions réelles plutôt que supposé. Le
risque principal d'un 4e essai n'est plus ce bug, mais tout ce qui reste
Docker-dépendant et jamais rejoué de bout en bout depuis — irréductible
sans relancer une VM, ce que cette tâche interdisait explicitement.

FIX6_DONE
