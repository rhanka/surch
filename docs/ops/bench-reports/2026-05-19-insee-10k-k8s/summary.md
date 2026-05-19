# Surch bench summary reports

Generated 2026-05-19T14:01:38Z.

## Artillery results (surch.bench.artillery.v1)

| Engine | Workload | p50 ms | p95 ms | p99 ms | max ms | issued | errors |
|---|---|---:|---:|---:|---:|---:|---:|
| elasticsearch | deces | 3.8 | 9.9 | 20.8 | 135.3 | 13170 | 0 |
| surch | deces | 1.9 | 3.6 | 6.9 | 17.9 | 13170 | 0 |

## BAN HTTP results (surch.bench.ban_http.v1)

| Engine | Operation | status | iterations | p50 us | p95 us | p99 us | max us | errors | docs/s | bytes/s |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| _no data_ |  |  |  |  |  |  |  |  |  |  |

## BEIR retrieval results

| Engine | Workload | NDCG@10 | Recall@10 | processed | total | bulk ms | Lucene baseline NDCG@10 |
|---|---|---:|---:|---:|---:|---:|---:|
| _no data_ |  |  |  |  |  |  |  |

## RSS samples (surch.bench.rss.v1)

| Engine | Workload | peak MB | final MB |
|---|---|---:|---:|
| _no data_ |  |  |  |

## SLO checks

- artillery p95 ≤ 200 ms [art-os] : PASS (observed p95 = 9.9 ms)
- artillery max ≤ 500 ms [art-os] : PASS (observed max = 135.3 ms)
- artillery error rate ≤ 1 % [art-os] : PASS (observed = 0.000 % (0 errors / 13170 issued))
- artillery p95 ≤ 200 ms [art-surch] : PASS (observed p95 = 3.6 ms)
- artillery max ≤ 500 ms [art-surch] : PASS (observed max = 17.9 ms)
- artillery error rate ≤ 1 % [art-surch] : PASS (observed = 0.000 % (0 errors / 13170 issued))

## Skipped files

- /reports/bootstrap-opensearch.bulk.json: no schema field
- /reports/bootstrap-opensearch.put.json: no schema field
- /reports/bootstrap-opensearch.refresh.json: no schema field
- /reports/bootstrap-surch.bulk.json: no schema field
- /reports/bootstrap-surch.put.json: no schema field
- /reports/bootstrap-surch.refresh.json: no schema field

