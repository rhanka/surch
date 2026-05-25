# wp-a-perf-followups Plan

Track principal: A - perf / optimisation
Branch: `main` (no dedicated long branch; follow-ups land directly on
`main` like Lot 3, with paired perf proof in the same delivery slice)
Owner: conductor / SearchEngine / StorageEngine / IndexEngine depending
on lot
Status: open. Track A delivered lots are recorded in
`plan/wp-a-optim.md` (closed at `c5980ad` on 2026-05-20). This file
owns the forward queue of perf follow-ups; the historical cumulative
replay line is owned by `plan/perf-replay-wp-a-algo-ledger.md`.

## Finality

- [ ] Continue measurable Surch search/index performance gains
  without quality regression, with each follow-up landing a paired
  K8s perf proof + Track A ledger row in the same delivery slice.

## Scope

- [x] Forward perf work only (TREC-COVID bulk scaling, skip lists,
  next Block-Max WAND step). The 3 historical A-replay-1/2/3 points
  remain owned by `plan/perf-replay-wp-a-algo-ledger.md` and are
  cited from Lot 4 below without duplication.
- [x] Each follow-up Lot must close at least one row in
  `docs/ops/bench-reports/track-a-performance-ledger.md`.
- [x] No re-opening of historical commits (no rebase, no amend, no
  history rewrite). Patches are forward-only.
- [x] Evidence source: K8s `ndcg-gate` and `insee-bench` artifacts on
  Scaleway burst pool, plus the paired RSS samplers wired in
  `b9faefe`.

## Hors scope

- [x] Track D matchID parity work (`plan/wp-d-matchid-phase4.md`).
- [x] Track C release verification (`plan/wp-c-ops.md` Lot 4).
- [x] Track B / Track E sampling parity follow-ups beyond the
  upcoming `b9faefe` replay.

## Required proof per Lot

For every Lot below the closure delivery slice must carry, in the
same commit range:

- A promoted report under `docs/ops/bench-reports/<date>-A-...-K8s/`
  with at least one paired Surch vs Elasticsearch/OpenSearch K8s run
  id and artefact id.
