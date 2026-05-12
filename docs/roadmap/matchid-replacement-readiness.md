# MatchID Elasticsearch Replacement Readiness

Date: 2026-05-12

This note evaluates when Surch can realistically replace Elasticsearch/OpenSearch for MatchID.
It is a decision roadmap, not a compatibility claim: every go decision below requires replayed
MatchID traffic and symmetric HTTP benchmark evidence.

## Current Verdict

Surch is ready for controlled compatibility demos on small BAN-style datasets. It is not ready
as a production Elasticsearch replacement for MatchID yet.

The earliest credible MatchID UAT path is a shadow-read deployment after the benchmark and
compatibility gates below are in place. A production cutover should wait until persistence,
operational recovery, and real MatchID query replay are proven.

## Readiness Levels

| Level | Status | Meaning | Exit Criteria |
|-------|--------|---------|---------------|
| Demo | green | Show Surch answering representative BAN/search flows | Existing Surch API tests and BAN oracle pass on committed fixtures |
| MatchID shadow UAT | yellow | Run Surch beside Elasticsearch without serving users | Real MatchID index mapping + bulk load + query log replay pass, and symmetric HTTP benchmark report is produced |
| Partial traffic | red | Route a low-risk MatchID cohort to Surch | UAT gates pass on production-like data, p95 latency/error budgets are met for 5 repeated runs, rollback is one config switch |
| Full replacement | red | Remove Elasticsearch from the MatchID serving path | Durable index recovery, operational runbook, backup/rebuild process, and compatibility freeze are proven |

## What Is Already Strong

- REST surface: root, index create/delete, document indexing, bulk, refresh, count, search,
  mget, msearch, field caps, analyze, mapping, cluster health, cat endpoints, aliases,
  index templates, component templates.
- Query DSL: `match_all`, `match` with `operator`, `match_phrase`, `term`, `terms`,
  `bool.must`, `range`, `exists`, `prefix`, `wildcard`, `multi_match`, `fuzzy`.
- Search response features: sort, pagination, `_source` filtering, `track_total_hits`,
  highlight fragments, BM25 scoring, alias fan-out, write alias resolution.
- Oracle foundation: committed OpenSearch replay fixtures and BAN tiny oracle replays exist.
- Demo foundation: BAN demo and local OpenSearch lifecycle scripts exist.

## Current Blockers For MatchID Replacement

### P0: Evidence Blockers

1. MatchID query replay does not exist in this repo.
   A safe decision needs a sanitized fixture set from real MatchID requests: mappings,
   bulk payloads, representative `_search`, `_msearch`, `_count`, `_mget`, aliases, and
   expected Elasticsearch responses.

2. Symmetric HTTP benchmark has an initial implementation but still needs real repeated runs.
   `ban-http-bench` now exercises Surch HTTP and OpenSearch HTTP through the same persistent
   Rust HTTP/1.1 client, with configurable timeout and oracle response comparison.
   The current blocker is scale evidence: `ban_tiny` passes, but the first Paris attempt shows
   Surch `_bulk` timeouts at 500 and 25,000 documents and slow match queries at 100 documents.

3. Production-like dataset benchmark is not pinned.
   `ban_tiny` has 3 documents and is only a smoke fixture. A real performance decision needs
   at least the Paris BAN sample or a sanitized MatchID-sized sample, with checksum and load
   procedure recorded.

### P0: Runtime Blockers

1. `surch-api` currently serves from in-memory API state.
   The lower-level store/index crates exist, but the API replacement path still needs durable
   index persistence, reload, and rebuild/recovery behavior before production cutover.

2. Request-time search still scans/re-tokenizes too much of the corpus for some queries.
   BM25 correctness exists, but the API path is not yet using postings as the primary execution
   plan for all scoring queries. This is the main performance risk as data grows.

3. API `_bulk` ingestion is too slow for larger-than-tiny HTTP datasets.
   The Paris attempt in `docs/poc/reports/ban-paris-http-019d91e.md` loaded 25,000 and 500
   documents in OpenSearch, while Surch timed out after 120s and left partial data visible.

4. Operational envelope is not proven.
   There is no production runbook yet for index rebuild, backup/restore, schema migration,
   memory sizing, slow query diagnosis, or rollback.

