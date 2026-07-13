#!/usr/bin/env bash
# vm-packing-gates.sh — gates 28M du packing side-table 2c (plan
# docs/paper/plan-packing-sidetable-2026-07-12.md, étape 4) sur la même VM calme
# que vm-triptych.sh. Trois runs séquentiels, image sha-20792db… (= triptyque + packing) :
#   E  surch packing @6g            -> vérif correction à l'échelle + anon attendu ~3,22 -> ~2,79 Go
#   F  surch packing+zstd @4g       -> LA re-tentative : transient ~4,15 -> ~3,7 Go attendu, doit PASSER
#   G  idem F, 2e run               -> le plancher n'est déclaré que reproductible (leçon du PASS non-reproductible du 06/07)
# Préalables identiques à vm-triptych.sh (corpus, mapping, fair-ab.sh dans /root).
# Usage : nohup /root/vm-packing-gates.sh > /root/packing-gates.log 2>&1 &
set -uo pipefail
cd /root

BULK=/root/deces-28M.ndjson
[ -s "$BULK" ] || zstd -d -T0 --force /root/deces-28M.ndjson.zst -o "$BULK"

PACK_IMG=ghcr.io/rhanka/surch:sha-20792db2501c09cd4b3492ea457f31082a16e7b5
COMMON="BULK_FILE=$BULK MAPPING_FILE=/root/deces-mapping.json PROBE_FIELD_NOM=NOM PROBE_FIELD_PRENOMS=PRENOMS CPUSET=0-7 POSTINGS_DISK=1"
BUDGETS="SURCH_FLUSH_BUDGET_BYTES=268435456 SURCH_MERGE_FANIN=8 SURCH_DENSIFY_BUDGET_DOCS=1000000 SURCH_MERGE_MAX_DOCS=7000000"

chmod +x /root/fair-ab.sh
docker pull -q "$PACK_IMG" >/dev/null

echo "=== E packing @6g (sans zstd — mesure l'anon du packing seul vs B'=3228 Mo) ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$PACK_IMG" MEM_LIMIT=6g OUT_DIR=/root/out-pack-6g \
  /root/fair-ab.sh > /root/runE.log 2>&1
tail -2 /root/out-pack-6g/surch.json 2>/dev/null || echo "E KO — voir runE.log"

echo "=== F packing+zstd @4g (re-tentative plancher, 1er run) ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$PACK_IMG" SURCH_SOURCE_COMPRESS=1 MEM_LIMIT=4g OUT_DIR=/root/out-pack-4g-r1 \
  /root/fair-ab.sh > /root/runF.log 2>&1
tail -2 /root/out-pack-4g-r1/surch.json 2>/dev/null || echo "F KO — voir runF.log"

echo "=== G packing+zstd @4g (reproductibilité, 2e run) ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$PACK_IMG" SURCH_SOURCE_COMPRESS=1 MEM_LIMIT=4g OUT_DIR=/root/out-pack-4g-r2 \
  /root/fair-ab.sh > /root/runG.log 2>&1
tail -2 /root/out-pack-4g-r2/surch.json 2>/dev/null || echo "G KO — voir runG.log"

echo PACKING_GATES_DONE
