# BEIR NDCG gate

## SciFact / Surch
## scifact-ndcg label=ndcg-surch-k8s  2026-05-24T03:18:48+00:00
url=http://127.0.0.1:7700 bulk_ms=2289.1
queries_processed=300 (out of 300 unique test qids)
NDCG@10 = 0.6576 (Lucene/Anserini baseline: 0.688)
Recall@10 = 0.8100

## SciFact / OpenSearch
## scifact-ndcg label=ndcg-os-k8s  2026-05-24T03:20:48+00:00
url=http://127.0.0.1:9200 bulk_ms=10891.2
queries_processed=300 (out of 300 unique test qids)
NDCG@10 = 0.6537 (Lucene/Anserini baseline: 0.688)
Recall@10 = 0.8033

## TREC-COVID / Surch
## trec-covid-ndcg label=ndcg-surch-k8s  2026-05-24T03:24:59+00:00
url=http://127.0.0.1:7700 bulk_ms=179859.6
queries_processed=50 (out of 50 unique test qids)
NDCG@10 = 0.4750 (Lucene/Anserini baseline: 0.595)
Recall@10 = 0.0132

## TREC-COVID / OpenSearch
## trec-covid-ndcg label=ndcg-os-k8s  2026-05-24T03:27:35+00:00
url=http://127.0.0.1:9200 bulk_ms=87044.5
queries_processed=50 (out of 50 unique test qids)
NDCG@10 = 0.4902 (Lucene/Anserini baseline: 0.595)
Recall@10 = 0.0132
