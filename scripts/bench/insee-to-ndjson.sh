#!/usr/bin/env bash
# Convert INSEE Deces CSV → NDJSON matchID-shaped for ingestion.
# Usage: insee-to-ndjson.sh <input.csv> <output.ndjson> <index_name> [limit]
set -euo pipefail
INPUT="${1:?input csv}"
OUTPUT="${2:?output ndjson}"
INDEX="${3:?index name}"
LIMIT="${4:-0}"

# INSEE CSV columns (semicolon-separated, quoted):
#   nomprenom;sexe;datenaiss;lieunaiss;commnaiss;paysnaiss;datedeces;lieudeces;actedeces
# nomprenom format: "NOM*PRENOM1 PRENOM2/"

awk -v LIMIT="$LIMIT" -v INDEX="$INDEX" '
function js_esc(s) {
    gsub(/\\/, "\\\\", s)
    gsub(/"/, "\\\"", s)
    return s
}
BEGIN {
    FS = ";"
    OFS = ""
    count = 0
}
NR == 1 { next }  # header
{
    # Strip CRLF + leading/trailing quotes
    for (i = 1; i <= NF; i++) {
        gsub(/\r/, "", $i)
        gsub(/^"|"$/, "", $i)
    }
    nomprenom = $1
    sexe = $2
    datenaiss = $3
    lieunaiss = $4
    commnaiss = $5
    paysnaiss = $6
    datedeces = $7
    lieudeces = $8
    actedeces = $9

    # Parse nomprenom "NOM*PRENOM1 PRENOM2/"
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

    id = "ins_" datedeces "_" actedeces "_" lieudeces "_" NR

    # action line
    print "{\"index\":{\"_id\":\"" id "\",\"_index\":\"" INDEX "\"}}"

    # source line
    printf "{"
    printf "\"id\":\"%s\",", js_esc(id)
    printf "\"PRENOMS\":\"%s\",", js_esc(prenoms)
    printf "\"NOM\":\"%s\",", js_esc(nom)
    printf "\"SEXE\":\"%s\",", js_esc(sexe)
    printf "\"DATE_NAISSANCE\":\"%s\",", js_esc(datenaiss)
    printf "\"CODE_INSEE_NAISSANCE\":\"%s\",", js_esc(lieunaiss)
    printf "\"COMMUNE_NAISSANCE\":\"%s\",", js_esc(commnaiss)
    printf "\"PAYS_NAISSANCE\":\"%s\",", js_esc(paysnaiss)
    printf "\"DATE_DECES\":\"%s\",", js_esc(datedeces)
    printf "\"CODE_INSEE_DECES\":\"%s\",", js_esc(lieudeces)
    printf "\"NUM_DECES\":\"%s\",", js_esc(actedeces)
    printf "\"SOURCE\":\"INSEE_%s\"", substr(datedeces, 1, 4)
    print "}"

    count++
    if (LIMIT > 0 && count >= LIMIT) exit
}' "$INPUT" > "$OUTPUT"

lines=$(wc -l < "$OUTPUT")
docs=$((lines / 2))
echo "wrote $docs documents to $OUTPUT"
