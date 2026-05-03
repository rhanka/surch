# SPEC - MatchID Elasticsearch Parity Exit Criteria

## Purpose

Define the final acceptance target for Surch in the MatchID context.

The goal is not to clone MatchID as a whole product.
The goal is to reproduce Elasticsearch behavior as MatchID uses it.

## Final Target

Surch must be able to stand in for Elasticsearch for MatchID-relevant search behavior with:

- zero accepted search gap on the agreed reference corpus
- no performance regression against the current Elasticsearch baseline
- reproducible proof on correctness and performance

## Scope Of This Exit Criterion

In scope:
- query semantics that MatchID actually uses
- response behavior that MatchID depends on
- ranking and candidate ordering where Elasticsearch behavior is user-visible through MatchID usage
- performance on representative MatchID workloads

Out of scope:
- cloning all MatchID services
- reproducing unrelated MatchID UI or workflow behavior
- parity on Elasticsearch features that MatchID never exercises

## Acceptance Metric 1 - Zero Search Gap

Definition:
- on a frozen MatchID-derived request corpus, Surch and Elasticsearch must return the same accepted result set for the agreed comparison contract

Default comparison contract to use:
- same match or no-match decision
- same ordered top results for compared scenarios
- same key identifiers in returned hits
- same pagination-visible result ordering

If any divergence exists, it must be classified as:
- expected and explicitly waived
- or release-blocking

Default stance for MVP:
- unexplained divergence is blocking

## Acceptance Metric 2 - No Performance Regression

Definition:
- Surch must be at least as performant as the Elasticsearch baseline on the approved MatchID workloads

Minimum performance axes:
- single-search latency
- single-search throughput
- bulk or batch search throughput if MatchID uses it in production flows
- tail latency under realistic load

## Reference Corpus Strategy

Use MatchID-derived corpora, not synthetic-only corpora.

Required corpus families:
- golden correctness corpus
- representative replay corpus
- adversarial corpus

Golden correctness corpus:
- curated requests covering the core MatchID search surface
- expected Elasticsearch outputs frozen and versioned

Representative replay corpus:
- sampled real or realistic MatchID request shapes
- broad enough to cover common names, dates, localities, and typo patterns

Initial mini-dataset baseline selected for first parity loop:
- `deces-2020-m01.txt.gz`
- `60,584` records
- manifest: `tests/matchid_parity/matchid_2020m01_manifest.json`

Adversarial corpus:
- typos
- swapped names
- sparse inputs
- foreign places
- historic locality edge cases
- no-match cases

## Benchmark Strategy

Artillery remains useful but is not enough on its own.

The benchmark stack should contain:

1. API load benchmark
- keep artillery or equivalent for HTTP-level load
- include p50, p95, p99, max, throughput, and error rate

2. Query replay benchmark
- replay frozen MatchID-like requests against Elasticsearch and Surch
- compare correctness and timing together

3. Batch or bulk benchmark
- if MatchID depends on batch flows, benchmark them explicitly

4. Warm and cold runs
- separate cold-start and warm-cache behavior

## Default Pass / Fail Thresholds

Correctness:
- zero unexplained search-gap cases on the frozen release corpus

Performance:
- Surch p95 and p99 not worse than Elasticsearch beyond agreed noise margin
- throughput no worse than Elasticsearch on the same hardware and corpus
- error rate no higher than Elasticsearch baseline under benchmark conditions

## UAT Checkpoints

### UAT-1 - Query Surface Lock
- confirm the exact MatchID query shapes that define the parity target

### UAT-2 - Correctness Replay
- replay golden corpus on Elasticsearch and Surch
- inspect any diff category before acceptance

### UAT-3 - Representative Load
- run realistic request mix benchmark
- compare latency and throughput

### UAT-4 - Adversarial Confidence
- run typo and edge-case corpus
- confirm no silent degradation on hard cases

### UAT-5 - Final Release Decision
- correctness green
- performance parity green
- no unresolved blocking divergence

## Open Questions To Resolve Later

- exact MatchID request corpus to freeze first
- exact tolerated noise margin for perf comparisons
- exact output fields to compare for release-level zero-gap validation

## Relation To Surch Branches

This exit-criteria spec should inform:
- BR-05 search semantics
- BR-07 search API compatibility
- BR-08 release hardening and final validation

## Current Gap To Final Release

Current state:
- Surch core and API technical slices are green on workspace tests and clippy.
- OpenSearch-like syntax coverage for the MVP surface is implemented and tested locally.

What is still missing before final user-level release sign-off:
- a frozen MatchID-derived golden corpus for Elasticsearch-vs-Surch comparison
- a reproducible zero-gap comparison harness
- a reproducible performance parity harness using MatchID-relevant workloads
- user-facing UAT sign-off on those two evidences

Therefore:
- technical MVP core is close to release-ready
- final release is not approved until MatchID-context parity evidence exists
