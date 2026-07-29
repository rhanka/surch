# Diagnostic du garde-fou BEIR nDCG — run 30451966000 (2026-07-29)

## Verdict en une ligne

L'echec n'est **pas** une regression du chantier de recherche. C'est un
**plancher perime** : les seuils du gate dataient du 2026-05-26 et etaient
anterieurs au commit `07a58fc` (2026-06-09) qui aligne volontairement le
scoring BM25 sur la quantization `SmallFloat` de Lucene. Le job
`beir-extra-ndcg` n'ayant pas retourne pendant **64 jours**, personne n'a
re-etalonne les planchers apres ce changement delibere.

Le chemin `match` mono-terme streame (`C2`, `333b902`) et la compression
`_source` par blocs (`D1`, `04e9da6`) sont **hors de cause**, prouve deux
fois (structurellement et par la mesure).

---

## 1. La cause reelle, avec le journal a l'appui

Le workflow archivait **deja** les journaux par conteneur dans l'artefact
`k8s-bench-beir-extra-ndcg-188906f4…` (etape « Collect logs + report »,
`if: always()`, qui tourne bien avant la suppression du Job). Ils n'etaient
simplement jamais **affiches** dans la console. Aucune re-execution n'a donc
ete necessaire pour trouver la cause : l'artefact suffisait.

`beir-extra-ndcg-p62v2.ndcg-driver.log`, integralement :

```text
Waiting for Surch + OpenSearch...
Both engines healthy.
BEIR_GATE status=FAIL reason=nfcorpus NDCG@10=0.3021 below frozen floor=0.3033
```

Etats des conteneurs (`pods.describe.txt`) : `surch` et `opensearch`
terminent en `Exit Code: 143` (SIGTERM de fin de Job, normal), `ndcg-driver`
en `Exit Code: 1`. Conditions du Job : `FailureTarget=True`, `Failed=True`.
Aucun `OOMKilled`, aucun `ImagePullBackOff`, aucun probleme de PVC, aucun
depassement de duree. **La cause est le verdict du gate lui-meme.**

### Pourquoi les chiffres avaient disparu

Le driver calcule les **quatre** mesures (nfcorpus×2, fiqa×2) dans une
boucle, *puis* seulement applique le gate. Au run 30451966000 les quatre
mesures avaient donc bien ete calculees — puis perdues, pour deux raisons
cumulees :

1. `gate_fail` faisait `exit 1` sur le **premier** plancher rate, avant
   d'avoir imprime quoi que ce soit et avant d'ecrire `summary.md` ;
2. la recuperation de secours `kubectl cp -c ndcg-driver /reports/…` ne peut
   pas s'executer une fois le driver mort (plus d'`exec` possible), donc
   aucun `*-surch.out` n'est present dans l'artefact.

Consequence concrete : on savait que nfcorpus etait sous son plancher, on
ignorait totalement le verdict de fiqa.

---

## 2. Attribution : bisection par images GHCR, en local, cout cloud nul

Les images `ghcr.io/rhanka/surch:sha-*` sont publiees pour la plupart des
commits. nfcorpus fait 3 633 documents et une mesure complete prend **12
secondes** en local — la bisection n'a donc coute aucune minute de cluster.
Protocole : meme corpus BEIR, meme script `scripts/bench/beir-ndcg.sh`
(inchange depuis), conteneur plafonne a 4 Gio / 2 CPU.

### Fidelite local ↔ cluster : 5 correspondances exactes

| grandeur | mesure en pod (artefact) | mesure locale | ecart |
|---|---|---|---|
| nfcorpus surch @188906f | 0.3021 (run 30451966000) | 0.3021 | 0 |
| nfcorpus surch @f34d006 | 0.3033 (run 26476471207) | 0.3033 | 0 |
| nfcorpus OpenSearch 2.17.1 | 0.3034 (run 26476471207) | 0.3034 | 0 |
| fiqa surch @f34d006 | 0.2294 (run 26476471207) | 0.2294 | 0 |
| fiqa OpenSearch 2.17.1 | 0.2389 (run 26476471207) | 0.2389 | 0 |
| scifact surch @188906f | 0.6599 (runs 06-15/06-30/07-03) | 0.6599 | 0 |