- Updated row(s) in
  `docs/ops/bench-reports/track-a-performance-ledger.md` citing the
  promoted report.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace` all green.

## Lots

### Lot 1 — TREC-COVID bulk scaling

Trigger: `docs/ops/bench-reports/2026-05-22-ndcg-gate-7Gi-K8s/`
exposed Surch bulk at `1001.95 s` on the full 171 k TREC-COVID
corpus vs OpenSearch at `72.27 s` — OpenSearch is `13.9x` faster on
the same 8 MiB pair-aware chunks under the same Surch 7 GiB cap.
SciFact bulk parity is not affected (Surch is `2.1x` faster there).
Reproduced on `137b352` paired run: `1112.52 s` vs `93.80 s`
(OpenSearch `11.9x` faster), same ranking, same magnitudes.

**Root cause confirmed by code read** (2026-05-23):

- `crates/surch-api/src/state.rs:919`
  (`apply_document_writes`) — after every `_bulk` chunk, the touched
  index calls `IndexData::rebuild_index()`.
- `crates/surch-api/src/state.rs:220` (`rebuild_index`) does
  `self.index.clear()` + `add_documents_with_mapping(documents, …)`
  over **all** `self.documents` (cumulative store), not just the
  newly upserted ids. This makes the bulk path O(N_cumul) per
  chunk and overall ~O(N² / chunk_size) over the corpus.
- TREC-COVID 171 k docs split in ~21 chunks of ~8 MiB each →
  cumulative re-indexing of roughly 21·22/2 · chunk_size docs
  ≈ 1.85 M doc-reindexings, matching the observed ~17 min wall
  clock. SciFact (one chunk, 5 183 docs) does not surface the
  pathology.
- `crates/surch-api/src/state.rs:713`
  (`AppState::refresh_index`) is currently a no-op while the
  router exposes `POST /:index/_refresh` (handler in
  `crates/surch-api/src/index.rs:171`), so callers that already
  refresh after bulk (SciFact, TREC-COVID, INSEE bootstrap, snapshot
  e2e) do not benefit from any deferred work.
- `crates/surch-index/src/document_index.rs:110` accepts strictly
  new doc ids (`DuplicateDocId` on collision, L125) and rebuilds
  the term dictionary unconditionally at L148 (`self.terms =
  self.postings_builder.clone().build();`), so a naive incremental
  add still pays an O(N_terms_cumul) rebuild per call.

**Proposed fix axes** (to arbitrate before implementation):

- (a) Defer rebuild to refresh: replace `rebuild_index()` in
  `apply_document_writes` with a `dirty` flag on `IndexData`;
  `refresh_index` becomes the rebuild trigger. Matches OpenSearch
  semantics. Breaks 2 existing tests (`aliases.rs`,
  `bulk_router.rs`) that issue `_bulk` then `_search` without an
  intermediate refresh; those tests should be updated to call
  `_refresh` (ES-compatible).
- (b) Same as (a) plus lazy rebuild on the read path: search /
  count / get-mapping check the `dirty` flag and trigger a
  rebuild under a write lock if needed, preserving the strict
  "writes immediately visible" semantics for callers that skip
  refresh. Heavier locking pattern.
- (c) Incremental add only: keep eager rebuild semantics but
  rebuild incrementally — extend `DocumentIndex` with a non-clearing
  `append_documents` API plus a separate `finalize_postings` /
  `refresh_terms` step that runs once per chunk instead of per
  cumulative doc. Largest change, preserves semantics, ~O(N) over
  the corpus instead of O(N²).
- (d) Document the corpus-shape limit and keep the eager rebuild;
  cap TREC-COVID in the gate at a smaller sample (e.g. qrels-only
  pool + sampled distractors). Lowest engineering cost but the
  ledger Bulk row stays "OpenSearch wins by `12-14x` on long-text /
  large-corpus shapes".

- [x] Reproduce the bulk gap on the K8s harness (done: `d9cac15` +
  `137b352` runs, both promoted under
  `docs/ops/bench-reports/2026-05-22-ndcg-gate-7Gi-K8s/` and
  `docs/ops/bench-reports/2026-05-23-ndcg-gate-7Gi-RSS-K8s/`).
- [x] Identify the dominant cost (done: `rebuild_index` re-indexes
  the cumulative document store after every `_bulk` chunk; the
  refresh handler is a no-op).
- [x] User picked axis (c) full incremental refactor of
  `DocumentIndex`.
- [x] Implemented in `367acdc`: `IndexData::append_to_index` takes
  only the freshly inserted doc ids; `apply_document_writes` routes
  pure-insert bulks to the incremental path and any update/delete
  bulk to the legacy `rebuild_index`. New test
  `bulk_router_accumulates_across_multiple_chunks` guards the
  multi-chunk accumulation.
- [x] Re-ran K8s `ndcg-gate` and promoted
  `docs/ops/bench-reports/2026-05-24-ndcg-gate-incremental-bulk-K8s/`:
  Surch TREC-COVID bulk `1001.95 s -> 179.86 s` (`~5.6x` speedup),
  Surch/OpenSearch ratio `13.9x -> 2.06x`. NDCG@10 and Recall@10
  unchanged. Track A performance ledger Bulk + RSS rows updated.
- [x] Gate: ledger Bulk row no longer flags "Surch ingest scaling
  for large-corpus / long-text shapes is the next target" — it now
  cites the new run and points at the term-dictionary rebuild as
  the next attack surface.

### Lot 1.5 — Free the PostingsBuilder snapshot on refresh

Trigger: `2026-05-24-ndcg-gate-incremental-bulk-K8s/` shows Surch
RSS peak rose from `4802 MiB` (full-rebuild path) to `5859 MiB`
(incremental path). The delta `~1057 MiB` is the live
`PostingsBuilder` snapshot kept alive across chunks to allow
incremental `append_to_index` calls. Once the index is declared
read-mostly (via `POST /:index/_refresh`), the snapshot is dead
weight and can be dropped.

- [x] Made `AppState::refresh_index` (was a no-op at
  `crates/surch-api/src/state.rs:713`) call
  `IndexData::finalize_terms_for_refresh` which drops the
  `PostingsBuilder` via `DocumentIndex::finalize_postings()`.
- [x] Added `terms_finalized: bool` on `InMemoryIndex` to track the
  post-refresh state.
- [x] `IndexData::append_to_index` falls back to a one-shot
  `rebuild_index()` if `terms_finalized` is true, so a
  bulk-after-refresh preserves previously-indexed postings.
- [x] Removed the unconditional `finalize_postings()` from
  `rebuild_index()`: post-Lot-1 the builder is the source of truth
  for further appends; only `refresh_index` finalizes.
- [x] Test `bulk_router_bulk_refresh_bulk_search_preserves_old_docs`
  in `crates/surch-api/tests/bulk_router.rs` covers the fallback.
- [x] K8s `ndcg-gate` run `26359069219` on `01ad77e` promoted as
  `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.5-ram-K8s/`.
  Surch RSS peak `5859 -> 5591 MiB` (`-268 MiB`), Lot 1 bulk gain
  preserved, NDCG@10 unchanged.

**Caveat**: the logical fix works (test passes; sampler shows the
delta) but the system RSS gain is modest because glibc's default
allocator keeps freed heap pages mapped without memory pressure.
Recovering the full `~1 GiB` requires an orthogonal allocator-level
follow-up (Lot 1.7).

### Lot 1.7 — Allocator memory return after refresh

Trigger: `2026-05-24-ndcg-gate-lot1.5-ram-K8s/` shows Lot 1.5
recovers only `268 MiB` of the `~1057 MiB` logically freed by
`finalize_postings()`. The remainder stays mapped in the glibc
heap because the allocator does not call `madvise(MADV_DONTNEED)`
without memory pressure. With the Surch sidecar's 7 GiB cap and
the steady-state peak around 5.5 GiB, there is no pressure.

- [x] User chose option B: switch the Surch global allocator to
  jemalloc via `tikv-jemallocator` 0.6, scoped to
  `cfg(target_os = "linux")` (runtime image is
  `gcr.io/distroless/cc-debian12`).
- [x] `crates/surch-api/src/main.rs` declares the jemalloc global
  allocator behind `#[cfg(target_os = "linux")]`.
