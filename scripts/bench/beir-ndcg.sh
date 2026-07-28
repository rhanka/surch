#!/usr/bin/env bash
# Generic BEIR NDCG@10 + Recall@10 parity test against a BM25-only engine.
# Usage: beir-ndcg.sh <dataset> <label> <out_file> <url>
#
# Generalises trec-covid-ndcg.sh to any BEIR dataset (uniform layout:
# corpus.jsonl {_id,title,text}, queries.jsonl {_id,text}, qrels/test.tsv).
# Indexes the corpus, runs every test query (qids with a positive qrel),
# computes NDCG@10 (graded gain 2^rel-1) + Recall@10, averages, appends to
# <out_file>. Works for graded (nfcorpus, trec-covid) and binary (fiqa,
# scifact) qrels alike.
#
# In K8s the corpus is pre-hydrated read-only on $BEIR_DIR by 00-init-corpora
# / 00b-init-beir-extra.  Set BEIR_REQUIRE_LOCAL_DATA=1 for a quality gate:
# a missing or partial corpus then fails instead of falling back to a download.
set -euo pipefail
DATASET="${1:?dataset (e.g. nfcorpus, fiqa, trec-covid, scifact)}"
LABEL="${2:?label}"
OUT="${3:?out}"
URL="${4:?url}"
INDEX="$DATASET"
BEIR_ROOT="${BEIR_DIR:-/home/antoinefa/src/surch/target/beir}"
DATA="$BEIR_ROOT/$DATASET"
BEIR_BASE="${BEIR_BASE:-https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets}"
ARCHIVE_URL="$BEIR_BASE/$DATASET.zip"
ARCHIVE="$BEIR_ROOT/$DATASET.zip"
BEIR_BULK_CHUNK_SIZE="${BEIR_BULK_CHUNK_SIZE:-8m}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

SCRIPT_NAME="${0##*/}"

http_request() {
  local label="${1:?label}"
  local method="${2:?method}"
  local url="${3:?url}"
  shift 3
  local response_file
  response_file=$(mktemp "$TMP/http-response.XXXXXX")
  local status
  local curl_rc=0
  status=$(curl -sS -o "$response_file" -w '%{http_code}' -X "$method" "$@" "$url") || curl_rc=$?
  if [ "$curl_rc" -ne 0 ]; then
    echo "[$SCRIPT_NAME] curl failed during $label: $method $url (exit $curl_rc)" >&2
    [ -s "$response_file" ] && sed -n '1,40p' "$response_file" >&2
    return "$curl_rc"
  fi
  if [ "$status" -lt 200 ] || [ "$status" -ge 300 ]; then
    echo "[$SCRIPT_NAME] HTTP $status during $label: $method $url" >&2
    [ -s "$response_file" ] && sed -n '1,40p' "$response_file" >&2
    return 22
  fi
  cat "$response_file"
}

require_expected_count() {
  local label="${1:?label}"
  local actual="${2:?actual}"
  local expected="${3:-}"
  if [ -z "$expected" ]; then
    return 0
  fi
  case "$expected" in
    *[!0-9]*|'')
      echo "[$SCRIPT_NAME] invalid expected $label count: $expected" >&2
      exit 1
      ;;
  esac
  if [ "$actual" -ne "$expected" ]; then
    echo "[$SCRIPT_NAME] $label count=$actual, expected=$expected" >&2
    exit 1
  fi
}

# -- Step 1: ensure dataset present (auto-download for local dev only) --
if [ ! -s "$DATA/corpus.jsonl" ] || [ ! -s "$DATA/queries.jsonl" ] || [ ! -s "$DATA/qrels/test.tsv" ]; then
  if [ "${BEIR_REQUIRE_LOCAL_DATA:-0}" = "1" ]; then
    echo "[$SCRIPT_NAME] required BEIR dataset is incomplete: $DATA" >&2
    echo "[$SCRIPT_NAME] expected non-empty corpus.jsonl, queries.jsonl and qrels/test.tsv" >&2
    exit 1
  fi
  mkdir -p "$BEIR_ROOT"
  if [ ! -s "$ARCHIVE" ]; then
    echo "[beir:$DATASET] downloading $ARCHIVE_URL ..." >&2
    curl -fSL --retry 3 -o "$ARCHIVE" "$ARCHIVE_URL"
  fi
  echo "[beir:$DATASET] extracting to $BEIR_ROOT ..." >&2
  ( cd "$BEIR_ROOT" && unzip -oq "$ARCHIVE" )
fi

corpus_docs=$(awk 'NF { count++ } END { print count + 0 }' "$DATA/corpus.jsonl")
if [ "$corpus_docs" -eq 0 ]; then
  echo "[$SCRIPT_NAME] $DATA/corpus.jsonl has no document" >&2
  exit 1
