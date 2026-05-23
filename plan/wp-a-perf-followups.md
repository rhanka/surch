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

- [ ] Reproduce the bulk gap locally with a deterministic
  `scripts/bench/trec-covid-ndcg.sh` invocation against the same
  surch-api image, recording wall-clock per-chunk timing and Surch
  RSS at chunk boundaries.
- [ ] Profile the dominant Surch bulk cost on the long-text /
  large-corpus shape: tokenization, FST insertion, postings build,
  source store, or codec write. Use the existing
  `GET /_surch/stats` plus a flamegraph / cargo-flamegraph run.
- [ ] Decide between an algorithmic fix (e.g. amortise the dominant
  cost) and a documented Surch-side limit (corpus shape boundary).
  Either path must be a forward delivery; the ledger Bulk row must
  reflect the chosen verdict.
- [ ] Re-run K8s `ndcg-gate` on the fix SHA and promote the paired
  report; ledger Bulk row updated to either "Surch within `Nx` of OS
  on TREC-COVID" or "Surch documented limit on long-text corpora at
  `Nx`".
- [ ] Gate: ledger Bulk row no longer flags "Surch ingest scaling
  for large-corpus / long-text shapes is the next target".

### Lot 2 — Skip lists on the codec FoR path

Deferred in `PLAN.md` since Track A closure. The codec already
persists per-block stats next to postings (`b680232 / 6df877d`) and
ships per-128 Block-Max WAND (`e38bf91`); skip lists on top of the
encoded block metadata are the next algorithmic layer.

- [ ] Define the on-disk skip list format for FoR-encoded postings,
  reusing `FOR_BLOCK_SIZE` and the codec block metadata helper
  introduced in `6f56fd2`.
- [ ] Add codec-level coverage (`crates/surch-codec/src`) including
  boundary tests, truncated-tail tests, and a seeded-corpus round
  trip.
- [ ] Wire the skip iterator into the search execution path the same
  way `df3b0aa` wired runtime FoR consumption; gate behind a feature
  flag if a runtime regression risk is detected.
- [ ] Promote a paired K8s perf proof and update the Track A ledger
  Search latency row with the new before/after delta.

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

- Track B owns the RSS sampling wiring (`b9faefe`). The bonus
  `ndcg-gate` replay on `b9faefe` will produce the first
  `surch.bench.rss.v1` envelopes; once promoted, this file's Lots 1
  and 3 can cite real RSS deltas instead of relying on
  `kubectl top`.
- Track E gating: the same `b9faefe` replay closes Track E's
  remaining leaf. No additional infra dependency is expected for
  Lots 1-3 on the current image surface.
