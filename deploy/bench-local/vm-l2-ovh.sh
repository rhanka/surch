#!/usr/bin/env bash
# vm-l2-ovh.sh — run L2 instrumenté sur le stockage CIBLE réel : volume OVH Public Cloud
# de type `classic` (l'équivalent Cinder du storageClass K8s `block-standard` utilisé par
# deploy/k8s/jobs/00-init-corpora.yaml depuis la PR #9).
#
# Pourquoi cette machine et pas celle des runs précédents : L1 (docs/paper/verdict-28M-6g-2026-07-11.md
# §3quater) a été mesuré sur du bloc Scaleway SBS, et son hypothèse « 10 preads séquentiels
# dominent le p50 » n'a jamais été instrumentée. L2 ne cherche PAS un gain : il répond à
# « où partent les 20-250 ms d'une requête aléatoire ? » (pread / decode zstd / parse JSON / reste).
#
# Placement disque VOULU (c'est le cœur du protocole) :
#   - `source.dat` + tout l'index Surch  -> /mnt/blockstd (volume `classic` OVH, docker data-root)
#   - corpus NDJSON 14 Go                -> disque système local (lu à l'indexation seulement,
#                                           jamais pendant les sondes -> n'influence pas la mesure)
#
# Contraintes de quota du projet OVH (6 cœurs / 25 Go RAM / 30 Go volume, dont 2 vCPU + 8 Go
# déjà pris par le node K8s) : instance b3-16 = 4 vCPU / 16 Go. D'où CPUSET=0-2 (3 cœurs
# moteur, 1 pour l'hôte + feeder) au lieu des 8 des runs SBS. Les chiffres ABSOLUS ne sont donc
# comparables ni au laptop ni aux runs SBS ; seules comptent (a) la décomposition en phases,
# (b) la comparaison interne H0/H1 de ce run.
#
# Usage : nohup /home/ubuntu/vm-l2-ovh.sh > /home/ubuntu/l2-ovh.log 2>&1 &
set -uo pipefail
cd /home/ubuntu

BULK=/home/ubuntu/deces-28M.ndjson
[ -s "$BULK" ] || zstd -d -T0 --force /home/ubuntu/deces-28M.ndjson.zst -o "$BULK"

IMG=ghcr.io/rhanka/surch:sha-56bf32fd78ec539fc63c27d853c3b1cd4e2a741f
COMMON="BULK_FILE=$BULK MAPPING_FILE=/home/ubuntu/deces-mapping.json PROBE_FIELD_NOM=NOM PROBE_FIELD_PRENOMS=PRENOMS CPUSET=0-2 MEM_LIMIT=6g POSTINGS_DISK=1 SURCH_SOURCE_COMPRESS=1 SURCH_SOURCE_FETCH_PROFILE=1"
BUDGETS="SURCH_FLUSH_BUDGET_BYTES=268435456 SURCH_MERGE_FANIN=8 SURCH_DENSIFY_BUDGET_DOCS=1000000 SURCH_MERGE_MAX_DOCS=7000000"

chmod +x /home/ubuntu/fair-ab.sh
docker pull -q "$IMG" >/dev/null

echo "=== H0 séquentiel, profil L2 actif ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$IMG" OUT_DIR=/home/ubuntu/out-l2-h0 \
  /home/ubuntu/fair-ab.sh > /home/ubuntu/runH0.log 2>&1
echo "exit_h0=$?"
tail -2 /home/ubuntu/out-l2-h0/surch.json 2>/dev/null || echo "H0 KO — voir runH0.log"

echo "=== H1 parallèle, profil L2 actif ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$IMG" SURCH_SOURCE_FETCH_PARALLEL=1 OUT_DIR=/home/ubuntu/out-l2-h1 \
  /home/ubuntu/fair-ab.sh > /home/ubuntu/runH1.log 2>&1
echo "exit_h1=$?"
tail -2 /home/ubuntu/out-l2-h1/surch.json 2>/dev/null || echo "H1 KO — voir runH1.log"

echo L2_OVH_DONE
