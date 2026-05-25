# Methodology — Surch performance evaluation

This is the methodology section backing Objective F
(`plan/wp-f-perf-paper.md`): the experimental harness, metrics
schemas, replay protocol, execution environment, and fairness /
quality controls used to evaluate every Surch optimisation. It
documents what is already in place; the statistical-rigour and
historical-isolation gaps are tracked in the Objective F plan.

## 1. System under test

- **Surch**: an OpenSearch-compatible search engine written in pure
  Rust (no JVM). Single-node, in-memory index with a FoR-encoded
  postings codec, an FST term dictionary, and refcounted stored
  `_source`.
- **Reference engine**: OpenSearch 2.17.1
  (`opensearchproject/opensearch:2.17.1`), single-node,
  `discovery.type=single-node`, security plugin disabled,
  `-Xms1g -Xmx1g`.

Both engines run as sibling containers in the same Kubernetes Pod,
driven over HTTP by a third "bench driver" container, so the network
path and the host are identical for both.

## 2. Execution environment

- **Cluster**: Scaleway managed Kubernetes, dedicated `burst` node
  pool, tenant namespace `surch`.
- **Pod shape** (BEIR `ndcg-gate`): driver `100m/500m CPU,
  128Mi/1Gi`; Surch init-sidecar `150m/2000m CPU, 256Mi/7Gi`;
  OpenSearch init-sidecar `250m/1200m CPU, 256Mi/2Gi`. Engines are
  restartable init containers so the Job completes when the driver
  exits. `activeDeadlineSeconds=3600`. `shareProcessNamespace=true`
  for RSS sampling.
- **Image provenance**: every run pins the runtime image
  `ghcr.io/rhanka/surch:sha-<full commit SHA>` and the bench driver
  `ghcr.io/rhanka/surch:bench-sha-<full commit SHA>`; the workflow
  fails closed if the expected image is missing. Each promoted
  report records the GHA run id, the Job conditions, pod describe,
  live `kubectl top` samples, and cluster events.
- **Dispatch**: `.github/workflows/ci-k8s.yml`
  (`workflow_dispatch`), jobs `ndcg-gate`, `insee-bench`,
  `b1-oracle-gate`, `00-init-corpora`.

## 3. Workloads

- **SciFact** (BEIR): 5 183 docs, 300 test queries. Small corpus —
  exercises quality + warm search, does not surface bulk scaling.
- **TREC-COVID** (BEIR): 171 332 docs, 50 test queries. Large
  long-text corpus — the primary bulk-indexing and memory stress.
  Ingested in pair-aware `_bulk` chunks below Surch's 16 MiB body
  cap.
- **INSEE deces** (matchID): 10 k real INSEE records, an artillery
  latency workload of 13 170 queries across 6 RPS phases
  (`2:30,2:30,5:30,10:30,20:30,50:240`, 8 workers) — multi-field
  name/date AND queries, the search-latency stress.

## 4. Metrics and machine schemas

Each producer emits a versioned JSON envelope; `bench_report`
aggregates them into a stable `surch.bench.summary.v1` plus a human
Markdown report.

- `surch.bench.artillery.v1` — per-engine search latency
  p50/p95/p99/max, issued, errors (from `artillery_bench`).
- `surch.bench.ndcg_gate.v1` — BEIR NDCG@10 / Recall@10 + bulk_ms
  per engine (from the `*-ndcg.sh` gate scripts).
- `surch.bench.rss.v1` — peak/final RSS sampled at 1 Hz from
  `/proc/<pid>/status:VmRSS` (from `scripts/bench/rss-sample.sh`).
  PID resolution matches the engine binary by argv[0] basename
  (`surch-api`; `java` + `org.opensearch.bootstrap`), so the driver
  shell is never mis-sampled.

RSS envelopes are streamed between `BEGIN_SURCH_K8S_RSS_FILE:<name>`
/ `END_…` markers in the driver log and reconstructed by the
workflow after the ephemeral `/reports` volume is gone.

## 5. SLO / quality guardrails

`bench_report` gates each run (exit non-zero on breach):

- artillery p95 ≤ 200 ms, max ≤ 500 ms, error rate ≤ 1 % (both
  engines).
- Surch RSS peak ≤ 1024 MB on the INSEE artillery workload (Surch
  engine only — the JVM reference engine's heap is exempt).
- SciFact NDCG@10 ≥ 0.65, TREC-COVID NDCG@10 ≥ 0.55.

Quality is reported for **every** optimisation run; a perf claim is
only admissible if NDCG@10 / Recall@10 are unchanged vs the prior
lot. Across the Lot 1 → Lot 2 sequence the BEIR quality numbers are
bit-stable (SciFact 0.6576/0.8100, TREC-COVID 0.4750/0.0132).

## 6. Fairness controls

- **Allocator parity**: since Lot 1.7, Surch links jemalloc with
  `MALLOC_CONF=background_thread:true,dirty_decay_ms:0,muzzy_decay_ms:0`,
  matching OpenSearch / Elasticsearch which default to jemalloc on
  Linux. Before Lot 1.7 Surch used the glibc default, which was a
  disadvantage; the article must present pre-1.7 numbers with that
  caveat.
- **Same Pod / host / network** for both engines on every run.
- **Identical corpus and chunking** fed to both engines.

## 7. Isolation protocol

To attribute a delta to a single optimisation, the change is
measured against a **control SHA that differs only by that
optimisation**, on the same stack. Example (Lot 2 skip lists):
control `b9f6636` (jemalloc, no Lot 2) vs `d73c862` (jemalloc +
Lot 2), neither carrying Lot 1.6 (bulk-only, search-neutral), so the
search-latency delta is attributable to the skip lists alone
(`2026-05-25-insee-lot2-skiplists-K8s/`). Bulk-only and search-only
optimisations are cross-checked to be neutral on the other axis
(e.g. the Lot 2-only `ndcg-gate` control showed bulk unchanged,
confirming the bulk gain belongs to Lot 1.6).

## 8. Statistical rigour — current limitation

The Track A replay protocol (`plan/perf-replay-wp-a-algo-ledger.md`)
requires **≥ 3 successful repetitions per compared ref**, aggregated
as median + IQR (or min/median/max), for a *final* verdict. The Lot
1 → Lot 2 measurements published so far are **single-run** and must
be re-run in triplicate before the article cites them as final;
observed tail variance (e.g. INSEE max 21.6 ms vs 64.1 ms across two
Lot 2 runs) makes this mandatory for p99/max claims. Median latency
and bulk/RSS deltas are large enough to be robust to single-run
noise, but the article will report them with the repetition count
stated explicitly.

## 9. Reproducibility

Every promoted report under `docs/ops/bench-reports/<date>-…/`
carries the run id, image tags, Job manifest (`job.yaml`), raw
`summary.md` / `bench.json`, and RSS envelopes, so a third party can
re-dispatch the same `ci-k8s` job at the same SHA and obtain the
same artefacts. Release artefacts themselves are verifiable via
`scripts/verify-release.sh` (`docs/ops/release-verification.md`).
