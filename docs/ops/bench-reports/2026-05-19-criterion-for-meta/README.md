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

## Results (`--sample-size 50`, ns/iter, system noisy)

Same harness, two follow-up passes (`c01b0a2` worktree paired with a
re-run of HEAD after the worktree was cleaned up).

| Bench group | Before `c01b0a2` | After `df3b0aa` (idle re-run) | Δ |
|---|---:|---:|---|
| `match_all` size=10 | 7 433 (±490) | 12 059 (±1 889) | +62 % |
| `match_simple` size=10 | 349 (±62) | 406 (±76) | +16 % |
| `bool_must_2` size=10 | 292 319 (±16 182) | 541 478 (±79 309) | +85 % |
| `multi_match` size=10 | 662 699 (±246 121) | 1 082 130 (±102 165) | +63 % |

The variance bands (±15-37 %) consistently swallow the cross-commit
deltas; the workstation is **not pinned**, no `taskset`, no governor
freeze, and concurrent compile / harness work was running across the
two captures. A first HEAD pass while the BEFORE worktree was still
compiling produced ±50-60 % bands, confirming CPU contention drives
most of the numbers. The Δ direction reverses (`bool_must_2`,
`match_simple`) between `--quick` and `--sample-size 50`, which is the
hallmark of noise dominating signal here.

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

The local Criterion harness on this workstation is **too noisy to
prove or disprove the FoR-meta wiring's effect at micro-bench
granularity**. The 50-sample pass even shows an apparent regression
across all groups including `match_all`, which strictly does not go
through the modified code path — confirming the deltas are CPU
contention / scheduler variance, not real Track A behaviour.

The fiber-grade evidence for Lot 2 sits one layer up, in
`2026-05-19-insee-10k-k8s/`: on a dedicated Scaleway burst pod, with
the same `df3b0aa` HEAD, the 50-RPS / 4-min INSEE 10k artillery
scenario shows **Surch p50/p95/p99 = 1.9 / 3.6 / 6.9 ms vs OS 2.17.1
3.8 / 9.9 / 20.8 ms, 0 errors over 13 170 issued**, all SLOs PASS.
That is the regression signal we actually care about for matchID
50 RPS — and it is well within the historical baseline (`2026-05-16-vs-os-2.17.1/`).

## What Lot 3 still owes

- A Criterion-grade rerun on a **CPU-pinned, governor-frozen** host
  (none of `taskset`, `cset shield`, or `cpupower frequency-set` were
  used here). On such a host, `--sample-size 100` with the Criterion
  baseline machinery (`--save-baseline before` then `--baseline before`)
  produces statistically meaningful deltas. The workstation used
  here cannot.
- Optional: a paired K8s `insee-bench` against `c01b0a2` to mirror
  the `2026-05-19-insee-10k-k8s/` post-FoR capture as a true before /
  after on the burst pool (the burst pod is isolated and reproducible,
  unlike this workstation).
- Quality guardrail: `cargo test -p surch-search --test execution`
  + the existing SciFact NDCG@10 gate — both currently green
  (gates recorded in `plan/wp-a-optim.md`).
