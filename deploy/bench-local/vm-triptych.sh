#!/usr/bin/env bash
# vm-triptych.sh — triptyque 28M@6g dos-à-dos sur VM dédiée CALME (SCW PRO2-M 16 vCPU/64 Go),
# en réponse aux anomalies du 2026-07-11 : les runs B (surch réf) et C (surch zstd) locaux
# n'étaient pas comparables (hôte chargé, cache fichier 171 vs 678 Mio au moment de la sonde,
# redémarrage du démon docker en plein run). Ici : trois runs séquentiels sur machine vide,
# mêmes requêtes de sonde par construction (LCG graine fixe, corpus identique).
#
# S'exécute SUR LA VM (root), après avoir déposé : fair-ab.sh, deces-28M.ndjson.zst,
# deces-mapping.json dans /root. Usage : nohup /root/vm-triptych.sh > /root/triptych.log 2>&1 &
#
# CPUSET=0-7 : la VM n'a que 16 vCPU (pas de topologie SMT N/N+16 comme le laptop) ;
# 8 vCPU aux moteurs, 8 au feeder/hôte — même partage qu'en local.
# NB : vCPU EPYC ≠ cœurs Ryzen du laptop — les chiffres ABSOLUS ne sont pas comparables
# à l'historique local ; seules les comparaisons INTERNES au triptyque comptent.
set -uo pipefail
cd /root

BULK=/root/deces-28M.ndjson
[ -s "$BULK" ] || zstd -d -T0 --force /root/deces-28M.ndjson.zst -o "$BULK"

REF_IMG=ghcr.io/rhanka/surch:sha-b795b100682afcfa65ab7db14f36d543cf039b38
ZSTD_IMG=ghcr.io/rhanka/surch:sha-d70f624c0b19385d641c666400848e29663897a9
COMMON="BULK_FILE=$BULK MAPPING_FILE=/root/deces-mapping.json PROBE_FIELD_NOM=NOM PROBE_FIELD_PRENOMS=PRENOMS CPUSET=0-7 MEM_LIMIT=6g POSTINGS_DISK=1"
BUDGETS="SURCH_FLUSH_BUDGET_BYTES=268435456 SURCH_MERGE_FANIN=8 SURCH_DENSIFY_BUDGET_DOCS=1000000 SURCH_MERGE_MAX_DOCS=7000000"

chmod +x /root/fair-ab.sh

echo "=== B' surch référence (sans compression) ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$REF_IMG" OUT_DIR=/root/out-6g \
  /root/fair-ab.sh > /root/runB.log 2>&1
tail -2 /root/out-6g/surch.json 2>/dev/null || echo "B' KO — voir runB.log"

echo "=== C' surch zstd (SURCH_SOURCE_COMPRESS=1) ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$ZSTD_IMG" SURCH_SOURCE_COMPRESS=1 OUT_DIR=/root/out-6g-compress \
  /root/fair-ab.sh > /root/runC.log 2>&1
tail -2 /root/out-6g-compress/surch.json 2>/dev/null || echo "C' KO — voir runC.log"

echo "=== D' Elasticsearch 8.6.1 ==="
env $COMMON ENGINES=es OUT_DIR=/root/out-6g \
  /root/fair-ab.sh > /root/runD.log 2>&1
tail -2 /root/out-6g/es.json 2>/dev/null || echo "D' KO — voir runD.log"

echo TRIPTYCH_DONE
