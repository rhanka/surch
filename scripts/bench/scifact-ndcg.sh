#!/usr/bin/env bash
# SciFact BEIR NDCG@10 parity test against a BM25-only engine.
# Usage: scifact-ndcg.sh <label> <out_file> <url>
#
# Indexes SciFact corpus, runs all 300 test queries, computes NDCG@10
# per query, averages, compares with Lucene/Anserini baseline 0.688.
set -u
LABEL="${1:?label}"
OUT="${2:?out}"
URL="${3:?url}"
INDEX="scifact"
DATA="/home/antoinefa/src/surch/target/beir/scifact"
NDJSON="$DATA/corpus.ndjson"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

if [ ! -s "$NDJSON" ]; then
  # Convert corpus.jsonl -> bulk NDJSON (index action + source)
  jq -rc --arg idx "$INDEX" '[
    {"index":{"_id":._id,"_index":$idx}},
    {"id":._id,"title":.title,"text":.text}
  ] | .[] | tostring' "$DATA/corpus.jsonl" > "$NDJSON"
fi

curl -fsS -X DELETE "$URL/$INDEX" >/dev/null 2>&1 || true
curl -fsS -X PUT "$URL/$INDEX" -H 'Content-Type: application/json' \
  -d '{"mappings":{"properties":{"title":{"type":"text"},"text":{"type":"text"}}}}' >/dev/null
t0=$(date +%s.%N)
curl -fsS -X POST "$URL/_bulk" -H 'Content-Type: application/x-ndjson' --data-binary "@$NDJSON" >/dev/null
t1=$(date +%s.%N)
bulk_ms=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", (b-a)*1000 }')
curl -fsS -X POST "$URL/$INDEX/_refresh" >/dev/null

# Extract test query ids and their relevant doc ids from qrels
# Format: query-id<TAB>corpus-id<TAB>score
awk -F'\t' 'NR>1 && $3>0 { print $1 }' "$DATA/qrels/test.tsv" | sort -u > "$TMP/test_qids.txt"
total_queries=$(wc -l < "$TMP/test_qids.txt")

# Build qid -> {rel_doc_id : score} map (TSV "qid<TAB>doc<TAB>score")
awk -F'\t' 'NR>1 && $3>0 { print $1"\t"$2"\t"$3 }' "$DATA/qrels/test.tsv" \
  | sort -k1,1 > "$TMP/qrels.tsv"

# Build qid -> query text map
jq -r '"\(._id)\t\(.text)"' "$DATA/queries.jsonl" > "$TMP/queries.tsv"

cum_ndcg=0
processed=0
recall_count=0  # number of queries with at least 1 relevant doc found in top-10
while read -r qid; do
  qtext=$(awk -F'\t' -v q="$qid" '$1==q{print $2; exit}' "$TMP/queries.tsv")
  [ -z "$qtext" ] && continue
  # Escape for JSON
  qjson=$(printf '%s' "$qtext" | jq -Rsa . | sed 's/^"//;s/"$//')
  body=$(printf '{"query":{"multi_match":{"query":"%s","fields":["title","text"]}},"size":10,"track_total_hits":false}' "$qjson")
  resp=$(curl -fsS -X POST "$URL/$INDEX/_search" -H 'Content-Type: application/json' --data "$body")
  # Top hit ids in rank order
  top10=$(jq -r '.hits.hits[]._id' <<<"$resp")
  if [ -z "$top10" ]; then
    processed=$((processed+1))
    continue
  fi
  # Compute DCG@10 from binary relevance (score=1 for relevant, 0 otherwise)
  # and IDCG@10 = sum over r=1..min(R,10) of 1/log2(r+1) where R = number of relevant
  ndcg=$(awk -v qid="$qid" -v rels_file="$TMP/qrels.tsv" -v top="$top10" '
  BEGIN {
    # Load rels for this qid
    n_rel = 0
    while ((getline line < rels_file) > 0) {
      split(line, f, "\t")
      if (f[1] == qid) {
        rel[f[2]] = f[3]
        n_rel++
      }
    }
    close(rels_file)
    # IDCG@10
    idcg = 0
    cap = (n_rel < 10 ? n_rel : 10)
    for (i = 1; i <= cap; i++) idcg += 1.0 / log(i+1) * log(2)
    if (idcg == 0) { print "0.0"; exit }
    # DCG@10 over the hit list
    dcg = 0
    rank = 0
    split(top, hits, "\n")
    for (i in hits) {
      if (hits[i] == "") continue
      rank++
      if (rank > 10) break
      if (hits[i] in rel) {
        dcg += 1.0 / log(rank+1) * log(2)
      }
    }
    printf "%.6f\n", dcg/idcg
  }')
  # Check recall@10
  found=$(awk -v top="$top10" -v rels_file="$TMP/qrels.tsv" -v qid="$qid" '
  BEGIN {
    while ((getline line < rels_file) > 0) {
      split(line, f, "\t")
      if (f[1] == qid) rel[f[2]] = 1
    }
    close(rels_file)
    n = split(top, hits, "\n")
    for (i in hits) if (hits[i] in rel) { print 1; exit }
    print 0
  }')
  cum_ndcg=$(awk -v a="$cum_ndcg" -v b="$ndcg" 'BEGIN { printf "%.6f", a+b }')
  recall_count=$((recall_count + found))
  processed=$((processed+1))
done < "$TMP/test_qids.txt"

avg_ndcg=$(awk -v c="$cum_ndcg" -v n="$processed" 'BEGIN { if (n>0) printf "%.4f", c/n; else print "n/a" }')
recall=$(awk -v r="$recall_count" -v n="$processed" 'BEGIN { if (n>0) printf "%.4f", r/n; else print "n/a" }')

{
  echo "## scifact-ndcg label=$LABEL  $(date -Iseconds)"
  echo "url=$URL bulk_ms=$bulk_ms"
  echo "queries_processed=$processed (out of $total_queries unique test qids)"
  echo "NDCG@10 = $avg_ndcg (Lucene/Anserini baseline: 0.688)"
  echo "Recall@10 = $recall"
} >>"$OUT" 2>&1