### Conditional Compatibility Risks

These are not blockers if MatchID does not use them. They become P0 if query logs show usage:

- aggregations;
- nested/object field semantics beyond flat object handling;
- geo queries/sorts;
- scroll/PIT/search_after workflows;
- analyzers beyond the current supported set;
- ingest pipelines;
- per-field similarity/custom analyzers;
- index templates/settings not currently interpreted by Surch;
- update-by-query/delete-by-query/reindex APIs.

## Roadmap To A Credible MatchID Decision

### Milestone 1: MatchID Compatibility Replay

Estimate: 1-2 days after sanitized fixtures are available.

Deliverables:

- `tests/matchid_compat/` with sanitized mappings, bulk samples, and replay manifests.
- Replay runner coverage for MatchID critical paths.
- Gap report that classifies every mismatch as accepted, fixed, or blocking.

Exit criteria:

- 100% of critical MatchID search/count/mget/msearch requests return compatible status,
  totals, top hit IDs, source fields, sort order, and error envelopes.
- Any unsupported API returns an explicit compatible error rather than silent degradation.

### Milestone 2: Symmetric HTTP Benchmark

Estimate: harness implemented; repeated runs are blocked until Surch API ingestion/search
performance improves beyond tiny fixtures.

Deliverables:

- `ban-http-bench` measures Surch HTTP and OpenSearch HTTP with the same Rust client,
  same dataset bytes, same warmup, same iterations, and same request bodies.
- JSON and Markdown reports under `docs/poc/reports/`.
- Hard validation before timing: oracle mismatch aborts the benchmark.

Exit criteria:

- `ban_tiny` smoke passes for both engines
  (`docs/poc/reports/ban-http-4c35045.{json,md}`).
- Paris attempt is documented
  (`docs/poc/reports/ban-paris-http-019d91e.md`) and currently marks ingestion/search
  performance as a blocker.
- Paris BAN or MatchID-sized sample produces 5 successful repeated runs.
- p95 variance is <= 15% or the variance is explicitly reported.
- No global ratio is published; report per-operation latency and throughput.

### Milestone 3: Performance Execution Path

Estimate: 4-8 days for the first serious pass, depending on query mix.

Deliverables:

- API search execution uses postings/index structures for scoring queries instead of
  request-time corpus scans where possible.
- Benchmarks include bulk ingest docs/s, count latency, match/bool/fuzzy p50/p95/p99,
  memory footprint, timeout/error count.

Exit criteria:

- On the selected MatchID-sized sample, Surch meets the agreed latency budget for the
  critical query classes.
- No query class has unbounded corpus-scan behavior that violates the budget.

### Milestone 4: Durable Serving Path

Estimate: 1-2 weeks for UAT-grade durability; more for production hardening.

Deliverables:

- API state backed by durable store/index files or a documented rebuild-from-source path.
- Startup reload/recovery test.
- Crash/restart smoke.
- Operational runbook.

Exit criteria:

- Restart preserves or deterministically rebuilds the index.
- Bulk load and refresh semantics are documented and tested.
- Rollback path to Elasticsearch is rehearsed.

## When Replacement Becomes Possible

Best-case shadow UAT: after Milestones 1 and 2 pass, assuming MatchID only uses the already
implemented API/query subset. That is likely a few focused development days once fixtures are
available.

Best-case partial production traffic: after Milestones 1, 2, and 3 pass on production-like data,
with Surch behind a feature flag and Elasticsearch kept as rollback. That is plausible after
roughly 1-2 weeks of focused work if the query mix stays inside the current API surface.

Full Elasticsearch removal: after Milestone 4 and operational hardening. Treat this as a
multi-week target, not the next immediate milestone.

## Immediate Next Work

1. Produce the first BAN HTTP report on `ban_tiny` and then the pinned Paris BAN sample.
2. Add no-bind tests for `ban-http-bench` report serialization and oracle mismatch rejection.
3. Add a `tests/matchid_compat/README.md` and fixture contract so MatchID traffic can be
   sanitized and replayed without leaking production data.
4. Inspect MatchID Elasticsearch usage and classify every API/query feature as supported,
   unsupported-but-unused, or blocking.
