#!/usr/bin/env bash
# MVP L6.2 measurement: snapshot v2 (segment files) vs v1 (documents) on
# 200k synthetic deces-like docs, release binary, fs repository.
#
# v2: build under SURCH_DATA_DIR -> snapshot -> fresh data dir -> restore
#     with the matchID Job body -> time to first query.
# v1: build without data dir -> snapshot (logical) -> fresh data dir ->
#     same restore (re-ingest + commit) -> time to first query.
# Responses of 6 queries are compared source vs restored (took and the
# concrete index name normalised).
#
# Usage: ROOT=<scratch dir, wiped> l62-snapshot-200k.sh <surch-api> <mapping.json>
set -euo pipefail
BIN=${1:?usage: l62-snapshot-200k.sh <surch-api release binary> <mapping.json>}
MAPPING=${2:?usage: l62-snapshot-200k.sh <surch-api release binary> <mapping.json>}
ROOT=${ROOT:?set ROOT to a scratch directory (it is wiped)}
GEN=$ROOT/gen
PORT=17792
N=${N:-200000}
CHUNK=10000
TARGET=deces-snap-200k
rm -rf "$ROOT"
mkdir -p "$GEN"

awk -v n="$N" -v chunk="$CHUNK" -v out="$GEN" 'BEGIN {
  srand(42)
  np = split("MAR BER DU THO RO RI PE LE MO SI LAU FOUR GAR DA BERT VIN GI AN MER LAM BON FRAN COU DE CHA BLAN GUE FAU RE MAS", pre, " ")
  ns = split("TIN NARD BOIS MAS BERT CHARD TIT RAND ROY REAU MON RENT NIER CIA VID TRAND CENT RARD DRE CIER BERT NET COIS LON SSIN VAL NIEL RAUD CHET LIN", suf, " ")
  nf = split("MARIE JEAN PIERRE JEANNE LOUIS ANNE PAUL JOSEPH MARGUERITE GEORGES LOUISE HENRI MADELEINE ANDRE GERMAINE MARCEL YVONNE RENE SUZANNE ROGER MARCELLE ROBERT ODETTE FERNAND SIMONE LUCIEN DENISE MAURICE PAULETTE RAYMOND", fn, " ")
  nc = split("PARIS LYON MARSEILLE BREST LILLE NANTES ANGERS ROUEN DIJON NICE TOULOUSE BORDEAUX REIMS LEHAVRE METZ NANCY CAEN TOURS LIMOGES AMIENS", cm, " ")
  for (i = 1; i <= n; i++) {
    file = sprintf("%s/chunk-%03d.ndjson", out, int((i - 1) / chunk))
    nom = pre[1 + int(rand() * np)] suf[1 + int(rand() * ns)]
    if (rand() < 0.3) nom = nom " " pre[1 + int(rand() * np)] suf[1 + int(rand() * ns)]
    p = fn[1 + int(rand() * nf)] " " fn[1 + int(rand() * nf)]
    sexe = (rand() < 0.5) ? "M" : "F"
    yb = 1900 + int(rand() * 90); yd = yb + 1 + int(rand() * 30)
    db = sprintf("%04d%02d%02d", yb, 1 + int(rand() * 12), 1 + int(rand() * 28))
    dd = sprintf("%04d%02d%02d", yd, 1 + int(rand() * 12), 1 + int(rand() * 28))
    cn = cm[1 + int(rand() * nc)]; cd = cm[1 + int(rand() * nc)]
    printf "{\"index\":{\"_index\":\"deces\",\"_id\":\"d%07d\"}}\n", i > file
    printf "{\"NOM\":\"%s\",\"PRENOMS\":\"%s\",\"SEXE\":\"%s\",\"DATE_NAISSANCE\":\"%s\",\"COMMUNE_NAISSANCE\":\"%s\",\"CODE_INSEE_NAISSANCE\":\"%05d\",\"DATE_DECES\":\"%s\",\"COMMUNE_DECES\":\"%s\",\"SOURCE\":\"INSEE\",\"SOURCE_LINE\":%d}\n", nom, p, sexe, db, cn, 1000 + int(rand() * 95000), dd, cd, i > file
  }
}'

now_ms() { date +%s%3N; }
hwm_kib() { awk '/^VmHWM:/ {print $2}' "/proc/$1/status"; }
rss_kib() { awk '/^VmRSS:/ {print $2}' "/proc/$1/status"; }
url="http://127.0.0.1:$PORT"
json='content-type: application/json'

