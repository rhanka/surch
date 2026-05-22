# ci-k8s summary

- Run: https://github.com/rhanka/surch/actions/runs/26266511714
- Attempt: 1
- Ref: main
- Input job: `insee-bench`
- Kubernetes Job: `insee-bench`
- Namespace: `surch`
- Image: `ghcr.io/rhanka/surch:sha-5a93ee331e3e779d851681c136d191c6d89c63a3`
- Bench driver image: `ghcr.io/rhanka/surch:bench-sha-5a93ee331e3e779d851681c136d191c6d89c63a3`
- Artifact: `k8s-bench-insee-bench-5a93ee331e3e779d851681c136d191c6d89c63a3`

## Job conditions

```text
SuccessCriteriaMet=True
Complete=True
```

## Bench summary

# Surch bench summary reports

Generated 2026-05-22T03:42:47Z.

## Artillery results (surch.bench.artillery.v1)

| Engine | Workload | p50 ms | p95 ms | p99 ms | max ms | issued | errors |
|---|---|---:|---:|---:|---:|---:|---:|
| elasticsearch | deces | 4.3 | 9.1 | 17.6 | 317.8 | 13170 | 0 |
| surch | deces | 2.1 | 3.6 | 5.4 | 18.3 | 13170 | 0 |

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

- artillery p95 ≤ 200 ms [art-os] : PASS (observed p95 = 9.1 ms)
- artillery max ≤ 500 ms [art-os] : PASS (observed max = 317.8 ms)
- artillery error rate ≤ 1 % [art-os] : PASS (observed = 0.000 % (0 errors / 13170 issued))
- artillery p95 ≤ 200 ms [art-surch] : PASS (observed p95 = 3.6 ms)
- artillery max ≤ 500 ms [art-surch] : PASS (observed max = 18.3 ms)
- artillery error rate ≤ 1 % [art-surch] : PASS (observed = 0.000 % (0 errors / 13170 issued))

## Skipped files

- /reports/bootstrap-opensearch.bulk.json: no schema field
- /reports/bootstrap-opensearch.put.json: no schema field
- /reports/bootstrap-opensearch.refresh.json: no schema field
- /reports/bootstrap-surch.bulk.json: no schema field
- /reports/bootstrap-surch.put.json: no schema field
- /reports/bootstrap-surch.refresh.json: no schema field

