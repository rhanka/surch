# Bug — search during sustained bulk stalls the engine (large index)

**Surfaced 2026-05-27** by the matchID real-corpus eval: bulk-indexing the
1.36M-doc `deces` corpus into Surch while issuing concurrent reads
(`_count`/`_search` polling) wedged the engine — reads **and** writes hung; the
bulk loop froze (`_count` stuck mid-load). A serial bulk with **no** concurrent
reads completed cleanly (1.36M in ~426 s), so it is a concurrency pathology, not
a scale ceiling.

## Root cause (analysis, not yet reproduced in a test)

`AppState.store: Arc<RwLock<MemoryStore>>` (`std::sync::RwLock`, writer-preferring
on Linux/glibc).

1. Each `_bulk` write takes `store.write()` and sets `terms_dirty = true`.
2. **Every search-path method calls `ensure_terms_ready`**
   (`crates/surch-api/src/state.rs:1199,1519,1544,1563,1581,1633,1660,1691`).
   Under `terms_dirty` it takes `store.write()` and runs `materialize_terms()`
   — a **full FST term-dictionary rebuild** (expensive at ~1M terms).
3. During interleaved bulk + search, the bulk re-dirties `terms_dirty` after
   every chunk, so each concurrent search forces another full FST materialize
   that the next chunk immediately invalidates → write/write thrash, each
   materialize multi-second at 1.36M.
4. With a writer-preferring `RwLock`, this collapses throughput; readers and the
   bulk loop stall for the whole bulk window (~minutes) → looks like a hang.

So the engine is effectively single-writer-serialised AND repeatedly rebuilding
the FST whenever a read races a write. Pathological for any production workload
that searches while indexing.

## Reproduction (to write as an in-process test — light, no Docker)

`#[test]` on `AppState`: spawn one thread appending N small batches (dirtying
terms each batch) and another issuing `match`/`_count` reads; a watchdog thread
fails the test if any read exceeds a few hundred ms (or if total time blows up).
Expected today: read latency tracks the FST-rebuild cost / stalls. Keep N small
(a few k docs) — the bug is qualitative, not scale-gated. **Validate via the
cloud `ci` workflow, never a heavy local run** (see memory: no local heavy
workloads — a local 1.36M run crashed the dev box).

## Proposed fix directions (pick after the repro is red)

- **Keep `materialize_terms` off the read hot path.** Build/refresh the FST on
  `refresh_index` / a background task, and let reads serve from the last-good
  FST snapshot while `terms_dirty` (accept staleness until refresh) instead of
  forcing a write-lock rebuild per search.
- Or make terms its own lock so a read-side materialize does not block the
  document `store` writers (decouple FST build from the doc store lock).
- Or incremental FST append instead of full rebuild, so materialize is cheap.
- Consider `parking_lot::RwLock` for fairness (avoids reader starvation), but
  that alone does not remove the per-search full-rebuild cost — fix (1) first.

## Status

Investigation only (no code change yet — a lock/FST change must land with the
red→green in-process repro test, validated in cloud CI). Filed for a focused
follow-up. Eval impact: the matchID indexation number was taken with a **serial,
no-concurrent-read** bulk; concurrent search-while-indexing is currently unsafe
on a large index.