fi
require_expected_count "corpus document" "$corpus_docs" "${BEIR_EXPECTED_DOCS:-}"

# -- Step 2: convert corpus.jsonl -> bulk NDJSON ---------------------------
if [ -s "$DATA/corpus.ndjson" ]; then
  NDJSON="$DATA/corpus.ndjson"
else
  NDJSON="$TMP/corpus.ndjson"
  jq -rc --arg idx "$INDEX" '[
    {"index":{"_id":._id,"_index":$idx}},
    {"id":._id,"title":.title,"text":.text}
  ] | .[] | tostring' "$DATA/corpus.jsonl" > "$NDJSON"
fi

# -- Step 3: (re)create index ----------------------------------------------
curl -fsS -X DELETE "$URL/$INDEX" >/dev/null 2>&1 || true
http_request "create index $INDEX" PUT "$URL/$INDEX" -H 'Content-Type: application/json' \
  -d '{"mappings":{"properties":{"title":{"type":"text"},"text":{"type":"text"}}}}' >/dev/null

parse_size_to_bytes() {
  local raw="${1:?size}"
  case "$raw" in
    *[kK])  printf '%s' "$(( ${raw%[kK]} * 1024 ))" ;;
    *[mM])  printf '%s' "$(( ${raw%[mM]} * 1024 * 1024 ))" ;;
    *[gG])  printf '%s' "$(( ${raw%[gG]} * 1024 * 1024 * 1024 ))" ;;
    *[0-9]) printf '%s' "$raw" ;;
    *) echo "[$SCRIPT_NAME] invalid BEIR_BULK_CHUNK_SIZE='$raw'" >&2; return 2 ;;
  esac
}
t0=$(date +%s.%N)
chunk_max_bytes=$(parse_size_to_bytes "$BEIR_BULK_CHUNK_SIZE")
awk -v out="$TMP/bulk" -v maxb="$chunk_max_bytes" '
  BEGIN { i = 0; sz = 0; cf = sprintf("%s.%04d", out, i); action = "" }
  {
    if (NR % 2 == 1) { action = $0; next }
    pair = length(action) + 1 + length($0) + 1
    if (sz + pair > maxb && sz > 0) { close(cf); i++; cf = sprintf("%s.%04d", out, i); sz = 0 }
    print action > cf
    print $0 > cf
    sz += pair
  }
  END {
    if (NR % 2 == 1) { printf "[%s] odd line count (NR=%d) — unpaired action\n", "'"$SCRIPT_NAME"'", NR > "/dev/stderr"; exit 2 }
    if (cf != "") close(cf)
  }
' "$NDJSON"
for chunk in "$TMP"/bulk.*; do
  chunk_lines=$(wc -l < "$chunk")
  if [ "$(( chunk_lines % 2 ))" -ne 0 ]; then
    echo "[$SCRIPT_NAME] $(basename "$chunk") has $chunk_lines lines (odd) — refusing to POST" >&2
    exit 22
  fi
  bulk_response=$(http_request "bulk ingest $INDEX chunk $(basename "$chunk")" POST "$URL/_bulk" -H 'Content-Type: application/x-ndjson' \
    --data-binary "@$chunk")
  if ! jq -e '.errors == false' >/dev/null <<<"$bulk_response"; then
    echo "[$SCRIPT_NAME] bulk ingest $INDEX chunk $(basename "$chunk") reported item errors" >&2
    exit 1
  fi
done
t1=$(date +%s.%N)
bulk_ms=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", (b-a)*1000 }')
http_request "refresh $INDEX" POST "$URL/$INDEX/_refresh" >/dev/null
count_response=$(http_request "count $INDEX" GET "$URL/$INDEX/_count")
indexed_docs=$(jq -er '.count | if type == "number" and . >= 0 and floor == . then . else error("invalid count") end' <<<"$count_response") || {
  echo "[$SCRIPT_NAME] malformed count response for $INDEX" >&2
  exit 1
}
if [ "$indexed_docs" -ne "$corpus_docs" ]; then
  echo "[$SCRIPT_NAME] indexed $indexed_docs documents in $INDEX, expected $corpus_docs" >&2
  exit 1
fi

# -- Step 4: qrels + queries -----------------------------------------------
awk -F'\t' 'NR>1 && $3>0 { print $1 }' "$DATA/qrels/test.tsv" | sort -u > "$TMP/test_qids.txt"
total_queries=$(wc -l < "$TMP/test_qids.txt")
if [ "$total_queries" -eq 0 ]; then
  echo "[$SCRIPT_NAME] $DATA/qrels/test.tsv has no positive test qrel" >&2
  exit 1
