#!/usr/bin/env bash
# Bench BAN Paris 25k. Usage: bench.sh <label> <out_file> [<url>]
set -u
LABEL="${1:?label}"
OUT="${2:?out}"
URL="${3:-http://127.0.0.1:7700}"
INDEX="ban_paris_25000"
DATASET="/home/antoinefa/src/surch/target/ban-bench/ban-paris-25000.ndjson"

dt_ms() {
  awk -v a="$1" -v b="$2" 'BEGIN { printf "%.1f", (b-a)*1000 }'
}
took() { jq -r '.took // empty' <<<"$1"; }
hits_total() { jq -r '.hits.total.value // .hits.total // empty' <<<"$1"; }
hits_top_id() { jq -r '.hits.hits[0]._id // empty' <<<"$1"; }

post_search() {
  local q="$1"
  local body
  body=$(cat <<EOF
{"query":{"match":{"label":"$q"}},"size":10,"track_total_hits":true}
EOF
)
  local t0 t1
  t0=$(date +%s.%N)
  resp=$(curl -fsS -X POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' --data "$body")
  t1=$(date +%s.%N)
  printf 'search\t%s\tclient_ms=%s\ttook=%s\thits=%s\ttop=%s\n' \
    "$q" "$(dt_ms "$t0" "$t1")" "$(took "$resp")" "$(hits_total "$resp")" "$(hits_top_id "$resp")"
}

{
  echo "## bench label=$LABEL  $(date -Iseconds)"
  echo "binary_sha256=$(sha256sum /home/antoinefa/src/surch/target/release/surch-api | cut -d' ' -f1)"

  # reset index
  curl -fsS -X DELETE "$URL/$INDEX" >/dev/null 2>&1 || true
  curl -fsS -X PUT "$URL/$INDEX" -H 'Content-Type: application/json' -d '{}' >/dev/null

  # bulk ingest 25k
  t0=$(date +%s.%N)
  bulk_resp=$(curl -fsS -X POST "$URL/_bulk" \
    -H 'Content-Type: application/x-ndjson' \
    --data-binary "@$DATASET")
  t1=$(date +%s.%N)
  printf 'bulk\t-\tclient_ms=%s\ttook=%s\terrors=%s\n' \
    "$(dt_ms "$t0" "$t1")" "$(took "$bulk_resp")" "$(jq -r '.errors' <<<"$bulk_resp")"

  # refresh
  t0=$(date +%s.%N)
  curl -fsS -X POST "$URL/$INDEX/_refresh" >/dev/null
  t1=$(date +%s.%N)
  printf 'refresh\t-\tclient_ms=%s\n' "$(dt_ms "$t0" "$t1")"

  # count
  count_resp=$(curl -fsS "$URL/$INDEX/_count")
  printf 'count\t-\tcount=%s\n' "$(jq -r '.count' <<<"$count_resp")"

  # warmup 1 query each
  curl -fsS -X POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' \
    --data '{"query":{"match":{"label":"Rue Payenne"}}}' >/dev/null
  curl -fsS -X POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' \
    --data '{"query":{"match":{"label":"Place Patrice Chereau"}}}' >/dev/null

  # 5 iterations of each, full output
  for i in 1 2 3 4 5; do
    post_search "Rue Payenne"
  done
  for i in 1 2 3 4 5; do
    post_search "Place Patrice Chereau"
  done
} >>"$OUT" 2>&1