- [x] Dockerfile builder stage gains `build-essential` to compile
  the bundled jemalloc C sources.
- [x] Dockerfile runtime stage sets
  `MALLOC_CONF=background_thread:true,dirty_decay_ms:0,muzzy_decay_ms:0`
  so the async purge thread returns freed pages immediately.
- [x] K8s `ndcg-gate` run `26360701909` on `b9f6636` promoted as
  `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.7-jemalloc-K8s/`.
  Surch RSS peak `5591 -> 3424 MiB` (`-39 %`), Surch RSS final
  `5591 -> 1382 MiB` (`-75 %`), Surch TREC-COVID bulk
  `189 -> 139 s` (`-26 %` allocator bonus). NDCG unchanged.
- [x] Allocator parity with Elasticsearch / OpenSearch
  (which both default to jemalloc on Linux since ~7.13) achieved.

### Lot 1.6 — Deferred term dictionary build (CLOSED)

Trigger: after Lot 1, the cumulative `terms.build()` call inside
`DocumentIndex::add_documents_with_mapping` (rebuilds the whole
FST from `self.postings_builder` after every `_bulk` POST) was the
dominant Surch bulk cost on long-text corpora.

- [x] Confirmed the bottleneck by code read + the run series.
- [x] Implemented in `2e4361e`: `add_documents_with_mapping_deferred`
  sets a `terms_dirty` flag instead of rebuilding the FST;
  `materialize_terms()` rebuilds lazily iff dirty;
  `AppState::ensure_terms_ready` materializes at the 7 search /
  count / lookup / scoring entry points; `finalize_terms_for_refresh`
  materializes once at `_refresh`. `terms_build_count` instrumentation
  + test assert the rebuild count stays ~constant across chunks.
- [x] K8s `ndcg-gate` run `26373579876` on `2e4361e` promoted as
  `docs/ops/bench-reports/2026-05-24-ndcg-gate-lot1.6-K8s/`.
  TREC-COVID Surch bulk `139.05 -> 56.38 s`; **Surch now `1.54x`
  FASTER than OpenSearch** (`86.61 s`). RSS peak `3424 -> 2156 MiB`.
  NDCG unchanged. Total Lot 1→1.6 speedup `~17.8x` (`1002 -> 56 s`).

