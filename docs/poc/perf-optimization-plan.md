# Surch performance optimization plan

Updated: 2026-05-14 (R&D agents pass 2)

For each optimization, the 3 axes are tracked separately:

- **VÉLOCITÉ** = expected speedup on `_search` server `took` for the BAN/INSEE matchID workload (high-cardinality `Rue Payenne` / `match NOM` plus medium-cardinality multi_match)
- **RAM** = MB freed on the in-memory Paris 25k working set (raw object lifetime; the OS may or may not return the freed pages to the kernel)
- **DISQUE** = how the change unlocks (or doesn't) a future durable segment format; not measured today (Surch is in-memory), but noted because it sets the codec contract for the persistence milestone

## Already shipped (this branch on `main`)

| Tag | Commit | Title | Vélocité | RAM | Disque |
|---|---|---|---|---|---|
| v2.1 | `1b2e380` | BM25 stats recorded at indexing time | source-tokenization moved off the search path | neutral | neutral |
| v2.3 | `3157afb` | Top-K with lazy `_source` hydration | 18 k JSON clones removed per `Rue Payenne` | neutral (clones are temporary) | neutral |
| v2.6 | `ed76014` | MaxScore-style WAND skipping for OR-Match | `Rue Payenne` ~120 ms → ~30 ms | neutral | neutral |
| v2.7 | `d778ee1` | Sorted `Vec<(u32, u64)>` for scoring stats (brainstorm #1) | `Rue Payenne` ~30 ms → ~16 ms (2–5× on dense tokens) | neutral (same data, denser layout) | neutral |
| v2.8 — A | `65ccfbe` | `finalize_postings()` drops `PostingsBuilder` snapshot | neutral | **~150 MB** of duplicate postings reclaimed by the heap allocator on BAN Paris 25k | neutral |
| v2.8 — W | `65ccfbe` | MaxScore for `MultiMatch` via per-field max | matchID `multi_match NOM+PRENOMS` now goes through WAND (p50 4 ms vs OS 15 ms on INSEE 25k) | neutral | neutral |
| v2.8 — P6 | `8757288` | Deduplicate repeated query tokens with boost | half the posting walks for queries with duplicate analyzed tokens | neutral | neutral |
| v2.9 — B | `651e22a` | `DocumentIndex` keeps `BTreeSet<u32>` `live_docs` (no `StoredDocument` duplicate) | neutral | **~30 MB** observed (RSS 260 → 231 MB on Paris 25k) | unblocks a cleaner future codec (sources are owned only by `InMemoryIndex`) |
| v2.10 — C1 | `644f62b` | Per-index LRU search-response cache (256 entries, byte-cached, generation-invalidated on every mutation) | **dramatic on repeated queries** (cache hit ≈ 0 work, see matchID auto-suggest / dedupe-review-list workflows); neutral on unique-query benches | small (+capacity × avg response bytes; ~256 × ~3 KB ≈ 1 MB per index ceiling) | neutral |
| O3 — artillery harness | _scripted_ | `scripts/bench/artillery-replay.sh` reproduces `test-backend-v1.yml` shape (50/50 `multi_match` vs `bool.must`, phases 2→50 RPS over 4 min scaled by `ARTILLERY_SCALE`) | bench infrastructure only | neutral | neutral |

### SciFact BEIR NDCG@10 parity (BM25 correctness gate)

`scripts/bench/scifact-ndcg.sh` indexes the SciFact corpus (5 183
docs), runs all 300 test queries through `multi_match` over
`title`+`text`, and computes NDCG@10 against the BEIR `qrels/test.tsv`
binary judgments.

| Engine | NDCG@10 | Recall@10 |
| --- | ---: | ---: |
| **Surch v2.11** (after O2 + T2) | **0.6576** | 0.8100 |
| OpenSearch 2.17.1 (default BM25) | 0.6537 | 0.8033 |
| Anserini / Lucene BM25 tuned baseline (BEIR paper) | 0.688 | — |

Surch and OS both sit ~5 % below the Anserini-tuned baseline; the
gap is fully explained by the tuned BM25 parameters (Anserini uses
k1=0.9 / b=0.4 against our default 1.2 / 0.75) and the Porter
English stemmer that neither Surch nor a plain OS BM25 with the
`standard` analyzer applies. Surch is **slightly above** OS on this
fixture, which gives the BM25 implementation a written third-party
correctness gate before we keep optimising.

### Artillery scaled run on INSEE 25k (ARTILLERY_SCALE=0.2, ~50 s/engine)

`p50` client-side latency from the bash+curl harness, paired runs:

| Phase (RPS) | Surch v2.10 p50 | OpenSearch 2.17.1 p50 | Surch p95 | OS p95 |
|---|---:|---:|---:|---:|
| 1 (2) | **174** | 361 | 324 | 796 |
| 2 (2) | **164** | 472 | 247 | 809 |
| 3 (5) | **192** | 357 | 300 | 659 |
| 4 (10) | **227** | 377 | 440 | 788 |
| 5 (20) | **275** | 392 | 709 | 777 |
| 6 (50) | **157** | 175 | 465 | 515 |

Surch wins p50 by 1.4–2.9× on low/mid-RPS phases, comparable at 50 RPS sustained. Both engines have high tails because the harness opens a fresh TCP/HTTP connection per request and runs each backgrounded bash fork at the artillery cadence — the bottleneck is the harness, not the engines. The matchID SLO of `p95 < 200 ms` will need a real keep-alive HTTP client (Rust binary) to be measurable end-to-end; the comparative win is already visible.

## Plan ahead (ordered)

Order is fixed by ratio (gain on the 3 axes) / effort. Each entry cites the
R&D source that recommends it.

### Phase 1 — easy wins still on the table

| ID | Title | Vélocité | RAM | Disque | Effort | Source |
|---|---|---|---|---|---|---|
| **O2** | Default `track_total_hits=10000` cap (matches OS ≥7 default) | reduces work past the cap when the user doesn't ask for an exact total | neutral | neutral | 0.5 d | local |
| **T1** | `TopNComputer` replacement for the post-scoring `select_nth_unstable_by` (small array, conditional insert; +15 % at Tantivy 0.22) | small but cheap | neutral | neutral | 0.5 d | Tantivy agent #2 |
| **T2** | Order clause scoring by ascending `doc_freq` (cheap first, expensive last) inside `BoolMust` paths | 1.2–1.5× on AND multi-term | neutral | neutral | 0.5 d | Tantivy agent #6 |
| **T3** | Saturated-posting → `AllScorer` short-circuit (if a token's `doc_freq ≥ N - K` it stops being a candidate filter, only a scorer contribution) | marginal on BAN, real on stop-word-heavy queries | neutral | neutral | 0.5 d | Tantivy agent #10 |

### Phase 2 — Postings codec rework (the big lever)

This is where most of the remaining RAM, disk and CPU lives. Bundled because the layout decisions are interdependent.

| ID | Title | Vélocité | RAM | Disque | Effort | Source |
|---|---|---|---|---|---|---|
| **C** | Block-128 postings with delta + scalar bitpacking (FoR-style, no SIMD yet) | 1.3–1.6× scan, 5 ns/int decode vs ~30–50 ns now | **−50 to −80 MB** on BAN 25k postings (bits/int ~ ceil(log2 max_delta) instead of 32) | sets the on-disk format (each block = 1 byte selector + payload) | 3 d | Index audit + Tantivy + R&D #1 (FoR) |
| **SK** | Skip list per block (last_doc, block-wand `term_freq_max`, block-wand `doc_len_min`) | 1.5–3× on selective intersections, **enables true Block-Max WAND**; ~2× on `Rue Payenne` once paired with C | small (+8 B/block × ~500 blocks = ~4 KB/term) | required for any incremental `advance(target)` from disk | 1.5 d | Tantivy agent #1, #5 |
| **D** | FST term dictionary (BurntSushi `fst` crate) replacing `BTreeMap<String, …>` for terms | µs lookup (cache-friendly, prefix shared) — neutral on a single exact `match`, **3–5× on prefix/regex queries** | **−5 to −10 MB** on BAN (5 M voies with shared prefixes) | unlocks mmap-able term dictionary | 2 d | R&D #1 (Mihov/Maurel), Tantivy agent #4 |
| **P2** | Roaring bitmaps for dense postings (auto-switch when `doc_freq / doc_count > 0.3`) — also for the doc-id matching set in `BoolMust` | 2–10× on AND/OR intersections (900× WAH for bitmap∩bitmap) | adaptive (often ↘); ArrayContainer vs BitmapContainer crossover at 4096 docs | neutral on disk format directly; codec choice | 2–3 d | Survey agent #3 (ClickHouse), R&D #4 |
| **P3** | Precomputed BM25 lookup table per term (256-bucket quantization on `tf × dl`) | removes `log` + div from the hot loop; ~2× on the scoring portion of the per-doc work | small (+1 KB per query token) | neutral | 3–4 d | Brainstorm agent (Mallia SIGIR21) |
| **EF** | Elias-Fano codec as a per-block alternative when density > 30 % (`rue`, `saint`, etc.) | 25–40 % size win on dense postings, random access still O(1) | extra ~−10 MB on BAN | unlocks Quasi-Succinct Indices | 2 d | R&D #1 (Vigna) |

### Phase 3 — Ranking quality + UX

Doesn't change throughput on artillery numbers, but is the moat vs OS for matchID-style search relevance.

| ID | Title | Vélocité | RAM | Disque | Effort | Source |
|---|---|---|---|---|---|---|
| **Q1** | Phased ranking à la Vespa: first-phase BM25 cheap (top-N=200), second-phase compute (exact-match boost, position, prefix-match, attribute boost) | neutral on `took`, **+NDCG significantly** on ambiguous queries ("rue de paris" vs "paris") | neutral | neutral | 1 d | Survey agent #1 (Vespa) |
| **Q2** | Cascade ranking à la Meilisearch: tuple `(words_matched, typo_count, proximity, bm25, attribute)` lexicographic Ord | neutral on `took`, **+matchID-aligned ordering** (number-exact > BM25 raw) | neutral | neutral | 2 d | Survey agent #2 (Meilisearch) |
| **Q3** | Levenshtein DFA × FST for typo-tolerance (requires **D**) | O(query) typo lookup vs scan(O(vocab)) | small (+DFA build per query) | builds on **D** | 3 d | Survey agent #4 (Meilisearch) — depends on D |

### Phase 4 — Cluster / persistence / operational

| ID | Title | Vélocité | RAM | Disque | Effort | Source |
|---|---|---|---|---|---|---|
| **P4** | Block-Max WAND classic (formal upper bound per block, integrated with `SK`) | extends `SK` gain to disjunction-only queries; 1.3–1.5× over plain WAND | neutral | depends on **C** | bundled with **C/SK** | R&D #1 (Ding/Suel) |
| **VBMW** | Variable Block-Max WAND (block size optimized by min-error DP) | +30–50 % throughput over `BMW` on Gov2 | small DP state | builds on `P4` | 1 d after **P4** | R&D #1 (Mallia SIGIR17) |
| **P7** | Recursive Graph Bisection (BP) doc-id reorder (Lucene 9.10 BPIndexReorderer port) | composes with `P4` to give +47–82 % throughput on AND queries (Lucene wikibigall) | indirect (+cache hit rate) | indirect (better deltas → ~−18 % postings disk) | 3–4 d | R&D #1 (Dhulipala KDD16) |
| **HC** | Hotcache à la Quickwit: separable footer carrying term-dict header + fast-field meta + skip-list tops | ~zero on warm in-memory bench; **<60 ms cold open** if the segment lives on disk/S3 | small (+~10 MB for a 25 k-doc segment) | **mandatory for the persistence milestone** | 2 d | Survey agent #5 (Quickwit) |
| **O3** | Artillery harness (replay `test-backend-v1.yml`, 2→50 RPS, 50/50 GET+POST on INSEE 25k), per-phase percentile reporter | enables apples-to-apples vs matchID SLO (`p95 < 200 ms`) | neutral | neutral | 1 d | matchID agent |
| **MID** | INSEE `Deces_2024`+`Deces_2025` ingestion in `tests/matchid_compat/`, replay manifest with the 50/50 mix, oracle responses captured from OpenSearch | enables the compatibility gate from the README | neutral | neutral | 1.5 d | matchID agent |
| **O1** | Durable persistence (write segments, restart-aware reload) | enables shadow UAT on production-like data | neutral | the disk dimension we have been preparing | 1–2 w | matchid-replacement-readiness.md |

## Execution sequence

1. ✅ A, W, P6, B, sorted-Vec(v2.7) and `C1` are shipped or in flight.
2. **T1 (TopNComputer)** + **T2 (cost-ordered intersections)** — half-day each, no risk, banks small wins.
3. **C (block-128 FoR) + SK (skip list) + P4 (true BMW)** bundle — the big lever. ~5 days.
4. **D (FST)** — unblocks **Q3** typo-tolerance and the on-disk term dictionary.
5. **P2 (Roaring)** for filter clauses (matchID `term` on `SEXE`, `DATE_DECES` range etc.).
6. **Q1 (phased)** + **Q2 (cascade)** for matchID ranking quality.
7. **O3 + MID** to validate everything on the matchID artillery scenario.
8. **VBMW, P7, EF, HC** as further refinement passes.
9. **O1** persistence — multi-week, after the perf wave hardens.

## Target

By the end of Phase 2, on the BAN Paris 25k and INSEE Deces 25k workloads:

- `_search` `took`: **≤ 5 ms p95** on the matchID `match` workload (vs OS ~10 ms now)
- `_bulk` 25 k: **≤ 2 s** consistently (vs OS ~5 s, system permitting)
- RSS: **≤ 150 MB** for the loaded 25 k index (vs ~231 MB today)

By the end of Phase 4: matchID artillery v1 SLOs met (`p95 < 200 ms`, `max < 500 ms`, error < 1 %) on a 1 M-document INSEE Deces index, shadow-UAT-ready against OS 2.17.1 on the same host.
