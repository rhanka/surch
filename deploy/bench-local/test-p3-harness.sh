#!/usr/bin/env bash
# Régressions unitaires sans Docker ni charge pour les garde-fous P3.
set -euo pipefail
export LC_ALL=C

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
HARNESS="$ROOT_DIR/deploy/bench-local/fair-ab.sh"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/surch-p3-harness.XXXXXX")
trap 'rm -rf -- "$TMP_DIR"' EXIT

fail(){ printf '[test-p3-harness] %s\n' "$*" >&2; exit 1; }

# Charge seulement les helpers purs : sourcer fair-ab.sh exécuterait un run.
awk '
  /^p2_validate_pairs\(\)\{/ { capture = 1 }
  /^p2_validate_body_files\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/term-helpers.sh"
awk '
  /^p2_metric_present\(\)\{/ { capture = 1 }
  /^p2_proc_status_bytes\(\)\{/ { capture = 0 }
  capture { print }
' "$HARNESS" > "$TMP_DIR/metric-helpers.sh"
awk '
  /^p2_cgroup_io_json\(\)\{/ { capture = 1 }
  capture { print }
  capture && /^}$/ { exit }
' "$HARNESS" > "$TMP_DIR/io-helpers.sh"

# B1 : la forme cgroup v2 minimale doit être un JSON consommable par jq.
printf '%s\n' '8:0 rbytes=1 wbytes=2 rios=3 wios=4' > "$TMP_DIR/io.stat"
source "$TMP_DIR/io-helpers.sh"
p2_cgroup_io_json "$TMP_DIR/io.stat" > "$TMP_DIR/io.json"
jq -e 'type == "array" and length == 4 and .[0] == {device:"8:0",metric:"rbytes",value:1}' "$TMP_DIR/io.json" >/dev/null \
  || fail 'B1: io.stat n est pas sérialisé en JSON valide attendu'

# B2 : Dupont et DUPONT désignent le même posting ASCII après lowercase ; le
# validateur indépendant doit refuser leur placement dans deux ensembles.
source "$TMP_DIR/term-helpers.sh"
P2_PAIR_COUNT=1
P2_WARM_TERM_COUNT=1
PROBE_FIXED_TERM=MARTIN
printf '%s\n' $'1\tDupont\tJean' > "$TMP_DIR/bool.tsv"
printf '%s\n' $'1\tDUPONT' > "$TMP_DIR/control.tsv"
printf '%s\n' $'1\tDurand\tJules' > "$TMP_DIR/warm.tsv"
if p2_validate_term_sets "$TMP_DIR/bool.tsv" "$TMP_DIR/control.tsv" "$TMP_DIR/warm.tsv"; then
  fail 'B2: collision analysée Dupont/DUPONT acceptée'
fi
printf '%s\n' $'1\tMartin\tJean' > "$TMP_DIR/fixed.tsv"
if p2_validate_pairs "$TMP_DIR/fixed.tsv" 1; then
  fail 'B2: le terme fixe Martin n est pas exclu après lowercase'
fi

# M4 : une jauge non finie n est jamais convertie silencieusement en zéro.
source "$TMP_DIR/metric-helpers.sh"
printf '%s\n' 'surch_jemalloc_resident_bytes NaN' > "$TMP_DIR/prometheus.bad"
if p2_metric_value surch_jemalloc_resident_bytes "$TMP_DIR/prometheus.bad" >/dev/null; then
  fail 'M4: NaN Prometheus a été accepté'
fi
printf '%s\n' 'surch_jemalloc_resident_bytes 0' > "$TMP_DIR/prometheus.zero"
[ "$(p2_metric_value surch_jemalloc_resident_bytes "$TMP_DIR/prometheus.zero")" = 0 ] \
  || fail 'M4: la valeur numérique zéro n est pas préservée'

printf 'test-p3-harness: PASS\n'
