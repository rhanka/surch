# Surch bench summary reports

Generated 2026-05-25T14:23:00Z.

## Artillery results (surch.bench.artillery.v1)

| Engine | Workload | p50 ms | p95 ms | p99 ms | max ms | issued | errors |
|---|---|---:|---:|---:|---:|---:|---:|
| elasticsearch | deces | 3.8 | 11.1 | 23.9 | 310.5 | 13170 | 0 |
| surch | deces | 1.4 | 3.7 | 7.2 | 47.9 | 13170 | 0 |

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
| opensearch | art | 1376.0 | 1376.0 |
| surch | art | 65.0 | 41.0 |

## SLO checks

- artillery p95 ≤ 200 ms [art-os] : PASS (observed p95 = 11.1 ms)
- artillery max ≤ 500 ms [art-os] : PASS (observed max = 310.5 ms)
- artillery error rate ≤ 1 % [art-os] : PASS (observed = 0.000 % (0 errors / 13170 issued))
- artillery p95 ≤ 200 ms [art-surch] : PASS (observed p95 = 3.7 ms)
- artillery max ≤ 500 ms [art-surch] : PASS (observed max = 47.9 ms)
- artillery error rate ≤ 1 % [art-surch] : PASS (observed = 0.000 % (0 errors / 13170 issued))
- Surch RSS peak ≤ 1024 MB (artillery on INSEE) [rss-art-surch] : PASS (observed peak = 65.0 MB)

## Skipped files

- /reports/bootstrap-opensearch.bulk.json: no schema field
- /reports/bootstrap-opensearch.put.json: no schema field
- /reports/bootstrap-opensearch.refresh.json: no schema field
- /reports/bootstrap-surch.bulk.json: no schema field
- /reports/bootstrap-surch.put.json: no schema field
- /reports/bootstrap-surch.refresh.json: no schema field

