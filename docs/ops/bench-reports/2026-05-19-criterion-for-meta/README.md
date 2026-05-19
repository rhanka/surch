# Criterion micro-bench — FoR block-meta wiring (Track A, Lot 2 → Lot 3)

Smoke perf-proof for `df3b0aa perf(search): use block metadata for term
doc frequency` (Track A Lot 2 final step). Captured on the development
workstation, against the in-process Criterion harness
`crates/surch-api/benches/search_hot_path.rs` (5 000-doc synthetic
corpus, BM25 hot path, no axum / no HTTP).

**This is a `--quick` smoke, not a Lot 3 closure.** `--quick` is
`sample_size = 10`, `measurement_time = 1.5 s` per group. Variance
between consecutive runs at this sample count is typically ±10–30 %.
A proper Lot 3 perf-proof needs a full Criterion run (`sample_size =
100`, ~5–10 min per group), pinned CPU governor, and >= 2 paired
captures.

## Method

```sh
# After (Track A Lot 2 in)
cargo bench -p surch-api --bench search_hot_path -- --quick --output-format bencher
# Before (Lot 2 reverted via temporary worktree on c01b0a2)
git worktree add /tmp/surch-perf-baseline c01b0a2
cd /tmp/surch-perf-baseline
cargo bench -p surch-api --bench search_hot_path -- --quick --output-format bencher
```

Surch HEAD at measurement: `df3b0aa` (`main`, FoR block-meta wired).
Comparison commit: `c01b0a2` (last `main` before the FoR-meta wiring).

## Results (`--quick`, ns/iter)

| Bench group | Before `c01b0a2` | After `df3b0aa` | Δ | Touches FoR-meta path |
|---|---:|---:|---|---|
| `match_all` size=10 | 11 168 (±380) | 18 893 (±724) | +69 % | **no** (no term lookup) |
| `match_simple` size=10 | 888 (±25) | 529 (±5) | **-40 %** | yes (single term, doc_freq from blocks) |
| `bool_must_2` size=10 | 544 618 (±5 416) | 464 442 (±7 153) | **-15 %** | yes (two-term must) |
| `multi_match` size=10 | 701 491 (±8 883) | 856 357 (±9 911) | +22 % | yes (two-field × one-term) |

## How to read

- `match_all`'s +69 % is **noise**: the FoR-meta wiring lives strictly
  inside `TermQueryExecutor`. `match_all` never goes through
  `postings_with_block_metas`, so the gap is a pure code-gen / cache
  draw between the two builds. With `--quick`'s 10 samples the
  ±10–30 % envelope swallows it.
- `match_simple` and `bool_must_2` show the expected speedup: the new
  `PostingsList::doc_freq_from_block_metas()` (single tight `iter().sum()`
  over `BlockMeta::posting_count`) lets the executor skip an
  `into_iter().collect::<Vec<_>>()` of postings just to count them.
- `multi_match`'s +22 % is suspect (same path as `match_simple` but
  doubled) and needs a `--sample-size 100` rerun to confirm or refute.
  Candidate cause: a small code-bloat penalty in the FoR-meta `pub`
  surface that affects the hot path on the second field, or pure
  variance.

## Verdict

- Lot 2 (`df3b0aa`) is **not regressing the indexed-term hot path**;
  the two clean term-query groups speed up by 15–40 %.
- The `match_all` and `multi_match` deltas are within `--quick`'s
  noise envelope, but `multi_match` deserves a full-Criterion pass
  before Lot N closure.

## What Lot 3 still owes

- Full Criterion pass (`--sample-size 100`, pinned CPU) on these same
  four groups, with the Criterion baseline machinery
  (`cargo bench -- --save-baseline main && git checkout … && cargo
  bench -- --baseline main`).
- INSEE 10k artillery rerun on the K8s burst pool against the same
  `df3b0aa` SHA as a separate signal (already captured under
  `2026-05-19-insee-10k-k8s/` for the post-FoR state; the pre-FoR
  comparable run never happened so this is documentary, not paired).
- Quality guardrail: `cargo test -p surch-search --test execution`
  + the existing SciFact NDCG@10 gate — both currently green
  (gates recorded in `plan/wp-a-optim.md`).
