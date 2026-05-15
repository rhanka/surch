#!/usr/bin/env bash
# Bench INSEE deces multi_match (NOM + PRENOMS), matchID-style.
# Usage: insee-bench-multi.sh <label> <out_file> <url> [iterations]
set -u
LABEL="${1:?label}"
OUT="${2:?out}"
URL="${3:-http://127.0.0.1:7700}"
ITER="${4:-50}"
INDEX="deces_25k"
DATASET="/home/antoinefa/src/surch/target/insee/deces_25k.ndjson"
NAMES="/home/antoinefa/src/surch/target/insee/full_names_sample.txt"

dt_ms() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.1f", (b-a)*1000 }'; }
took() { jq -r '.took // empty' <<<"$1"; }
hits_total() { jq -r '.hits.total.value // .hits.total // empty' <<<"$1"; }
hits_top_id() { jq -r '.hits.hits[0]._id // empty' <<<"$1"; }

# Build full-name samples: "PRENOMS NOM" from the dataset
if [ ! -s "$NAMES" ]; then
  grep '"PRENOMS":' "$DATASET" \
    | jq -r 'select(.PRENOMS != "" and .NOM != "") | "\(.PRENOMS) \(.NOM)"' \
    | sort -u | shuf -n 200 > "$NAMES"
fi

{
  echo "## bench label=$LABEL  $(date -Iseconds)"

  # reset + create index
  curl -fsS -X DELETE "$URL/$INDEX" >/dev/null 2>&1 || true
  curl -fsS -X PUT "$URL/$INDEX" -H 'Content-Type: application/json' \
    -d '{"mappings":{"properties":{"PRENOMS":{"type":"text"},"NOM":{"type":"text"}}}}' >/dev/null

  # bulk ingest
  t0=$(date +%s.%N)
  bulk_resp=$(curl -fsS -X POST "$URL/_bulk" \
    -H 'Content-Type: application/x-ndjson' \
    --data-binary "@$DATASET")
  t1=$(date +%s.%N)
  printf 'bulk\t-\tclient_ms=%s\ttook=%s\terrors=%s\n' \
    "$(dt_ms "$t0" "$t1")" "$(took "$bulk_resp")" "$(jq -r '.errors' <<<"$bulk_resp")"

  curl -fsS -X POST "$URL/$INDEX/_refresh" >/dev/null
  count_resp=$(curl -fsS "$URL/$INDEX/_count")
  printf 'count\t-\tcount=%s\n' "$(jq -r '.count' <<<"$count_resp")"

  # warmup
  head -3 "$NAMES" | while read -r name; do
    curl -fsS -X POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' \
      --data "{\"query\":{\"multi_match\":{\"query\":\"$name\",\"fields\":[\"NOM\",\"PRENOMS\"]}},\"size\":10,\"track_total_hits\":true}" >/dev/null
  done

  i=0
  while read -r name; do
    i=$((i+1))
    [ "$i" -gt "$ITER" ] && break
    t0=$(date +%s.%N)
    resp=$(curl -fsS -X POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' \
      --data "{\"query\":{\"multi_match\":{\"query\":\"$name\",\"fields\":[\"NOM\",\"PRENOMS\"]}},\"size\":10,\"track_total_hits\":true}")
    t1=$(date +%s.%N)
    printf 'multi_match\t%s\tclient_ms=%s\ttook=%s\thits=%s\ttop=%s\n' \
      "$name" "$(dt_ms "$t0" "$t1")" "$(took "$resp")" "$(hits_total "$resp")" "$(hits_top_id "$resp")"
  done < "$NAMES"

} >>"$OUT" 2>&1