La mesure locale est donc un substitut fidele : BM25 est deterministe, les
corpus sont figes et les images sont les binaires exacts de la CI.

### Courbe nfcorpus, 26 points du 2026-05-26 au 2026-07-29

```text
2026-05-28  9b7e632e  0.3033      2026-07-03  94e11a89  0.3021
2026-05-29  8aae6a1a  0.3033      2026-07-03  a664ed8b  0.3021
2026-05-30  3bfec8f0  0.3033      2026-07-03  69668db4  0.3021
2026-06-01  55820746  0.3033      2026-07-05  6af8a052  0.3021
2026-06-02  706d5396  0.3033      2026-07-06  76674ed4  0.3021
2026-06-03  3ae72b8e  0.3033      2026-07-06  ac3f12af  0.3021
2026-06-05  96f43913  0.3033      2026-07-06  af65940e  0.3021
2026-06-09  136016a5  0.3033  <-- dernier point AVANT SmallFloat
2026-06-09  eb3e1883  0.3021  <-- premier point APRES SmallFloat
2026-06-10  9faac87d  0.3021      2026-07-06  d259d402  0.3021
2026-06-11  7a649417  0.3021      2026-07-06  2a83d046  0.3021
2026-06-29  af94e52c  0.3021      2026-07-06  23b28b6f  0.3021
2026-07-01  ccc0851a  0.3021      2026-07-06  b795b100  0.3021
2026-07-02  cbf7771e  0.3021      2026-07-11  d70f624c  0.3021
2026-07-02  51a4f701  0.3021      2026-07-12  20792db2  0.3021
2026-07-03  22b27476  0.3021      2026-07-18  18e1c25e  0.3021
                                  2026-07-24  e2cb0780  0.3021
                                  2026-07-24  56bf32fd  0.3021
                                  2026-07-25  961ade10  0.3021
                                  2026-07-26  6ce390e5  0.3021
                                  2026-07-28  d0accd6e  0.3021
                                  2026-07-29  188906f4  0.3021
```

La transition est enfermee dans l'intervalle `136016a5..eb3e1883`, entierement
date du 2026-06-09. Le seul commit de cet intervalle qui touche au **scoring**
est :

```text
07a58fc [18-ndcg-smallfloat] feat(scoring): quantize doc_len via Lucene
        SmallFloat (parite NDCG TREC-COVID + ~65 MiB)
```

Tous les autres commits de l'intervalle sont du stockage `_source`
(deflate/zstd/mmap) et leurs reverts : ils ne peuvent pas deplacer un
classement. Le commit lui-meme annonce l'intention :

> TREC-COVID NDCG@10 0.4750 vs OS 0.4902 (-0.0152) etait entierement
> explique par le scoring BM25 cote Surch qui utilisait des doc_len exacts
> (Vec<u64>) la ou Lucene utilise une quantization 1 byte/doc via
> SmallFloat.intToByte4 / byte4ToInt.

Depuis le 2026-06-09, la valeur est **stable au bit pres sur 22 points de
mesure** — multi-segment, merge tiere, postings sur disque, compression
`_source`, C2 et D1 compris.

### Effet mesure du SmallFloat, sur les quatre jeux

| jeu | avant (≤ 2026-06-09) | apres | delta |
|---|---|---|---|
| scifact | 0.6576 | 0.6599 | **+0.0023** |
| trec-covid | 0.4750 (annonce commit) | 0.4777 (mesure en pod) | **+0.0027** |
| nfcorpus | 0.3033 | 0.3021 | −0.0012 |
| fiqa | 0.2294 | 0.2274 | −0.0020 |

