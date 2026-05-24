# BEIR NDCG gate

## SciFact / Surch
## scifact-ndcg label=ndcg-surch-k8s  2026-05-24T12:06:15+00:00
url=http://127.0.0.1:7700 bulk_ms=1697.8
queries_processed=300 (out of 300 unique test qids)
NDCG@10 = 0.6576 (Lucene/Anserini baseline: 0.688)
Recall@10 = 0.8100

## SciFact / OpenSearch
## scifact-ndcg label=ndcg-os-k8s  2026-05-24T12:08:32+00:00
url=http://127.0.0.1:9200 bulk_ms=13443.1
queries_processed=300 (out of 300 unique test qids)
NDCG@10 = 0.6537 (Lucene/Anserini baseline: 0.688)
Recall@10 = 0.8033

## TREC-COVID / Surch
## trec-covid-ndcg label=ndcg-surch-k8s  2026-05-24T12:12:11+00:00
url=http://127.0.0.1:7700 bulk_ms=139048.0
queries_processed=50 (out of 50 unique test qids)
NDCG@10 = 0.4750 (Lucene/Anserini baseline: 0.595)
Recall@10 = 0.0132

## TREC-COVID / OpenSearch
## trec-covid-ndcg label=ndcg-os-k8s  2026-05-24T12:15:07+00:00
url=http://127.0.0.1:9200 bulk_ms=97830.6
queries_processed=50 (out of 50 unique test qids)
NDCG@10 = 0.4902 (Lucene/Anserini baseline: 0.595)
Recall@10 = 0.0132
