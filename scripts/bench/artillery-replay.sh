#!/usr/bin/env bash
# Artillery-style replay of the matchID test-backend-v1.yml scenario shape
# against an ES-compatible engine (Surch or OpenSearch).
#
# Reproduces the 50/50 query mix on INSEE 25k:
#   - 50% "POST search" : { multi_match: query, fields: [PRENOMS, NOM] }
#   - 50% "POST search" : { bool: must [ match PRENOMS, match NOM ] }
# (Both shapes are functionally the same matchID search; we use the two
# variants to cover the WAND multi_match path and the bool must path.)
#
# Phases match test-backend-v1.yml: 30s@2 RPS, 30s@2, 30s@5, 30s@10,
# 30s@20, 60s@50.
#
# Usage: artillery-replay.sh <label> <out_file> [url] [names_csv]
#
# Paramétrage (1d, brainstorm-4-fronts-2026-07-09.md b1/P4) — tout par env, défauts inchangés :
#   INDEX/DATASET   : index cible / corpus à bulk-charger (défaut historique deces_25k).
#   SKIP_BULK=1     : ne PAS delete/create/bulk-charger — rejoue contre un index DÉJÀ INDEXÉ
#                     (ex. le "deces_bench" laissé par fair-ab.sh avant son teardown, via
#                     FAIRAB_CONTAINER + HOLD_SECONDS côté fair-ab.sh).
#   FAIRAB_CONTAINER: nom du conteneur fair-ab (ex. fairab-surch / fairab-es) — si défini, l'URL
#                     est résolue automatiquement (IP du conteneur sur FAIRAB_NETWORK, host ->
#                     conteneur direct, aucun port publié requis) ; l'argument positionnel <url>
#                     devient alors optionnel.
#   FAIRAB_NETWORK  : réseau docker fair-ab (défaut fair-ab-net).
#   FAIRAB_PORT     : port du moteur dans le conteneur (défaut 9200, cf fair-ab.sh SURCH_PORT).
#   NAMES_ORDER     : "prenoms_nom" (défaut historique, colonnes PRENOMS\tNOM\tDATE_NAISSANCE) ou
#                     "nom_prenoms" pour réutiliser TEL QUEL le probe_names.tsv de fair-ab.sh 1a
#                     (colonnes nom\tprenoms, ordre inverse).
#   FIELD_NOM/FIELD_PRENOMS : clés ES interrogées (défaut NOM/PRENOMS, schéma matchID réel).
set -u
export LC_ALL=C   # décimales en point, pas virgule (LC_NUMERIC locale casse awk/sleep sinon, cf fair-ab.sh)
LABEL="${1:?label}"
OUT="${2:?out}"
URL="${3:-}"
NAMES="${4:-/home/antoinefa/src/surch/target/insee/artillery_names.txt}"
INDEX="${INDEX:-deces_25k}"
DATASET="${DATASET:-/home/antoinefa/src/surch/target/insee/deces_25k.ndjson}"
SKIP_BULK="${SKIP_BULK:-0}"
FAIRAB_CONTAINER="${FAIRAB_CONTAINER:-}"
FAIRAB_NETWORK="${FAIRAB_NETWORK:-fair-ab-net}"
FAIRAB_PORT="${FAIRAB_PORT:-9200}"
NAMES_ORDER="${NAMES_ORDER:-prenoms_nom}"
FIELD_NOM="${FIELD_NOM:-NOM}"
FIELD_PRENOMS="${FIELD_PRENOMS:-PRENOMS}"
LAT_DIR=$(mktemp -d)
trap 'rm -rf "$LAT_DIR"' EXIT

dt_ms() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.1f\n", (b-a)*1000 }'; }

# Branchement sur le réseau docker de fair-ab (post-indexation) : un réseau docker
# user-defined est routable directement depuis l'hôte via l'IP du conteneur (pas de port
# publié nécessaire) -- pas besoin de conteneuriser ce script, juste résoudre l'IP.
if [ -n "$FAIRAB_CONTAINER" ]; then
  cip=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$FAIRAB_NETWORK\").IPAddress}}" "$FAIRAB_CONTAINER" 2>/dev/null)
  if [ -z "$cip" ]; then
    echo "artillery-replay: IP introuvable pour $FAIRAB_CONTAINER sur $FAIRAB_NETWORK (conteneur up ? réseau correct ?)" >&2
    exit 1
  fi
  URL="http://$cip:$FAIRAB_PORT"
  echo "artillery-replay: branché sur $FAIRAB_CONTAINER ($URL) via réseau $FAIRAB_NETWORK" >&2
fi
[ -n "$URL" ] || { echo "artillery-replay: url requis (3e argument, ou FAIRAB_CONTAINER)" >&2; exit 1; }

# Build the names sample once (schéma historique prenoms_nom uniquement — un NAMES fourni via
# 4e argument, ex. probe_names.tsv de fair-ab.sh 1a, est utilisé TEL QUEL sans régénération).
if [ ! -s "$NAMES" ]; then
  grep '"PRENOMS":' "$DATASET" \
    | jq -r 'select(.PRENOMS != "" and .NOM != "") | "\(.PRENOMS)\t\(.NOM)\t\(.DATE_NAISSANCE)"' \
    | shuf -n 5000 > "$NAMES"