C'est le profil attendu d'une quantization : elle regroupe des documents
dans le meme godet de longueur et redistribue les egalites. Elle gagne sur
deux jeux, perd un peu sur deux autres. Le choix etait delibere et
documente (`docs/paper/ndcg-trec-covid-rootcause-22.md`, #22).

---

## 3. C2 et D1 sont hors de cause — double preuve

**Preuve structurelle.** Le gate n'emet que des requetes `multi_match` :

```sh
# scripts/bench/beir-ndcg.sh:187
body=$(printf '{"query":{"multi_match":{"query":"%s","fields":["title","text"]}},…')
```

Or le routage C2 ne se declenche que sur `SearchQuery::Match` :

```rust
// crates/surch-api/src/search.rs:2461-2467
let streame = match query {
    SearchQuery::Match { field, value, operator }
        if *operator != MatchOperator::And => reader.single_term_match_topk(field, value, limit),
    _ => None,   // <-- MultiMatch tombe ici
};
```

`MultiMatch` prend la branche `_ => None` et retombe integralement sur
`topk_scored_documents_reference`. **Le chemin streame n'est jamais
emprunte par ce garde-fou**, quelle que soit sa correction.

**Preuve par la mesure.** `d0accd6e` (2026-07-28, juste avant C2 et D1) et
`188906f4` (2026-07-29, apres C2 **et** D1) donnent des valeurs identiques :

| jeu | d0accd6e (avant C2+D1) | 188906f4 (apres C2+D1) |
|---|---|---|
| nfcorpus | 0.3021 | 0.3021 |
| fiqa | 0.2274 | 0.2274 |

Le gain de latence de C2 n'a donc ete paye par **aucune** perte de
pertinence sur ce garde-fou.

---

## 4. Etat de la qualite aujourd'hui (188906f)

| jeu | surch | OpenSearch 2.17.1 | ecart | ancien plancher (05-26) |
|---|---|---|---|---|
| nfcorpus | 0.3021 | 0.3034 | −0.0013 | 0.3033 ❌ |
| fiqa | 0.2274 | 0.2389 | −0.0115 | 0.2294 ❌ |
| scifact *(non gate)* | 0.6599 | 0.6537 | **+0.0062** | — |
| trec-covid *(non gate)* | 0.4777 | 0.4902 | −0.0125 | — |

Note importante : **les deux jeux gates echouaient**, pas seulement
nfcorpus — et sur les deux criteres. Outre les planchers, l'ecart a
OpenSearch depassait aussi son plafond (nfcorpus 0.0013 > 0.0010 ;
fiqa 0.0115 > 0.0100). Le run 30451966000 s'arretait avant de le decouvrir.

---

## 5. Ce qui a ete corrige

### a) `deploy/k8s/jobs/beir-extra-ndcg.yaml` — re-etalonnage trace

Planchers portes aux valeurs post-SmallFloat **reellement mesurees, sans
marge** (toute baisse ulterieure d'un cran de 1e-4 echoue toujours) :

| variable | avant | apres |
|---|---|---|
| `NFCORPUS_NDCG_FLOOR` | 0.3033 | **0.3021** |
| `FIQA_NDCG_FLOOR` | 0.2294 | **0.2274** |
| `NFCORPUS_MAX_OS_GAP` | 0.0010 | **0.0015** |
| `FIQA_MAX_OS_GAP` | 0.0100 | **0.0120** |

Les ecarts OpenSearch recoivent un cran de marge au-dela de l'ecart observe,
pour une raison purement numerique : `0.3021 - 0.3034` vaut
`-0.0013000000000000123` en double IEEE754, et un plafond fixe a exactement
`0.0013` echouerait sur le bruit de representation. Les planchers, eux,
restent exacts : les deux cotes de la comparaison viennent du meme `%.4f`.

L'en-tete du manifeste porte toute la provenance : date, commit fautif,
tableau des quatre deltas, et la mention explicite que resserrer les ecarts
a OpenSearch reste un travail de qualite **ouvert**.

### b) Le driver imprime ses mesures AVANT de juger

C'est la correction de fond. Le driver :

1. n'interrompt plus la campagne quand une mesure echoue (il l'enregistre et
   continue), pour que le bloc de mesures parte toujours ;
2. ecrit `summary.md` et emet `BEGIN_SURCH_K8S_SUMMARY … END` **avant toute
   decision de gate** — les quatre mesures sont donc dans
   `kubectl logs -c ndcg-driver` *et*, via les marqueurs que le workflow sait
   deja reconstruire, dans le resume GitHub, y compris en cas d'echec ;
3. evalue **les deux jeux** et accumule les echecs au lieu de sortir au
   premier — un plancher rate sur nfcorpus ne masque plus le verdict de fiqa ;
4. emet `bench.json` (schema `v3`) avec les valeurs observees et le statut,
   les valeurs etant serialisees en chaines pour qu'une mesure ratee donne
   `""` et non un JSON casse.

Teste hors cluster sous `sh` sur cinq scenarios (script du manifeste extrait
par `kubectl create --dry-run -o jsonpath`, mesures simulees) :

| scenario | attendu | obtenu |
|---|---|---|
| valeurs reelles 0.3021/0.3034 + 0.2274/0.2389 | PASS, exit 0 | ✅ PASS, exit 0 |
| nfcorpus a 0.3020 (−1e-4) | FAIL, fiqa quand meme evalue | ✅ nfcorpus FAIL + fiqa PASS, exit 1 |
| ecart OpenSearch elargi | FAIL | ✅ FAIL, exit 1 |
| mesure fiqa/surch plantee | FAIL, mesures nfcorpus conservees | ✅ exit 1, 3 mesures dans le journal |
| les deux jeux en regression | les deux rapportes | ✅ 4 motifs, exit 1 |

La sensibilite du garde-fou est donc conservee : il detecte toujours une
derive de 1e-4.

### c) `.github/workflows/ci-k8s.yml` — vidange de diagnostic en console

Nouvelle etape **`Dump diagnostics (on failure, before Job deletion)`**,
declaree **avant** `Delete Job (always)` — les steps s'executent dans leur
ordre de declaration, un `always()` place plus bas ne peut donc pas
supprimer le Job avant elle. Elle affiche, en `::group::` replies :

- l'etat des **PVC** du namespace ;
- le `describe` du **Job** et ses conditions (avec `reason` et `message`) ;
- le `describe` de **chaque pod** ;
- les **journaux de chaque conteneur** du pod — init containers (`surch`,
  `opensearch`) *et* conteneur principal (`ndcg-driver`) — plus le
  `--previous` quand il existe reellement ;
- les 60 derniers **events** du namespace ;
- un verdict synthetique pousse dans le resume GitHub.

Points de robustesse : `set +e` en tete (aucune commande de diagnostic ne
peut interrompre la vidange), `exit 0` en fin (la vidange ne masque pas la
cause d'echec initiale), le `grep` de fin protege par `|| true` (un `grep`
sans correspondance renvoie 1 et avalerait le reste du bloc), et un
avertissement explicite quand aucun Job ou aucun pod n'a ete resolu.

La condition est **`failure() || cancelled()`** et non `failure()` seul : le
job GitHub porte `timeout-minutes: 35` alors que la boucle d'attente se
donne `SCW_MAX_DURATION_MIN=60`. Un Job reellement bloque est donc **annule**
par GitHub avant que la boucle n'atteigne son propre plafond, et
`failure()` serait faux — l'echec le plus opaque de tous serait reste muet.

L'etape « Collect logs + report » collecte en plus desormais l'etat des PVC
(`get pvc -o wide` et `describe pvc`) dans l'artefact.

`actionlint` (avec shellcheck) passe : `rc=0`, aucune remarque sur la
nouvelle etape.

---

## 6. Confirmation sur le cluster — le gate est VERT

Manifeste corrige applique a la main sur `poc-…` / namespace `surch`, avec la
substitution exacte du workflow (`envsubst '${SURCH_SHA}'`,
`SURCH_SHA=188906f414e1ff6ecde3a566d61742e190fc32b0`) et les images deja
publiees — **aucune reconstruction d'image, une seule execution cluster**.

Pod `beir-extra-ndcg-2w2jx`, Job `Complete=True`, `SuccessCriteriaMet=True`.
Journal du conteneur `ndcg-driver`, integralement conserve avant suppression :

```text
BEGIN_SURCH_K8S_SUMMARY
- Etalon courant : 2026-07-29 (run 30451966000)
- Planchers : nfcorpus >= 0.3021, fiqa >= 0.2274
- Ecart OpenSearch max : nfcorpus 0.0015, fiqa 0.0120

## nfcorpus / surch    NDCG@10 = 0.3021   Recall@10 = 0.1488   (323/323 qids)
## nfcorpus / os       NDCG@10 = 0.3034   Recall@10 = 0.1495   (323/323 qids)
## fiqa / surch        NDCG@10 = 0.2274   Recall@10 = 0.2941   (648/648 qids)
## fiqa / os           NDCG@10 = 0.2389   Recall@10 = 0.3004   (648/648 qids)
END_SURCH_K8S_SUMMARY
BEIR_GATE dataset=nfcorpus status=PASS … surch_ndcg=0.3021 opensearch_ndcg=0.3034
BEIR_GATE dataset=fiqa     status=PASS … surch_ndcg=0.2274 opensearch_ndcg=0.2389
BEIR_GATE status=PASS failures=0
beir-extra-ndcg complete.
```

Trois choses sont prouvees d'un coup par ce journal :

1. **le garde-fou repasse au vert** sur les deux jeux et sur les deux
   criteres, avec les planchers re-etalonnes ;
2. **les quatre mesures sont imprimees AVANT le verdict** — c'etait la
   correction de fond, et elle tient dans le vrai `/bin/sh` de l'image de
   bench, pas seulement dans le `sh` local du test ;
3. **la fidelite local ↔ cluster monte a 6/6** : la valeur fiqa/surch a
   188906f, jusque-la seulement predite en local (0.2274), est confirmee au
   chiffre pres en pod. La bisection locale etait donc bien un substitut
   valide, et elle a coute zero minute de cluster.

Job supprime apres capture des journaux ; namespace `surch` laisse vide.
Cout total : **une seule execution cluster d'environ 13 minutes**, tres en
deca du plafond de 2 EUR / 60 minutes.

---

## 7. Ce qui reste ouvert

1. **L'ecart a OpenSearch s'est elargi sur nfcorpus et fiqa** depuis le
   SmallFloat : nfcorpus 0.0001 → 0.0013, fiqa 0.0095 → 0.0115. C'est le
   revers du gain sur scifact et trec-covid. Ce n'est pas un blocage du
   garde-fou, mais c'est un vrai sujet de qualite : la quantization censee
   *rapprocher* de Lucene nous en **eloigne** sur ces deux jeux. A arbitrer.
2. **`ndcg-gate` (scifact + trec-covid) n'a aucun plancher** : il mesure et
   publie, sans jamais echouer sur une baisse. C'est le trou le plus large du
   dispositif — les deux jeux les plus suivis n'ont, en pratique, pas de
   garde-fou automatique. Les valeurs sont pourtant stables et connues
   (scifact 0.6599 / 0.4777 trec-covid), donc le plancher est ecrivable
   immediatement.
3. **`timeout-minutes: 35` vs `SCW_MAX_DURATION_MIN: 60`** : incoherence
   laissee en l'etat (elle est desormais couverte par `cancelled()`, mais la
   vraie correction serait d'aligner les deux plafonds).
4. **La cadence du garde-fou** : `beir-extra-ndcg` n'avait pas tourne depuis
   **64 jours**. Aucun re-etalonnage n'est possible si le gate ne tourne pas ;
   c'est ce qui a transforme un choix de design assume en echec CI opaque
   deux mois plus tard.
5. **Aucun push effectue**, conformement a la consigne. La confirmation
   cluster a tourne sur les images `sha-188906f4…` deja publiees, sans
   reconstruction. En revanche HEAD est maintenant `d908c44` : relancer
   `ci-k8s.yml` depuis GitHub exigera l'image `sha-<HEAD>`, donc une chaine
   `docker-build` prealable une fois ces commits pousses. Le garde-fou est
   deja prouve vert sur le fond ; ce run-la ne validera que la mecanique du
   workflow.
