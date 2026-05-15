# Surch performance optimization plan

Date: 2026-05-14

Order is fixed by ratio gain / effort and dependencies. Each item is a
self-contained PR with bench evidence before merge. Hashes refer to
commits on `main`.

## Already shipped

| Tag | Commit | Title | Impact |
|---|---|---|---|
| v2.1 | `1b2e380` | BM25 stats recorded at indexing time | source-tokenization moved off the query path |
| v2.3 | `3157afb` | Top-K with lazy `_source` hydration | 18 k JSON clones removed per `Rue Payenne` |
| v2.6 | `ed76014` | MaxScore-style WAND skipping for OR-Match | `Rue Payenne` ~120 ms → ~30 ms |
| v2.7 | `d778ee1` | Sorted `Vec<(u32, u64)>` for scoring stats | `Rue Payenne` ~30 ms → ~16 ms |

## Plan (ordered)

### 1. Memory / disk — RAM short-term

| ID | Title | Source | Effort | RAM gain | Disk impact |
|---|---|---|---:|---:|---|
| **A** | Drop `postings_builder` clone in `DocumentIndex::add_documents_with_mapping` (consume builder on build) | index-audit | 0.5 d | ~75 MB | none (in-memory only) |
| **B** | Deduplicate document sources between `InMemoryIndex.documents` and `DocumentIndex.documents` | index-audit | 1.5 d | ~24 MB | none |
| **C** | Packed postings: replace `Vec<Posting{doc_id,freq,positions:Vec<u32>}>` with delta + bitpacking buffers (style Lucene FoR/PFOR) using `bitpacking` crate | index-audit + brainstorm | 3 d | ~60–80 MB | unblocks compact disk format later |
| **D** | FST term dictionary (`fst` crate) for shared prefixes (e.g. `rue de la …`) | brainstorm | 2 d | ~5–10 MB | ditto |

A and B are pure refactors. C and D require touching the postings codec and all readers; do them after the search-side extensions land so we can keep regression bench data comparable.

### 2. Search engine extensions (semantics + perf)

| ID | Title | Source | Effort | Perf gain |
|---|---|---|---:|---|
| **W** | Extend WAND/MaxScore to `MultiMatch` (per-field max) and `BoolMust` (sum of clause max contributions) | local | 1–2 d | unlocks matchID multi-field workloads |
| **P5** | Common-Terms split (stopword-style: low-freq MUST, high-freq SHOULD evaluated only if MUST matched) | brainstorm | 2 d | significant on geo/text-heavy queries |
| **P6** | Query token deduplication + boosting (`to be or not to be` → `to^2 be^2 or not`) | brainstorm | 0.5 d | modest, very cheap |
| **P9** | Two-phase iterator for `MatchPhrase` (cheap intersection postings, then position verification) | brainstorm | 2 d | modest on BAN, important if we add phrase-heavy fixtures |

### 3. Caching (latency for warm workloads)

| ID | Title | Source | Effort | Perf gain |
|---|---|---|---:|---|
| **C1** | Per-index LRU search result cache (key = normalized query body hash; invalidated on `_bulk` / document writes) | local | 1 d | dramatic on repeated queries (matchID name lookups, auto-suggest) |
| **C2** | Filter clause result cache (Roaring bitmap of doc ids matching a static filter) | brainstorm | 1.5 d | useful when matchID adds `term`/`range` filters |

### 4. Postings layout (deep perf, RAM and CPU)

| ID | Title | Source | Effort | Perf + RAM gain |
|---|---|---|---:|---|
| **P2** | Roaring bitmaps for dense postings (`roaring` crate); auto-switch when `doc_freq / doc_count > 0.3` | brainstorm | 2–3 d | significant on `MUST`/filter; AND-intersections free |
| **P3** | BM25 8-bit quantized lookup table per term (precompute `score(tf, dl)` over 256 buckets) | brainstorm | 3–4 d | removes `log` and div from hot path |
| **P4** | Block-Max WAND classic (per-block max scores in 128-doc blocks) | brainstorm | 4–5 d | modest on Paris 25k, large on production scale |
| **P7** | Recursive Graph Bisection (BP) doc-id reorder | brainstorm | 3–4 d | enables long zero runs in postings, amplifies P4 |

P4 and P7 are bundled: P4 alone gains modestly, P7 alone gains marginally; together they double down. Sequence them after P2 so the bitmap path is already in.

### 5. Operational / API completeness

| ID | Title | Source | Effort | Notes |
|---|---|---|---:|---|
| **O2** | Default `track_total_hits=10000` cap when not specified, with `relation=gte` reporting (matches OpenSearch ≥7 default) | local | 0.5 d | reduces query work for default OS-style requests |
| **O3** | Artillery-style concurrent bench tool: replay `test-backend-v1.yml` (2→50 RPS, 4 min, 50/50 GET/POST) against Surch and OS, report p50/p95/p99 per phase | local | 1 d | enables apples-to-apples vs matchID SLO (`p95 < 200 ms`) |
| **O1** | Durable persistence: write index + sources to disk, restart-aware reload | brainstorm | 1–2 wk | unblocks production cutover; not needed for the bench story |

### 6. matchID fixture + replay (after the perf wave)

| ID | Title | Source | Effort | Notes |
|---|---|---|---:|---|
| **MID** | INSEE `Deces_2024`+`Deces_2025` ingested in `tests/matchid_compat/datasets/`, `replays/insee_match_critical.json` with the 50/50 GET+POST mix, oracle expected responses captured from OpenSearch | matchID agent | 1.5 d | data already downloaded under `target/insee/`; only conversion + manifest left |

## Execution order

1. **A** (drop builder clone) — pure RAM, 30 min, no risk.
2. **W** (WAND for MultiMatch / BoolMust) — extends the v2.6 win to matchID-style multi-field queries.
3. **P6** (token dedup) — half-day free win.
4. **O2** (track_total_hits default cap) — half-day, matches OS default and saves work.
5. **B** (source dedup) — 1.5 d structural refactor.
6. **C1** (search result cache) — 1 d, big lever for matchID warm workloads.
7. **P5** (Common-Terms split) — 2 d.
8. **P2** (Roaring bitmaps for dense postings) — 2–3 d, enables AND filter speed.
9. **C** (packed postings) + **D** (FST term dict) — 3 d + 2 d, biggest RAM/disk reductions.
10. **P3** (BM25 LUT) — 3–4 d.
11. **P4** + **P7** (Block-Max WAND + BP reorder) — bundled, 7–8 d total.
12. **O3** (artillery bench) — 1 d.
13. **MID** (matchID fixture + replay) — 1.5 d.
14. **O1** (persistence) — multi-week, post-MatchID UAT.

Each step ships its own commit + bench delta in
`docs/poc/reports/ban-api-perf-index-stats-bench-0dc30ad.md`. The
target by the end of step 8 is **Surch beating OpenSearch on every
matchID v1 artillery SLO bucket on the INSEE 25k fixture, while
holding ≤ 60 % of OpenSearch RAM**.
