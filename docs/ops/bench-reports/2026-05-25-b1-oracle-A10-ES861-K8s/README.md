# b1 matchID oracle — 2026-05-25 (A10, parity vs Elasticsearch 8.6.1)

matchID B1 oracle replay on the Track D Phase 4 **A10** SHA
(write-time sub-field fan-out), confirming the new indexing feature
preserves parity with Elasticsearch 8.6.1.

## Result

| Field | Value |
|-------|------:|
| total_requests | 30 |
| divergence_count | **0** |
| skipped_count | 0 |
| reference | Elasticsearch 8.6.1 (`docker.elastic.co/elasticsearch/elasticsearch:8.6.1`) |

A10 introduces write-time fan-out of multi-field sub-fields
(`NOM.raw`, `NOM.norm`, …): at index time the parent value is
re-analysed with each sub-field's own analyzer/normalizer and stored
(qualified `parent.sub` postings + a `subfield_values` side-table).
The B1 oracle replays the 30 matchID `deces_v1` requests against both
Surch and Elasticsearch 8.6.1 and finds **0 divergence** — the
fan-out does not change any B1 query result.

## Scope of A10 validated here

- **Indexing / storage**: sub-fields are fanned out and stored at
  write time (the agent's `crates/surch-index` tests cover the
  fan-out + mapping round-trip).
- **No B1 regression**: the existing 30-request oracle stays
  0-divergence.

Deferred (A1/A12 follow-ups, not in this run): threading
`DocumentIndex::subfield_value` into `sort`/`agg.cardinality` so the
query side consumes the stored `.raw`/`.norm` projection without a
source-scan. The storage is now in place for that.

## Provenance

- GHA run: <https://github.com/rhanka/surch/actions/runs/26404122287>
- Workflow: `.github/workflows/ci-k8s.yml` (`workflow_dispatch`,
  `job=b1-oracle-gate`), Job PASS (oracle exit=0).
- Head SHA: `e293cfc` (main, Lot 3 + A10).
- Surch image: `ghcr.io/rhanka/surch:sha-e293cfc…`.
- Raw files: `b1-oracle.json` (`surch.bench.b1_oracle.v1`), `job.yaml`.

## Closure

Track D Phase 4 A10 indexing/storage lands with matchID parity
preserved (B1 30/30, 0 divergence). The next Phase 4 lots
(A1 multi-field widening, A12 composite/agg consuming `.raw`) build
on this storage.
