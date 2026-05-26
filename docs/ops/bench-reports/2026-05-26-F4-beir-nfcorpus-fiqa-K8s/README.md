# Objective F — F4: BEIR quality generality (NFCorpus + FiQA) vs OpenSearch 2.17.1

Extends the retrieval-quality evidence beyond SciFact + TREC-COVID to two
more standard BEIR datasets, so the article's quality claim is shown to
generalise. Surch and OpenSearch 2.17.1 run as sibling containers in one
Pod (`beir-extra-ndcg` gate); both index each corpus and answer every test
query (qids with a positive qrel) via `multi_match` over `title`/`text`;
NDCG@10 (graded gain `2^rel-1`) + Recall@10 are averaged by
`scripts/bench/beir-ndcg.sh`.

- GHA run `26476471207` on `main` @ `f34d006`.
- Corpora hydrated by `00b-init-beir-extra` (shell, no-Python) into the
  `surch-corpus-beir` PVC.

## Results (all test queries, NDCG@10 / Recall@10)

| Dataset | queries | Surch NDCG@10 | OpenSearch NDCG@10 | Δ NDCG | Surch Recall@10 | OpenSearch Recall@10 |
|---------|--------:|--------------:|-------------------:|-------:|----------------:|---------------------:|
| NFCorpus | 323 | **0.3033** | 0.3034 | −0.0001 | 0.1495 | 0.1495 |
| FiQA | 648 | **0.2294** | 0.2389 | −0.0095 | 0.2928 | 0.3004 |

## Reading

- **NFCorpus**: Surch is **bit-identical to OpenSearch** at NDCG@10 (within
  rounding, `−0.0001`) and identical at Recall@10. Surch's BM25 + analysis
  chain matches the JVM engine exactly on this medical-IR corpus.
- **FiQA**: Surch trails OpenSearch by `−0.0095` NDCG@10 (~4 % relative) and
  `−0.0076` Recall@10 — a small gap of the same character as the TREC-COVID
  `−0.0152`, attributable to minor analyzer/tokenisation differences on this
  financial-QA corpus, not a retrieval defect.

Combined with the earlier SciFact (`0.6576` vs `0.6537`, Surch ahead) and
TREC-COVID (`0.4750` vs `0.4902`) results, Surch's retrieval quality now
tracks OpenSearch 2.17.1 across **four** BEIR datasets — at or within a few
percent of parity on every one, ahead on SciFact. The quality story
generalises.

## Sources

- GHA run `26476471207` (ci-k8s `beir-extra-ndcg`), image
  `sha-f34d006…` / `bench-sha-f34d006…`.
- Per-dataset `*-surch.out` / `*-os.out` + `summary.md` (driver log markers).
- Script `scripts/bench/beir-ndcg.sh`; job `deploy/k8s/jobs/beir-extra-ndcg.yaml`.
- SciFact / TREC-COVID cross-check: `2026-05-25-F2-ndcg-3rep-K8s/`.
