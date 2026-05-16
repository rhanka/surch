#!/usr/bin/env bash
# fetch-insee-slice.sh — build the real INSEE 10k matchID-shaped slice
# committed under tests/matchid_compat/deces/slice-10000.ndjson.gz.
#
# This is the B2 v1 fetcher (real INSEE extract). The synthetic AWK
# generator (tools/gen_deces_slice.awk) is kept as a dev-mode fallback
# and still drives the B1 replay (deces_v1.json) — the synthetic slice
# is byte-stable and the replay expectations are pinned to it.
#
# Source: INSEE Open Licence 2.0 (Etalab). The CSV is the
# "fichier des personnes décédées" published yearly at
#   https://www.insee.fr/fr/information/4769950
# The script does NOT auto-download (INSEE URLs rotate); it expects
# the CSV to be available locally and falls back to the workspace
# cache (target/insee/Deces_2024.csv) populated by scripts/bench.
#
# Output shape (matches tests/matchid_compat/deces/mapping.json):
#   {NOM, PRENOMS, SEXE(M/F), DATE_NAISSANCE, COMMUNE_NAISSANCE,
#    CODE_INSEE_NAISSANCE, DATE_DECES, COMMUNE_DECES, SOURCE,
#    SOURCE_LINE}
# _id is `ins_NNNNNNN` (1-based row index, zero-padded to 7 digits)
# to keep the slice immune to the synthetic `deces_NNNNN` namespace
# used by the v0 replay manifest.
#
# Usage:
#   tools/fetch-insee-slice.sh \
#       [INSEE_CSV=target/insee/Deces_2024.csv] \
#       [OUT=tests/matchid_compat/deces/slice-10000.ndjson.gz] \
#       [LIMIT=10000]
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSEE_CSV="${INSEE_CSV:-$REPO_ROOT/target/insee/Deces_2024.csv}"
OUT="${OUT:-$REPO_ROOT/tests/matchid_compat/deces/slice-10000.ndjson.gz}"
LIMIT="${LIMIT:-10000}"

if [[ ! -f "$INSEE_CSV" ]]; then
  echo "fetch-insee-slice: INSEE CSV not found at $INSEE_CSV" >&2
  echo "fetch-insee-slice: download from https://www.insee.fr/fr/information/4769950" >&2
  echo "fetch-insee-slice: (Deces_YYYY.zip → unzip → Deces_YYYY.csv) and re-run." >&2
  exit 1
fi

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

awk -v LIMIT="$LIMIT" '
function js_esc(s) {
    gsub(/\\/, "\\\\", s)
    gsub(/"/, "\\\"", s)
    return s
}
BEGIN {
    FS = ";"
    count = 0
}
NR == 1 { next }  # CSV header
{
    for (i = 1; i <= NF; i++) {
        gsub(/\r/, "", $i)
        gsub(/^"|"$/, "", $i)
    }
    nomprenom = $1
    sexe_raw  = $2
    datenaiss = $3
    lieunaiss = $4
    commnaiss = $5
    datedeces = $7
    lieudeces = $8

    # nomprenom = "NOM*PRENOM1 PRENOM2/"
    star_pos = index(nomprenom, "*")
    if (star_pos > 0) {
        nom = substr(nomprenom, 1, star_pos - 1)
        rest = substr(nomprenom, star_pos + 1)
        sub(/\/$/, "", rest)
        prenoms = rest
    } else {
        nom = nomprenom
        prenoms = ""
    }

    # INSEE sexe: 1=M, 2=F. matchID shape expects M/F.
    if (sexe_raw == "1") sexe = "M"
    else if (sexe_raw == "2") sexe = "F"
    else sexe = sexe_raw

    count++
    id = sprintf("ins_%07d", count)

    # action line
    printf "{\"index\":{\"_id\":\"%s\",\"_index\":\"deces\"}}\n", id
    # source line — strict matchID shape: COMMUNE_DECES holds the INSEE
    # commune code (no public commune-name lookup table is bundled; see
    # README for the limitation and the future COG join plan).
    printf "{\"NOM\":\"%s\",\"PRENOMS\":\"%s\",\"SEXE\":\"%s\",\"DATE_NAISSANCE\":\"%s\",\"COMMUNE_NAISSANCE\":\"%s\",\"CODE_INSEE_NAISSANCE\":\"%s\",\"DATE_DECES\":\"%s\",\"COMMUNE_DECES\":\"%s\",\"SOURCE\":\"INSEE\",\"SOURCE_LINE\":%d}\n", \
        js_esc(nom), js_esc(prenoms), sexe, datenaiss, js_esc(commnaiss), lieunaiss, datedeces, lieudeces, count

    if (LIMIT > 0 && count >= LIMIT) exit
}
' "$INSEE_CSV" > "$TMP"

gzip -9 -n -c "$TMP" > "$OUT"
docs=$(( $(wc -l < "$TMP") / 2 ))
bytes=$(wc -c < "$OUT")
sha=$(sha256sum "$OUT" | cut -d' ' -f1)
echo "wrote $docs documents to $OUT"
echo "  size: $bytes bytes (gzip -9)"
echo "  sha256: $sha"
