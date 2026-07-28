# Fix5 — diagnostic du harnais P3 synthétique

Date : 2026-07-28
Branche : `main`
Portée : job CI `harnais P3 synthétique`, sans Docker réel, VM ni charge moteur.

## Cause racine prouvée

Le job P3 de `f984de9` utilisait `actions/checkout@v4` avec la profondeur par
défaut de un commit. Le journal GitHub Actions du run
`30377864690`, job `90338037483`, prouve les deux faits suivants :

1. le checkout exécute `git ... fetch --depth=1 origin
   +f984de9...` ;
2. la troisième étape démarre à `16:40:45.890` et termine code `1` à
   `16:40:45.945`, sans aucune ligne du script.

Le nouveau test lance le vrai `p2-campaign.sh`. Ce pilote archive les trois
commits contractuels `961ade1` (A), `6ce390e` (B) et `d0accd6` (C), et refuse
explicitement un SHA absent avec `git cat-file -e`. Aucun de ces objets n'est
accessible depuis un clone ne contenant que `f984de9`.

La reproduction minimale, exécutée avant la correction, est :

```bash
repo=$(mktemp -d /tmp/surch-p3-shallow.XXXXXX)
git clone --no-local --depth 1 --branch main file:///chemin/vers/surch "$repo/repo"
git -C "$repo/repo" rev-list --count HEAD
# 1
git -C "$repo/repo" cat-file -e '961ade10ffb74d78156aee8148f1e5c6bbbe6ba2^{commit}'
# fatal: Not a valid object name ... ; code 128
TMPDIR=/tmp P3_CAMPAIGN_KEEP_TMP=1 \
  bash "$repo/repo/deploy/bench-local/test-p3-campaign.sh"
# code 1, stdout 0 octet, stderr 0 octet sur f984de9
```

Le fichier temporaire conservé contient alors exactement :

```text
fatal: Not a valid object name 961ade10ffb74d78156aee8148f1e5c6bbbe6ba2^{commit}
[p2-campaign] SHA absent du clone local: 961ade10ffb74d78156aee8148f1e5c6bbbe6ba2
```

La cause ne dépend donc ni de jq, ni de mawk, ni d'une variable locale, ni du
filesystem : elle est le contrat incompatible entre un pilote qui exige trois
commits et un checkout qui ne télécharge que HEAD.

## Correctif

- Le job `p3-harness` demande `fetch-depth: 0`, limité à ce job. Les trois SHA
  contractuels sont donc disponibles pour `git cat-file` et `git archive`.
- `test-p3-campaign.sh` active `ERR` avec `errtrace` et `EXIT`. Il affiche dès
  le départ les versions Bash, jq, awk et Git. En erreur il écrit sur stderr :
  code de sortie, étape, assertion, commande et ligne observées, état des
  artefacts attendus, extrait du pilote capturé et extrait du journal du faux
  pilote.
- `test-p3-harness.sh` reçoit le même en-tête de versions et les pièges
  `ERR`/`EXIT`; ses échecs donnent l'étape, l'assertion ou commande, et
  l'inventaire des artefacts temporaires.

## Exemple de diagnostic obtenu

Après application de Fix5 au même clone superficiel, le scénario précédent
reste volontairement rouge, mais devient lisible :

```text
test-p3-campaign: versions bash=5.3.9(1)-release jq=jq-1.8.1 awk=mawk 1.3.4 20260129 git=git version 2.53.0
test-p3-campaign: ECHEC (code de sortie 1)
test-p3-campaign: étape: smoke du pilote P3
test-p3-campaign: assertion/commande: campaign_env smoke doit construire et vérifier les variantes A/B/C
test-p3-campaign: commande observée (ligne 376): env ... bash "$CAMPAIGN"
test-p3-campaign: artefact attendu absent ou vide: .../smoke/smoke-proof.json
test-p3-campaign: extrait du pilote capturé: .../smoke.out
fatal: Not a valid object name 961ade10ffb74d78156aee8148f1e5c6bbbe6ba2^{commit}
[p2-campaign] SHA absent du clone local: 961ade10ffb74d78156aee8148f1e5c6bbbe6ba2
```

Une injection locale distincte d'un `io.stat` invalide dans le harnais de
matrice a aussi produit :

```text
test-p3-harness: ECHEC (code de sortie 1)
test-p3-harness: étape: B1 — sérialisation cgroup v2
test-p3-harness: assertion/commande: io.stat doit devenir le JSON attendu
test-p3-harness: artefacts temporaires disponibles:
  io.stat (43 octets)
  io.json (0 octets)
```

## Vérifications et limites

- `bash -n deploy/bench-local/test-p3-campaign.sh`,
  `bash -n deploy/bench-local/test-p3-harness.sh` et
  `bash -n deploy/bench-local/p2-campaign.sh` sont verts.
- Les trois tests locaux demandés sont verts :
  `bash deploy/bench-local/test-p3-harness.sh`,
  `P3_MATRIX_EXHAUSTIVE=1 bash deploy/bench-local/test-p3-harness.sh` et
  `bash deploy/bench-local/test-p3-campaign.sh`. Chacun affiche sa version ;
  les deux premiers terminent `test-p3-harness: PASS`, le dernier
  `test-p3-campaign: PASS`.
- La reproduction Ubuntu 22.04 est verte : le conteneur installe Bash
  `5.1.16`, jq `1.6` et mawk `1.3.4`, configure `/src` comme dépôt Git sûr,
  puis `bash deploy/bench-local/test-p3-campaign.sh` termine
  `test-p3-campaign: PASS`.
- Le conteneur Ubuntu 22.04 installe Bash `5.1.16`, jq `1.6` et mawk `1.3.4`.
  Sans configuration Git sûre, le montage bind `/src` échoue volontairement
  sur l'ownership ; ce cas est un artefact de conteneur et n'est pas la cause
  CI, car `actions/checkout` configure explicitement le dépôt comme sûr.
- Le run CI du correctif n'est pas encore vert au moment de l'écriture : aucun
  état fermé, aucune preuve de campagne ou de performance ne sont revendiqués.

FIX5_DONE
