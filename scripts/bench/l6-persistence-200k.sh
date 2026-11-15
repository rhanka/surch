#!/usr/bin/env bash
# MVP L6 measurement: 200k synthetic deces-like docs, release binary.
# Phase A: in-memory (no SURCH_DATA_DIR) build -> reference build time / RSS.
# Phase B: SURCH_DATA_DIR build -> persist size; kill -9; restart -> time to
#          first query, RSS after reopen, byte-identical responses.
set -euo pipefail
BIN=${1:?usage: l6-persistence-200k.sh <surch-api release binary> <mapping.json>}
MAPPING=${2:?usage: l6-persistence-200k.sh <surch-api release binary> <mapping.json>}
ROOT=${ROOT:?set ROOT to a scratch directory (it is wiped)}
GEN=$ROOT/gen
DATA=$ROOT/data
PORT=17791
N=${N:-200000}
CHUNK=10000
rm -rf "$ROOT"
mkdir -p "$GEN" "$DATA"

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
rss_kib() { awk '/^VmRSS:/ {print $2}' "/proc/$1/status"; }
hwm_kib() { awk '/^VmHWM:/ {print $2}' "/proc/$1/status"; }
url="http://127.0.0.1:$PORT"

start() {
  env "$@" SURCH_PORT=$PORT SURCH_SOURCE_COMPRESS=1 SURCH_POSTINGS_DISK=1 \
    SURCH_FLUSH_BUDGET_BYTES=268435456 SURCH_MERGE_FANIN=8 SURCH_MERGE_MAX_DOCS=7000000 \
    SURCH_DENSIFY_BUDGET_DOCS=1000000 "$BIN" > "$ROOT/server-$(now_ms).log" 2>&1 &
  echo $!
}

wait_query() {
  # Poll until the first real query answers; prints elapsed ms since $1.
  local t0=$1
  until curl -sf -XPOST "$url/deces/_search" -H 'content-type: application/json' \
      -d '{"query":{"match":{"NOM":"MARTIN"}},"size":1}' > /dev/null 2>&1; do
    kill -0 "$2" 2>/dev/null || { echo "server died on reopen" >&2; exit 1; }; sleep 0.02
  done
  echo $(( $(now_ms) - t0 ))
}

wait_up() {
  # $1 = server pid: fail fast if it died (port in use, open error).
  until curl -sf "$url/" > /dev/null 2>&1; do kill -0 "$1" 2>/dev/null || { echo "server died" >&2; exit 1; }; sleep 0.02; done
}

build() {
  curl -sf -XPUT "$url/deces" -H 'content-type: application/json' --data-binary @"$MAPPING" > /dev/null
  local t0; t0=$(now_ms)
  for f in "$GEN"/chunk-*.ndjson; do
    curl -sf -XPOST "$url/_bulk" -H 'content-type: application/x-ndjson' --data-binary @"$f" \
      | grep -q '"errors":false' || { echo "bulk failed: $f" >&2; exit 1; }
  done
  curl -sf -XPOST "$url/deces/_refresh" > /dev/null
  echo $(( $(now_ms) - t0 ))
}

queries() {
  local out=$1
  : > "$out"
  for body in \
    '{"query":{"match":{"NOM":"MARTIN"}},"size":10}' \
    '{"query":{"bool":{"must":[{"match":{"NOM":"BERNARD"}},{"match":{"PRENOMS":"MARIE"}}],"filter":[{"term":{"SEXE":"F"}}]}},"size":10}' \
    '{"query":{"fuzzy":{"NOM":{"value":"DUBIOS","fuzziness":"AUTO"}}},"size":10}' \
    '{"query":{"range":{"DATE_DECES":{"gte":"19500101","lt":"19600101"}}},"sort":[{"NOM.raw":"asc"}],"size":10}' \
    '{"size":0,"aggs":{"sexe":{"terms":{"field":"SEXE"}},"communes":{"terms":{"field":"COMMUNE_DECES","size":5}}}}' \
    '{"query":{"match":{"NOM.autocomplete":"lamb"}},"size":5}' ; do
    curl -sf -XPOST "$url/deces/_search" -H 'content-type: application/json' -d "$body" \
      | sed -E 's/"took":[0-9]+/"took":0/' >> "$out"
    echo >> "$out"
  done
}

# Phase A — in-memory reference.
pid=$(start)
wait_up "$pid"
build_mem=$(build)
rss_mem=$(rss_kib "$pid"); hwm_mem=$(hwm_kib "$pid")
queries "$ROOT/q-mem.txt"
kill -9 "$pid"; while kill -0 "$pid" 2>/dev/null; do sleep 0.02; done; while curl -s "$url/" >/dev/null 2>&1; do sleep 0.02; done

# Phase B — persisted build.
pid=$(start SURCH_DATA_DIR="$DATA")
wait_up "$pid"
build_disk=$(build)
rss_build=$(rss_kib "$pid"); hwm_build=$(hwm_kib "$pid")
queries "$ROOT/q-before.txt"
rss_build_q=$(rss_kib "$pid")
kill -9 "$pid"; while kill -0 "$pid" 2>/dev/null; do sleep 0.02; done; while curl -s "$url/" >/dev/null 2>&1; do sleep 0.02; done
size_apparent=$(du -sb --apparent-size "$DATA/indices" | cut -f1)
size_alloc=$(du -sk "$DATA/indices" | cut -f1)
files=$(find "$DATA/indices" -type f | wc -l)

# Restart (kill -9 above = pod killed without grace).
t0=$(now_ms)
pid=$(start SURCH_DATA_DIR="$DATA")
first_query_ms=$(wait_query "$t0" "$pid")
rss_reopen=$(rss_kib "$pid"); hwm_reopen=$(hwm_kib "$pid")
queries "$ROOT/q-after.txt"
rss_reopen_q=$(rss_kib "$pid")
kill -9 "$pid"; while kill -0 "$pid" 2>/dev/null; do sleep 0.02; done; while curl -s "$url/" >/dev/null 2>&1; do sleep 0.02; done

identical=no
cmp -s "$ROOT/q-before.txt" "$ROOT/q-after.txt" && identical=yes
identical_mem=no
cmp -s "$ROOT/q-mem.txt" "$ROOT/q-after.txt" && identical_mem=yes

cat <<EOF
docs=$N chunks=$(ls "$GEN" | wc -l)
build_ms_in_memory=$build_mem
build_ms_persisted=$build_disk
persist_bytes_apparent=$size_apparent
persist_kib_allocated=$size_alloc
persist_files=$files
restart_to_first_query_ms=$first_query_ms
rss_kib_after_build_in_memory=$rss_mem (hwm $hwm_mem)
rss_kib_after_build_persisted=$rss_build (hwm $hwm_build), after queries $rss_build_q
rss_kib_after_reopen=$rss_reopen (hwm $hwm_reopen), after queries $rss_reopen_q
responses_identical_before_after_restart=$identical
responses_identical_in_memory_vs_reopened=$identical_mem
EOF
ls -la "$DATA/indices"/i-deces | head -40
grep -h "data directory" "$ROOT"/server-*.log || true
