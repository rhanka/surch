#!/usr/bin/env bash
# TREC-COVID BEIR NDCG@10 + Recall@10 parity test against a BM25-only engine.
# Usage: trec-covid-ndcg.sh <label> <out_file> <url>
#
# Indexes TREC-COVID corpus (171 k docs), runs all 50 test queries, computes
# NDCG@10 per query (graded relevance 0/1/2 -> 2^rel - 1) and Recall@10,
# averages, compares with Lucene/Anserini baseline 0.595.
#
# Idempotent: downloads the BEIR archive to target/beir/ the first time only.
set -u
LABEL="${1:?label}"
OUT="${2:?out}"
URL="${3:?url}"
INDEX="trec-covid"
BEIR_ROOT="/home/antoinefa/src/surch/target/beir"
DATA="$BEIR_ROOT/trec-covid"
ARCHIVE_URL="https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/trec-covid.zip"
ARCHIVE="$BEIR_ROOT/trec-covid.zip"
NDJSON="$DATA/corpus.ndjson"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# -- Step 1: download + extract dataset (idempotent) ------------------------
mkdir -p "$BEIR_ROOT"
if [ ! -s "$DATA/corpus.jsonl" ] || [ ! -s "$DATA/queries.jsonl" ] || [ ! -s "$DATA/qrels/test.tsv" ]; then
  if [ ! -s "$ARCHIVE" ]; then
    echo "[trec-covid] downloading $ARCHIVE_URL (~200 MB) ..." >&2
    curl -fSL --retry 3 -o "$ARCHIVE" "$ARCHIVE_URL"
  fi
  echo "[trec-covid] extracting to $BEIR_ROOT ..." >&2
  ( cd "$BEIR_ROOT" && unzip -oq "$ARCHIVE" )
fi

# -- Step 2: convert corpus.jsonl -> bulk NDJSON (idempotent) ---------------
if [ ! -s "$NDJSON" ]; then
  jq -rc --arg idx "$INDEX" '[
    {"index":{"_id":._id,"_index":$idx}},
    {"id":._id,"title":.title,"text":.text}
  ] | .[] | tostring' "$DATA/corpus.jsonl" > "$NDJSON"
fi

# -- Step 3: (re)create index ----------------------------------------------
curl -fsS -X DELETE "$URL/$INDEX" >/dev/null 2>&1 || true
curl -fsS -X PUT "$URL/$INDEX" -H 'Content-Type: application/json' \
  -d '{"mappings":{"properties":{"title":{"type":"text"},"text":{"type":"text"}}}}' >/dev/null

# Bulk ingest. TREC-COVID is ~200 MB; ship it in 25 MB chunks so we stay below
# any reasonable HTTP body cap.
t0=$(date +%s.%N)
split -C 25m -d -a 4 "$NDJSON" "$TMP/bulk."
for chunk in "$TMP"/bulk.*; do
  # Ensure each chunk ends on a newline (split -C already respects line boundaries)
  curl -fsS -X POST "$URL/_bulk" -H 'Content-Type: application/x-ndjson' \
    --data-binary "@$chunk" >/dev/null
done
t1=$(date +%s.%N)
bulk_ms=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", (b-a)*1000 }')
curl -fsS -X POST "$URL/$INDEX/_refresh" >/dev/null

# -- Step 4: prepare qrels + queries ---------------------------------------
# qrels format: query-id<TAB>corpus-id<TAB>score   (score in {0,1,2}, header line)
awk -F'\t' 'NR>1 && $3>0 { print $1 }' "$DATA/qrels/test.tsv" | sort -u > "$TMP/test_qids.txt"
total_queries=$(wc -l < "$TMP/test_qids.txt")

awk -F'\t' 'NR>1 && $3>0 { print $1"\t"$2"\t"$3 }' "$DATA/qrels/test.tsv" \
  | sort -k1,1 > "$TMP/qrels.tsv"

jq -r '"\(._id)\t\(.text)"' "$DATA/queries.jsonl" > "$TMP/queries.tsv"

cum_ndcg=0
cum_recall=0
processed=0
while read -r qid; do
  qtext=$(awk -F'\t' -v q="$qid" '$1==q{print $2; exit}' "$TMP/queries.tsv")
  [ -z "$qtext" ] && continue
  qjson=$(printf '%s' "$qtext" | jq -Rsa . | sed 's/^"//;s/"$//')
  body=$(printf '{"query":{"multi_match":{"query":"%s","fields":["title","text"]}},"size":10,"track_total_hits":true}' "$qjson")
  resp=$(curl -fsS -X POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' --data "$body")
  top10=$(jq -r '.hits.hits[]._id' <<<"$resp")
  if [ -z "$top10" ]; then
    processed=$((processed+1))
    continue
  fi
  # NDCG@10 with graded relevance: gain = 2^rel - 1
  # IDCG@10 = sort all relevant docs for this qid by rel desc, take top 10
  ndcg=$(awk -v qid="$qid" -v rels_file="$TMP/qrels.tsv" -v top="$top10" '
  function pow2m1(r) { return (2 ^ r) - 1 }
  BEGIN {
    n_rel = 0
    while ((getline line < rels_file) > 0) {
      split(line, f, "\t")
      if (f[1] == qid) {
        rel[f[2]] = f[3] + 0
        rels[n_rel++] = f[3] + 0
      }
    }
    close(rels_file)
    # Sort rels desc (simple insertion sort, n_rel is small per query)
    for (i = 1; i < n_rel; i++) {
      v = rels[i]; j = i - 1
      while (j >= 0 && rels[j] < v) { rels[j+1] = rels[j]; j-- }
      rels[j+1] = v
    }
    cap = (n_rel < 10 ? n_rel : 10)
    idcg = 0
    for (i = 0; i < cap; i++) idcg += pow2m1(rels[i]) / (log(i+2) / log(2))
    if (idcg == 0) { print "0.0"; exit }
    dcg = 0
    rank = 0
    split(top, hits, "\n")
    # gawk preserves split() insertion order; iterate by numeric index for safety.
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
  # Recall@10 = | top10 ∩ relevant | / | relevant |
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
    hit = 0
    rank = 0
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

avg_ndcg=$(awk -v c="$cum_ndcg" -v n="$processed" 'BEGIN { if (n>0) printf "%.4f", c/n; else print "n/a" }')
avg_recall=$(awk -v c="$cum_recall" -v n="$processed" 'BEGIN { if (n>0) printf "%.4f", c/n; else print "n/a" }')

{
  echo "## trec-covid-ndcg label=$LABEL  $(date -Iseconds)"
  echo "url=$URL bulk_ms=$bulk_ms"
  echo "queries_processed=$processed (out of $total_queries unique test qids)"
  echo "NDCG@10 = $avg_ndcg (Lucene/Anserini baseline: 0.595)"
  echo "Recall@10 = $avg_recall"
} >>"$OUT" 2>&1