fi
NAMES_COUNT=$(wc -l < "$NAMES" 2>/dev/null); NAMES_COUNT=${NAMES_COUNT:-0}
[ "$NAMES_COUNT" -gt 0 ] || { echo "artillery-replay: NAMES vide ($NAMES)" >&2; exit 1; }

if [ "$SKIP_BULK" = "1" ]; then
  echo "artillery-replay: SKIP_BULK=1 -- réutilise l'index existant $INDEX (pas de delete/create/bulk)" >&2
  bulk_ms=0; bulk_took=0
else
  # Prepare the index (idempotent fresh load).
  curl -fsS -X DELETE "$URL/$INDEX" >/dev/null 2>&1 || true
  curl -fsS -X PUT "$URL/$INDEX" -H 'Content-Type: application/json' \
    -d '{"mappings":{"properties":{"'"$FIELD_PRENOMS"'":{"type":"text"},"'"$FIELD_NOM"'":{"type":"text"},"DATE_NAISSANCE":{"type":"keyword"}}}}' >/dev/null
  t0=$(date +%s.%N)
  bulk_resp=$(curl -fsS -X POST "$URL/_bulk" -H 'Content-Type: application/x-ndjson' --data-binary "@$DATASET")
  t1=$(date +%s.%N)
  bulk_ms=$(dt_ms "$t0" "$t1")
  bulk_took=$(jq -r '.took // 0' <<<"$bulk_resp")
  curl -fsS -X POST "$URL/$INDEX/_refresh" >/dev/null
fi

# One request: pick a random line from $NAMES, build a body, time it.
single_request() {
  local idx="$1"
  local line
  line=$(awk -v n="$idx" 'NR==n' "$NAMES")
  local col1 col2 prenoms nom
  col1=$(awk -F'\t' '{print $1}' <<<"$line")
  col2=$(awk -F'\t' '{print $2}' <<<"$line")
  if [ "$NAMES_ORDER" = "nom_prenoms" ]; then
    nom="$col1"; prenoms="$col2"
  else
    prenoms="$col1"; nom="$col2"
  fi
  local body
  if [ $((idx % 2)) -eq 0 ]; then
    body=$(printf '{"query":{"multi_match":{"query":"%s %s","fields":["%s","%s"]}},"size":10,"track_total_hits":true}' "$prenoms" "$nom" "$FIELD_PRENOMS" "$FIELD_NOM")
  else
    body=$(printf '{"query":{"bool":{"must":[{"match":{"%s":"%s"}},{"match":{"%s":"%s"}}]}},"size":10,"track_total_hits":true}' "$FIELD_PRENOMS" "$prenoms" "$FIELD_NOM" "$nom")
  fi
  local t0 t1
  t0=$(date +%s.%N)
  curl -fsS -X POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' --data "$body" >/dev/null
  t1=$(date +%s.%N)
  dt_ms "$t0" "$t1"
}
export -f single_request dt_ms
export URL INDEX NAMES NAMES_ORDER FIELD_NOM FIELD_PRENOMS

# Drive a phase at <rate> RPS for <duration> seconds.
phase() {
  local rate=$1
  local duration=$2
  local label=$3
  local interval
  interval=$(awk -v r="$rate" 'BEGIN { printf "%.4f", 1/r }')
  local lat_file="$LAT_DIR/$label.txt"
  local total=$((rate * duration))
  local start=$(date +%s.%N)
  for i in $(seq 1 "$total"); do
    local idx=$(( (RANDOM % NAMES_COUNT) + 1 ))
    local target=$(awk -v s="$start" -v iv="$interval" -v i="$i" 'BEGIN { printf "%.4f", s + iv*(i-1) }')
    local now=$(date +%s.%N)
    local wait=$(awk -v t="$target" -v n="$now" 'BEGIN { printf "%.4f", (t>n)?(t-n):0 }')
    awk -v w="$wait" 'BEGIN { if (w>0) system("sleep "w) }'
    # Run async, capture latency
    ( single_request "$idx" >> "$lat_file" ) &
  done
  wait
}

{
  echo "## artillery-replay label=$LABEL  $(date -Iseconds)"
  echo "url=$URL bulk_client_ms=$bulk_ms bulk_took=$bulk_took"

  : "${ARTILLERY_SCALE:=1}"
  d30=$(awk -v s="$ARTILLERY_SCALE" 'BEGIN { printf "%d", 30*s }')
  d60=$(awk -v s="$ARTILLERY_SCALE" 'BEGIN { printf "%d", 60*s }')
  [ "$d30" -lt 1 ] && d30=1
  [ "$d60" -lt 1 ] && d60=1
  phase 2  "$d30" phase1
  phase 2  "$d30" phase2
  phase 5  "$d30" phase3
  phase 10 "$d30" phase4
  phase 20 "$d30" phase5
  phase 50 "$d60" phase6

  for p in phase1 phase2 phase3 phase4 phase5 phase6; do
    if [ -s "$LAT_DIR/$p.txt" ]; then
      stats=$(sort -n "$LAT_DIR/$p.txt" | awk 'BEGIN{c=0} {a[c++]=$1} END { if(c>0) printf "n=%d p50=%.1f p95=%.1f p99=%.1f max=%.1f\n", c, a[int(c*0.5)], a[int(c*0.95)], a[int(c*0.99)], a[c-1] }')
      echo "$p: $stats"
    fi
  done
} >>"$OUT" 2>&1
