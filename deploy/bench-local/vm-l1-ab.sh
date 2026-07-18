#!/usr/bin/env bash
# vm-l1-ab.sh — A/B 28M@6g des fetchs `_source` parallèles (commit 18e1c25, flag
# SURCH_SOURCE_FETCH_PARALLEL) sur VM SCW PRO2-M dédiée calme, même méthode que
# vm-triptych.sh/vm-packing-gates.sh. Config commune = la meilleure connue
# (packing 2c dans l'image + zstd 2b actif) ; SEUL le flag change entre H0 et H1.
# Attendu (verdict-28M-6g-2026-07-11.md §3bis) : p50 aléatoire ~28 ms ≈ 10 preads
# séquentiels × ~2,8 ms (stockage bloc SBS) -> H1 vise p50 ÷2-3 (réf ES : 11,5 ms).
# Préalables : fair-ab.sh, deces-28M.ndjson.zst, deces-mapping.json dans /root.
# Usage : nohup /root/vm-l1-ab.sh > /root/l1-ab.log 2>&1 &
set -uo pipefail
cd /root

BULK=/root/deces-28M.ndjson
[ -s "$BULK" ] || zstd -d -T0 --force /root/deces-28M.ndjson.zst -o "$BULK"

IMG=ghcr.io/rhanka/surch:sha-18e1c25e86198579860c37df30a02d5ec536137e
COMMON="BULK_FILE=$BULK MAPPING_FILE=/root/deces-mapping.json PROBE_FIELD_NOM=NOM PROBE_FIELD_PRENOMS=PRENOMS CPUSET=0-7 MEM_LIMIT=6g POSTINGS_DISK=1 SURCH_SOURCE_COMPRESS=1"
BUDGETS="SURCH_FLUSH_BUDGET_BYTES=268435456 SURCH_MERGE_FANIN=8 SURCH_DENSIFY_BUDGET_DOCS=1000000 SURCH_MERGE_MAX_DOCS=7000000"

chmod +x /root/fair-ab.sh

echo "=== H0 fetchs séquentiels (flag off — référence, même image) ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$IMG" OUT_DIR=/root/out-l1-h0 \
  /root/fair-ab.sh > /root/runH0.log 2>&1
tail -2 /root/out-l1-h0/surch.json 2>/dev/null || echo "H0 KO — voir runH0.log"

echo "=== H1 fetchs parallèles (SURCH_SOURCE_FETCH_PARALLEL=1) ==="
env $COMMON $BUDGETS ENGINES=surch SURCH_IMAGE="$IMG" SURCH_SOURCE_FETCH_PARALLEL=1 OUT_DIR=/root/out-l1-h1 \
  /root/fair-ab.sh > /root/runH1.log 2>&1
tail -2 /root/out-l1-h1/surch.json 2>/dev/null || echo "H1 KO — voir runH1.log"

echo L1_AB_DONE
