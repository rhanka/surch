# BEIR NDCG gate

## SciFact / Surch
## scifact-ndcg label=ndcg-surch-k8s  2026-05-20T10:48:31+00:00
url=http://127.0.0.1:7700 bulk_ms=4097.5
queries_processed=300 (out of 300 unique test qids)
NDCG@10 = 0.6576 (Lucene/Anserini baseline: 0.688)
Recall@10 = 0.8100

## SciFact / OpenSearch
## scifact-ndcg label=ndcg-os-k8s  2026-05-20T10:50:47+00:00
url=http://127.0.0.1:9200 bulk_ms=12088.0
queries_processed=300 (out of 300 unique test qids)
NDCG@10 = 0.6537 (Lucene/Anserini baseline: 0.688)
Recall@10 = 0.8033

## TREC-COVID / Surch
## trec-covid-ndcg label=ndcg-surch-k8s  2026-05-20T10:51:48+00:00
url=http://127.0.0.1:7700 bulk_ms=5116.1
queries_processed=50 (out of 50 unique test qids)
NDCG@10 = 0.0000 (Lucene/Anserini baseline: 0.595)
Recall@10 = 0.0000

## TREC-COVID / OpenSearch
## trec-covid-ndcg label=ndcg-os-k8s  2026-05-20T10:53:32+00:00
url=http://127.0.0.1:9200 bulk_ms=28710.8
queries_processed=50 (out of 50 unique test qids)
NDCG@10 = 0.1141 (Lucene/Anserini baseline: 0.595)
Recall@10 = 0.0026