fi
require_expected_count "positive-qrel test query" "$total_queries" "${BEIR_EXPECTED_TEST_QIDS:-}"
awk -F'\t' 'NR>1 && $3>0 { print $1"\t"$2"\t"$3 }' "$DATA/qrels/test.tsv" | sort -k1,1 > "$TMP/qrels.tsv"
jq -r '"\(._id)\t\(.text)"' "$DATA/queries.jsonl" > "$TMP/queries.tsv"

cum_ndcg=0
cum_recall=0
processed=0
while read -r qid; do
  qtext=$(awk -F'\t' -v q="$qid" '$1==q{print $2; exit}' "$TMP/queries.tsv")
  if [ -z "$qtext" ]; then
    echo "[$SCRIPT_NAME] missing or empty query text for positive-qrel qid=$qid in $DATA" >&2
    exit 1
  fi
  qjson=$(printf '%s' "$qtext" | jq -Rsa . | sed 's/^"//;s/"$//')
  body=$(printf '{"query":{"multi_match":{"query":"%s","fields":["title","text"]}},"size":10,"track_total_hits":true}' "$qjson")
  resp=$(http_request "search $INDEX qid=$qid" POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' --data "$body")
  if ! jq -e '.hits.hits | type == "array"' >/dev/null <<<"$resp"; then
    echo "[$SCRIPT_NAME] malformed search response for qid=$qid" >&2
    exit 1
  fi
  top10=$(jq -r '.hits.hits[]._id' <<<"$resp")
  if [ -z "$top10" ]; then
    processed=$((processed+1))
    continue
  fi
  ndcg=$(awk -v qid="$qid" -v rels_file="$TMP/qrels.tsv" -v top="$top10" '
  function pow2m1(r) { return (2 ^ r) - 1 }
  BEGIN {
    n_rel = 0
    while ((getline line < rels_file) > 0) {
      split(line, f, "\t")
      if (f[1] == qid) { rel[f[2]] = f[3] + 0; rels[n_rel++] = f[3] + 0 }
    }
    close(rels_file)
    for (i = 1; i < n_rel; i++) { v = rels[i]; j = i - 1; while (j >= 0 && rels[j] < v) { rels[j+1] = rels[j]; j-- } rels[j+1] = v }
    cap = (n_rel < 10 ? n_rel : 10)
    idcg = 0
    for (i = 0; i < cap; i++) idcg += pow2m1(rels[i]) / (log(i+2) / log(2))
    if (idcg == 0) { print "0.0"; exit }
    dcg = 0; rank = 0
    split(top, hits, "\n")
    n_hits = 0
    for (k in hits) if (hits[k] != "") n_hits++
    for (k = 1; k <= n_hits; k++) {
      if (hits[k] == "") continue
      rank++
      if (rank > 10) break
      if (hits[k] in rel) dcg += pow2m1(rel[hits[k]]) / (log(rank+1) / log(2))
    }
    printf "%.6f\n", dcg/idcg
  }')
  recall=$(awk -v qid="$qid" -v rels_file="$TMP/qrels.tsv" -v top="$top10" '
  BEGIN {
    n_rel = 0
    while ((getline line < rels_file) > 0) {
      split(line, f, "\t")
      if (f[1] == qid) { rel[f[2]] = 1; n_rel++ }
    }
    close(rels_file)
    if (n_rel == 0) { print "0.0"; exit }
    n = split(top, hits, "\n")
    hit = 0; rank = 0
    for (k = 1; k <= n; k++) {
      if (hits[k] == "") continue
      rank++
      if (rank > 10) break
      if (hits[k] in rel) hit++
    }
    printf "%.6f\n", hit / n_rel
  }')
  cum_ndcg=$(awk -v a="$cum_ndcg" -v b="$ndcg" 'BEGIN { printf "%.6f", a+b }')
  cum_recall=$(awk -v a="$cum_recall" -v b="$recall" 'BEGIN { printf "%.6f", a+b }')
  processed=$((processed+1))
done < "$TMP/test_qids.txt"

if [ "$processed" -ne "$total_queries" ]; then
  echo "[$SCRIPT_NAME] processed $processed queries but expected $total_queries" >&2
  exit 1
fi

avg_ndcg=$(awk -v c="$cum_ndcg" -v n="$processed" 'BEGIN { if (n>0) printf "%.4f", c/n; else print "n/a" }')
avg_recall=$(awk -v c="$cum_recall" -v n="$processed" 'BEGIN { if (n>0) printf "%.4f", c/n; else print "n/a" }')

{
  # Header shape parsed by bench_report::parse_beir_text_output: the first
  # token before `-ndcg` is the workload, `label=` carries the engine.
  echo "## $DATASET-ndcg label=$LABEL  $(date -Iseconds)"
  echo "url=$URL bulk_ms=$bulk_ms"
  echo "queries_processed=$processed (out of $total_queries unique test qids)"
  echo "NDCG@10 = $avg_ndcg"
  echo "Recall@10 = $avg_recall"
} >>"$OUT" 2>&1