### Lot 2 — Skip lists on the codec FoR path

Deferred in `PLAN.md` since Track A closure. The codec already
persists per-block stats next to postings (`b680232 / 6df877d`) and
ships per-128 Block-Max WAND (`e38bf91`); skip lists on top of the
encoded block metadata are the next algorithmic layer.

- [x] Skip list format + leapfrog AND landed in `d73c862` (Stream B
  of the parallel dispatch): `crates/surch-codec/src/postings_block.rs`
  (+432 lines), `crates/surch-index/src/postings.rs`,
  `crates/surch-search/src/execution.rs` + tests.
- [x] Codec + search coverage added (`crates/surch-search/tests/execution.rs`).
- [x] Compiles + passes the workspace suite with Lot 1.6 (`ci` run
  `26373423517`). NDCG@10 unchanged on `ndcg-gate` run `26373579876`.
- [x] **Search-latency gain quantified** via a clean paired
  `insee-bench` isolation, promoted as
  `docs/ops/bench-reports/2026-05-25-insee-lot2-skiplists-K8s/`:
  control `b9f6636` (jemalloc, no Lot 2) Surch `1.6/3.9/7.9/68.3 ms`
  vs Lot 2 `d73c862` Surch `1.6/3.4/6.5/64.1 ms` → skip lists improve
  the Surch tail `p95 -13% / p99 -18%`, p50 flat. Both runs GREEN
  (the `bench_report` RSS-SLO fix `e37a864` is on both branches).
  Caveat: single run per SHA; a 3-rep paired run would tighten the
  CI (deferred).
- [x] Side effect: fixed a Track E/B regression — `bench_report`
  RSS SLO now gates Surch only, not the JVM reference engine
  (`e37a864`), so insee-bench no longer fails closed at teardown
  on the OpenSearch >1 GiB heap.

### Lot 3 — Next Block-Max WAND step

Builds on encoded block metadata + Lot 2 skip lists. Goal: extend
the per-128 contribution skip already shipped in `e38bf91` to
exploit the skip list cursors for cross-term skipping in OR-match
top-K and `multi_match`.

- [ ] Specify the next BMW step against the current encoded block
  metadata + skip list cursors; keep parity with the existing
  WAND/`multi_match` test surface.
- [ ] Implement and add SciFact quality guardrail tests before any
  perf claim.
- [ ] Promote a paired K8s perf proof + Track A ledger update for
  Search latency. SciFact NDCG@10 floor `>= 0.65` must hold;
  TREC-COVID NDCG@10 must not regress vs the
  `2026-05-22-ndcg-gate-7Gi-K8s` baseline.

### Lot 4 — Historical A-replay-1/2/3 promotion

This Lot is **delegated** to `plan/perf-replay-wp-a-algo-ledger.md`,
which owns the A-replay-1/2/3 historical proof points (top-K / lazy
hydration, WAND family, memory layout). The known blocker is that
historical SHAs lack the current `docker-build.yml` / `ci-k8s.yml`
surface, so direct workflow dispatch is not yet possible.

- [ ] Track resolution of the workflow-surface blocker via the
  replay ledger plan; do not duplicate replay tracking here.
- [ ] When a replay group closes, the replay ledger plan updates
  the Track A ledger rows; this file ticks Lot 4 in the same
  commit.

## Coordination

- Track B owns the RSS sampling wiring. The first paired
  `surch.bench.rss.v1` envelopes are now live in
  `docs/ops/bench-reports/2026-05-23-ndcg-gate-7Gi-RSS-K8s/`
  (Surch peak `4802 MiB / 7 GiB`, OpenSearch peak
  `1395 MiB / 2 GiB`), so Lots 1 and 3 can cite real paired RSS
  deltas directly rather than relying on `kubectl top`.
- Track E is closed: the `ndcg-gate` K8s harness is the standard
  heavy-run target. No additional infra dependency is expected for
  Lots 1-3 on the current image surface.