start() {
  env "$@" SURCH_PORT=$PORT SURCH_SOURCE_COMPRESS=1 SURCH_POSTINGS_DISK=1 \
    SURCH_FLUSH_BUDGET_BYTES=268435456 SURCH_MERGE_FANIN=8 SURCH_MERGE_MAX_DOCS=7000000 \
    SURCH_DENSIFY_BUDGET_DOCS=1000000 "$BIN" > "$ROOT/server-$(now_ms).log" 2>&1 &
  echo $!
}
wait_up() {
  until curl -sf "$url/" > /dev/null 2>&1; do kill -0 "$1" 2>/dev/null || { echo "server died" >&2; exit 1; }; sleep 0.02; done
}
stop() {
  kill -9 "$1"; while kill -0 "$1" 2>/dev/null; do sleep 0.02; done
  while curl -s "$url/" >/dev/null 2>&1; do sleep 0.02; done
}
build() {
  curl -sf -XPUT "$url/deces" -H "$json" --data-binary @"$MAPPING" > /dev/null
  for f in "$GEN"/chunk-*.ndjson; do
    curl -sf -XPOST "$url/_bulk" -H 'content-type: application/x-ndjson' --data-binary @"$f" \
      | grep -q '"errors":false' || { echo "bulk failed: $f" >&2; exit 1; }
  done
  curl -sf -XPOST "$url/deces/_refresh" > /dev/null
}
queries() {
  local index=$1 out=$2
  : > "$out"
  for body in \
    '{"query":{"match":{"NOM":"MARTIN"}},"size":10}' \
    '{"query":{"bool":{"must":[{"match":{"NOM":"BERNARD"}},{"match":{"PRENOMS":"MARIE"}}],"filter":[{"term":{"SEXE":"F"}}]}},"size":10}' \
    '{"query":{"fuzzy":{"NOM":{"value":"DUBIOS","fuzziness":"AUTO"}}},"size":10}' \
    '{"query":{"range":{"DATE_DECES":{"gte":"19500101","lt":"19600101"}}},"sort":[{"NOM.raw":"asc"}],"size":10}' \
    '{"size":0,"aggs":{"sexe":{"terms":{"field":"SEXE"}},"communes":{"terms":{"field":"COMMUNE_DECES","size":5}}}}' \
    '{"query":{"match":{"NOM.autocomplete":"lamb"}},"size":5}' ; do
    curl -sf -XPOST "$url/$index/_search" -H "$json" -d "$body" \
      | sed -E 's/"took":[0-9]+/"took":0/' | sed "s/$TARGET/deces/g" >> "$out"
    echo >> "$out"
  done
}
restore_body="{\"indices\":\"deces\",\"ignore_unavailable\":true,\"include_global_state\":false,\"rename_pattern\":\"^deces\$\",\"rename_replacement\":\"$TARGET\",\"index_settings\":{\"index.number_of_replicas\":0}}"

# phase <label> <source data dir or empty>
phase() {
  local label=$1 src_data=$2 repo=$ROOT/repo-$1 dst=$ROOT/dst-$1
  mkdir -p "$repo"
  local pid
  if [ -n "$src_data" ]; then pid=$(start SURCH_DATA_DIR="$src_data"); else pid=$(start); fi
  wait_up "$pid"
  local t0; t0=$(now_ms); build; local build_ms=$(( $(now_ms) - t0 ))
  queries deces "$ROOT/q-$label-source.txt"
  curl -sf -XPUT "$url/_snapshot/matchID" -H "$json" \
    -d "{\"type\":\"fs\",\"settings\":{\"location\":\"$repo\"}}" > /dev/null
  t0=$(now_ms)
  curl -sf -XPUT "$url/_snapshot/matchID/snap?wait_for_completion=true" -H "$json" \
    -d '{"indices":"deces","include_global_state":false}' > "$ROOT/snap-$label.json"
  local snap_ms=$(( $(now_ms) - t0 ))
  stop "$pid"
  local repo_bytes; repo_bytes=$(du -sb --apparent-size "$repo" | cut -f1)
  local repo_files; repo_files=$(find "$repo" -type f | wc -l)

  pid=$(start SURCH_DATA_DIR="$dst")
  wait_up "$pid"
  curl -sf -XPUT "$url/_snapshot/matchID" -H "$json" \
    -d "{\"type\":\"fs\",\"settings\":{\"location\":\"$repo\",\"readonly\":true}}" > /dev/null
  t0=$(now_ms)
  curl -sf -XPOST "$url/_snapshot/matchID/snap/_restore?wait_for_completion=true" -H "$json" \
    -d "$restore_body" > "$ROOT/restore-$label.json"
  local restore_ms=$(( $(now_ms) - t0 ))
  curl -sf -XPOST "$url/$TARGET/_search" -H "$json" -d '{"query":{"match":{"NOM":"MARTIN"}},"size":1}' > /dev/null
  local first_query_ms=$(( $(now_ms) - t0 ))
  local count; count=$(curl -sf "$url/$TARGET/_count" | sed -E 's/.*"count":([0-9]+).*/\1/')
  local hwm; hwm=$(hwm_kib "$pid"); local rss; rss=$(rss_kib "$pid")
  queries "$TARGET" "$ROOT/q-$label-restored.txt"
  stop "$pid"
  local identical=no
  cmp -s "$ROOT/q-$label-source.txt" "$ROOT/q-$label-restored.txt" && identical=yes
  local dst_bytes; dst_bytes=$(du -sb --apparent-size "$dst/indices" | cut -f1)
  echo "$label build_ms=$build_ms snapshot_ms=$snap_ms repo_bytes=$repo_bytes repo_files=$repo_files restore_ms=$restore_ms restore_to_first_query_ms=$first_query_ms restored_count=$count restored_dir_bytes=$dst_bytes restore_rss_kib=$rss restore_hwm_kib=$hwm responses_identical=$identical"
  echo "$label snapshot_response=$(cat "$ROOT/snap-$label.json")"
}

phase v2 "$ROOT/src-v2"
phase v1 ""
