# Beat-Elasticsearch campaign — optimisation log (research paper trace)

**Goal.** Make Surch demonstrably **≥ Elasticsearch 8.6.1 on every benchmark**,
proven without cheating, using the matchID `deces` corpus as the honest
proving ground: first **1.36M (`deaths`)**, then **28M (full prod)**, then other
benchmarks. Surch is the product; matchID is the challenge ground (we do not
optimise matchID itself).

Each optimisation below is one paper-traceable step: **hypothesis → change →
before/after measurement (CI, representative HW) → verdict**.

## No-cheat measurement bar (mandatory for every claim)
- **Engine-to-engine** where possible (Surch vs ES over the wire), not only
  through the matchID Node backend (which is its own bottleneck and confounds
  engine latency).
- **Representative hardware** (NOT the 2-vCPU GitHub runner; the runner caps
  every absolute number and starves the engines). Two environments are in play,
  always recorded per result: (a) **matchID CI = GitHub `ubuntu-latest`
  (2 vCPU)** — runs the deces indexation + engine-to-engine latency; absolutes
  runner-bound, relative Surch-vs-ES clean. (b) **surch ci-k8s = Scaleway burst
  node** for ndcg-gate / trec-covid-latency / oracles. **Burst-node migration
  (2026-05): DEV1-XL (4 vCPU / 12 GiB) → POP2-4C-16G (4 vCPU / 16 GiB).** CPU is
  unchanged (4 vCPU) so cross-run timing/latency comparisons survive the
  migration; RAM rose 12→16 GiB (more headroom for the 7Gi Surch limit, no
  behavioural change). **Same-run head-to-heads (both engines in one pod) are
  node-independent and remain valid regardless.** Engine RSS (e.g. #9's
  2168→907 MiB) is corpus-driven, not node-RAM-driven.
- **Real corpus** (`deces` 1.36M, then 28M), not a 50-query low-cardinality
  replay (the LRU cache gives a misleading "354x" on repeated queries — banned
  as a headline).
- **≥3 reps**, report **cache ON *and* OFF**, and report **all dimensions** so
  losses show as plainly as wins: bulk/indexation time, search p50/p95/p99/max,
  QPS under concurrency, and **RSS** (the in-memory model is the scale risk).

## Latency benchmarks — what we have and what we're adding
1. **OpenSearch 2.17.1, INSEE 10k** (`2026-05-25-F2-insee-3rep-K8s`): Surch
   `2.7–3.1x` faster, cache-independent — a real win.
2. **OpenSearch 2.17.1, TREC-COVID 171k** (`F4` cache-on + `F3-LRU` cache-off):
   the honest revealer — cache-on `354x` is LRU-masked; **cache-off raw engine
   is 1.83x SLOWER than OS at p50**. This is the diagnostic that defines the
   front to win.
3. **matchID `deces` vs ES 8.6.1 — engine-to-engine** (NEW, `surch-eval` CI
   `latency_engine.sh`): replays the real deces-backend query shape
   (`function_score`/`bool` `minimum_should_match`/`match` on PRENOM+NOM)
   **directly** against each engine's `_search`, NO Node backend in the path
   (the artillery-via-backend numbers are confounded by the backend + 2-vCPU
   runner). One engine per isolated matrix job. This is the proper matchID
   latency benchmark; absolutes stay runner-bound but the Surch-vs-ES relative
   is clean.
4. **External authoritative latency benchmark (to adopt):** the Tantivy
   **`search-benchmark-game`** (Lucene vs Tantivy vs PISA vs Bleve on an English
   Wikipedia corpus, standardised query set + latency methodology) — a citable,
   non-matchID, non-overfit cross-engine latency standard. Adding a Surch driver
   to it gives an independent, reproducible latency result the paper can cite
   alongside the matchID and BEIR numbers. (Backlog item — corpus + query set
   are public.)

## Dimensions tracked (where Surch can/can't credibly win)
| Dimension | Honest prior | Why |
|-----------|--------------|-----|
| Tail latency p99/max | **Surch can win** | no JVM/GC pauses |
| Footprint / startup / density | **Surch can win** | ~30 MB binary, no JVM heap |
| Bulk on simple mappings | parity/win shown vs OS 2.17.1 | deferred FST (Lot 1.6) |
| Bulk on rich mappings (deces, 28 fields) | **behind ES** | single-thread + multi-field analysis |
| Search QPS under concurrency | **at risk** | reader/writer lock contention bug |
| **Memory at scale (28M)** | **structural risk** | in-memory vs ES disk + page cache |

## Baseline (pre-campaign, deces 1.36M, matchID CI, 2-vCPU runner — confounded)
- Indexation `_bulk`: ES `116 s` (11 600 docs/s) vs Surch `2125 s` (640 docs/s) → ES ~18x.
  Cause: Surch bulk single-threaded (one write-lock) vs ES multi-thread.
- Artillery via backend: ES median `13.5 s` vs Surch `54.6 s` → ES ~4x (runner-bound, relative only).
- Source: `matchID surch-eval` branch, run `26528627429`.

## Optimisations

### #1 — Parallel bulk document analysis (rayon) — `dd3f528`
- **Hypothesis**: the single write-lock serialises the CPU-heavy per-doc
  analysis; ES parallelises bulk across cores. Moving analysis off-lock and
  running it `par_iter` should scale bulk with cores.
- **Change**: `crates/surch-index/src/document_index.rs` — pure
  `analyze_document` (off-lock, parallel) + serial `merge_analyzed`; documents
  merged in input order → **byte-identical postings (parity-preserving)**.
  Unit tests (`surch-index`) green; cloud `ci` (workspace test/clippy/fmt) green.
- **Measurement (ci-k8s `ndcg-gate`, run `26580620561`, image `sha-9b7e632`)**:
  - Quality **parity preserved** (bit-stable): SciFact NDCG@10 `0.6576`,
    TREC-COVID `0.4750` — identical to pre-change → the refactor is correct.
  - Bulk SciFact: Surch `1.83 s` vs OS `14.85 s` (~8.1x). TREC-COVID: Surch
    `68.4 s` vs OS `131.5 s` (~1.92x). Both within noise of the pre-change
    3-rep medians (SciFact `2.09 s`, TREC-COVID `70.96 s`) → **no clear gain on
    these corpora**, as expected: trec-covid/scifact have ≤2 analysed text
    fields, so per-doc analysis is not the bulk bottleneck there (the serial
    merge / postings build dominates).
  - The parallelisation's payoff is on **rich mappings** (deces: 28 fields,
    `norm` analyzer + `.raw` re-analysis + prefixes) where per-doc analysis
    dominates — measured separately on the matchID `surch-eval` deces CI.
- **Measurement (deces 1.36M, matchID `surch-eval` run `26582048931`,
  parallelised image, 2-vCPU runner)**: bulk **Surch `104.2 s` vs ES `115.9 s`**
  (1 355 728 docs). Baseline (pre-#1, run `26528627429`, same workflow): Surch
  `2125 s` vs ES `116 s`.
  → **The 18x indexation deficit on the rich deces mapping is CLOSED: Surch
  reaches parity and edges ES (104.2 s vs 115.9 s).** The win lands exactly
  where predicted (28 fields, `norm` + `.raw` re-analysis → analysis-bound).
  Quality/parity unaffected (search results unchanged; cloud `ci` oracles green).
- **3-rep median (deces 1.36M, runs `26582048931` / `26583239933` /
  `26583245581`)**:
  - Surch bulk: `104.2 / 100.8 / 106.1 s` → **median `104.2 s`**, range ±3% (tight).
  - ES bulk: `115.9 / 123.2 / 91.5 s` → **median `115.9 s`**, range ±15% (variable).
- **Verdict**: ✅ **first beat-ES milestone.** The 18x indexation deficit on the
  rich deces mapping is **eliminated**: Surch median ~10% ahead of ES **and ~5x
  more consistent** (variance ±3% vs ±15% — supports the no-GC predictability
  thesis). Honest nuance (no-cheat): not a clean "always faster" (ES best run
  91.5 s < Surch best 100.8 s) → claim = **parity / Surch slightly ahead +
  markedly more predictable**. Absolutes are still 2-vCPU-runner-bound (engine
  could be faster on real HW), but the head-to-head is fair (same workflow/runner,
  3 reps). Parity preserved (cloud `ci` oracles green). Search latency unchanged
  → optimisation #2 targets the read path / concurrency.

### #9 — Drop per-posting `Vec<u32>` positions from the in-memory index — `<sha pending>`
- **Hypothesis (from the candidate hunt, highest memory leverage)**: each
  in-memory `Posting` carried `{doc_id, freq, Vec<u32> positions}` (~32 B struct
  + a heap `Vec` + allocator metadata per posting), but **no production path
  reads index positions** — BM25 reads only `freq`; `match_phrase` re-tokenises
  `_source` (`search.rs` `phrase_token_spans`); the persisted codec never wrote
  positions. Positions were computed during analysis only to derive `freq`.
  Memory at scale is the in-memory engine's structural risk vs ES (disk +
  page-cache), so this is the highest-leverage RSS win toward the 28M goal.
- **Change**: `crates/surch-index/src/postings.rs` — `Posting{doc_id:u32,
  freq:u32}` (now `Copy`, 8 B); `freq` computed identically at build
  (`freq_from_positions`: empty→1 else len) so scoring is **bit-identical**.
  Positions dropped at the builder boundary. RSS accounting updated
  (`memory.rs`). **Parity-safe**: `freq`/`doc_id` unchanged, no positional read
  path exists — verified by audit (only non-test reader was the RSS gauge) and
  the full `surch-index` suite (95 tests green, incl. postings/document_index),
  clippy + workspace compile clean.
- **Matches/beats**: Lucene stores positions only when
  `index_options >= positions`; Surch stored them unconditionally — now it
  doesn't (until `index_options` is wired, separate item).
- **Measurement (ci-k8s `ndcg-gate`, run `26603090150`, image `sha-3ccdbc6`,
  paired RSS envelope, TREC-COVID 171k)**:
  - **RSS peak: Surch `907 MiB` vs OpenSearch `1465 MiB`**; final Surch `727`
    vs OS `1465`. Pre-#9 baseline (F2 3-rep median, draft): Surch peak
    `~2168 MiB`. → **Surch RSS peak `2168 → 907 MiB` (−58%)**, flipping the
    memory dimension from **1.48x heavier than ES** to **0.62x of ES peak /
    0.50x of ES final**. TREC-COVID body text is position-dense, so dropping the
    per-posting `Vec<u32>` is exactly where the multi-hundred-MiB lived.
  - Quality **bit-stable** (parity): SciFact NDCG@10 `0.6576`, TREC-COVID
    `0.4750` — identical to every prior run.
  - **No bulk regression** (positions are still computed in analysis to derive
    freq; only storage dropped): TREC-COVID bulk Surch `61.8 s` vs OS `111.9 s`
    (1.81x), SciFact `1.68 s` vs `13.6 s` — in-noise vs pre-#9, if anything a
    touch faster (less allocator pressure).
- **Verdict**: ✅ **second beat-ES milestone — and on the structural-risk
  dimension (memory at scale, the 28M juge-de-paix).** Surch now uses **less RAM
  than Elasticsearch** on the 171k corpus while preserving bit-stable quality and
  bulk speed. Honest nuance (no-cheat): single ndcg-gate run (RSS historically
  ±0.5% across reps, and the 907-vs-2168 / 907-vs-1465 margins dwarf that — the
  verdict is robust); OS RSS `1465 ≈ 1467` matches the prior baseline, confirming
  the measurement is comparable. Next: this de-risks the 28M memory question;
  confirm it holds at 28M, and pursue #6 (search-latency idf hoist) for the
  remaining unconquered front.

### #6 — Hoist BM25 idf + config validation out of the per-doc scoring loop — `<sha pending>`
- **Hypothesis (highest search-latency leverage)**: the WAND/MaxScore hot path
  (`maxscore_match`) called `bm25_score` per token, **per 128-block, and per
  scored doc** — each re-running `Bm25Config::new` (4 validation branches),
  corpus-stat validation, and a transcendental `ln()` idf, all **constant for a
  given term**. Hoisting them matches Lucene's per-term-cached `BM25Scorer.idf`.
- **Change**: `crates/surch-search/src/scoring.rs` — new `Bm25TermScorer`
  precomputes `{idf, k1, b, avg_doc_len}` once (`new`, fallible, same errors as
  before) and exposes a branch-free, `ln()`-free `score(tf, doc_len)`.
  `bm25_score` is refactored to a thin wrapper over the **same** kernel → one
  shared float expression. `maxscore_match` (`crates/surch-api/src/search.rs`)
  builds the scorer once per token and uses it at all three sites (token
  max-contrib, block bounds, per-doc). **Parity: bit-identical** — same float
  association, idf computed identically once; the only dropped work
  (`validate_score_inputs` per call) can never fire at these sites (matched
  postings have tf≥1, doc_len guarded ≥1). Unit: 60 surch-search tests green
  (incl. BM25), clippy + fmt clean.
- **Matches**: Lucene per-term-cached idf + length-norm table.
- **Measurement**: PENDING — the gain is on the COMPUTE path, which the LRU
  result cache masks at steady state, so it must be read **cache-OFF**
  (`trec-covid-latency` with `SURCH_DISABLE_SEARCH_CACHE` on `perf-isolation`
  rebased on this main), compared to the F3-LRU cache-off baseline (p50 309 ms /
  p99 624 ms). Plus b2-oracle (parity vs ES 8.6.1) + cache-on (no regression).
- **Measurement (ci-k8s `trec-covid-latency` cache-OFF, run `26605266605`,
  `perf-isolation` `05c7fce` = main+#6 with `SURCH_DISABLE_SEARCH_CACHE=1`)**:
  Surch global **p50 302.1 / p95 528.4 / p99 639.8 / max 918.5 ms** vs the
  F3-LRU cache-off baseline **p50 309.1 / p95 532.3 / p99 623.8 / max 913.8 ms**.
  → **Neutral (within run-to-run noise)** — the OS arm itself moved 169→184 ms
  p50 across runs (~±10% runner noise), so the 302-vs-309 Surch delta is noise.
  Retrieval parity held (hits probe 7 507 757, 50/50 non-zero).
- **Verdict**: parity-safe ✅, **but no measurable latency win on TREC-COVID
  cache-off.** Honest reading: the BM25 idf hoist removes a real per-doc `ln()`
  + branches, but the ~300 ms cache-off p50 is dominated by **postings decode +
  `_source` hydration / candidate resolution**, not BM25 arithmetic (exactly the
  verifier's "bounded — decode + doc_len lookup dominate" caveat). Kept on main
  (correct, zero-risk, helps BM25-bound high-QPS workloads with many cheap
  terms), but it is not the read-path bottleneck here. **Campaign signal: the
  read-path bottleneck is posting-list decode/copy + hydration → prioritise #7
  (zero-copy postings) and the hydration path over further scoring micro-opts.**

### #2 — Stop cloning the field name per token in term aggregation — `<sha pending>`
- **Hypothesis (highest indexing leverage, S effort)**: `analyzed_terms` /
  `subfield_terms` keyed the per-doc term map on `(field.to_owned(), term)` —
  cloning the field `String` on **every token** (O(tokens), even repeated tokens
  that collapse to one entry) and carrying field bytes through every BTreeMap
  key comparison.
- **Change**: `crates/surch-index/src/document_index.rs` — key the per-doc map
  on the **term only** (`BTreeMap<String, Vec<u32>>`); the caller attaches the
  constant field/path once per **unique term** when emitting postings. Field
  `String` allocations drop O(tokens)→O(unique-terms); key comparisons no longer
  carry field bytes. Matches Lucene's single `FieldInvertState`.
  **Parity-preserving**: identical `(field, term, positions)` postings — 95
  surch-index tests green (exact freq/postings/prefixes asserted), clippy + fmt
  clean.
- **Measurement**: folded into the next deces indexation 3-rep (the win is on
  many-token / edge_ngram fields like the deces `autocomplete` sub-field;
  expected single-digit-% bulk, likely within the ±3% deces-run noise — reported
  as an allocation-reduction that compounds with #1, not a standalone headline).
- **Verdict**: parity-safe ✅; magnitude pending the next deces 3-rep.

### #7+#8 — Single scoped read guard + zero-copy borrowed postings — `f66519b`
- **Hypothesis (the read-path bottleneck #6 revealed)**: the search path took
  `~2N+` `store.read()` acquisitions per query (each `term_scoring_stats` also
  re-ran `ensure_terms_ready`) and **deep-copied every posting list** into an
  owned `Vec<(u32,u64)>` + `block_metas.to_vec()` per token. The ~300 ms
  cache-off latency is dominated by this decode/copy + lock churn, not BM25.
- **Change**: `crates/surch-api/src/{state,search}.rs`,
  `crates/surch-search/src/maxscore.rs` — `ensure_terms_ready` runs ONCE up
  front (may write-lock), then **one** scoped read guard (`with_search_reader`)
  is threaded through candidate resolution, scoring, and `_source` hydration;
  term stats are a **zero-copy `TermScoringView`** borrowing `&[Posting]` (now
  `Copy {doc_id,freq}` after #9) + `&[BlockMeta]` directly from the live index —
  no per-query posting copy, no second lock. Matches Lucene's one
  `IndexSearcher`/`LeafReader` per query. **Deadlock-safe** (verified:
  `ensure_terms_ready` strictly precedes the read guard; `std::RwLock` is
  non-reentrant/writer-preferring).
- **Parity**: bit-stable — full workspace suite green on cloud `ci` (oracles,
  `bulk_router_*`, scoring, surch-index/search), clippy + fmt clean. (Recovered
  and re-validated from a parallel worktree agent after a session crash.)
- **Matches/beats**: Lucene single-reader-per-query + streamed `PostingsEnum`
  (no per-query list copy).
- **Measurement**: PENDING — `trec-covid-latency` **cache-OFF** (the compute
  path #6 left untouched): does removing the per-query posting copy + lock churn
  close the raw-engine gap vs OpenSearch (Surch 309 ms → ? vs OS 169 ms p50)?
  Plus cache-on no-regression + b2-oracle parity.
- **Verdict**: parity-safe ✅ (merged); latency delta pending the cache-off run.

## Backlog (ordered by leverage)
0z. **[RESOLVED by #12 — the lever was per-QUERY setup, NOT the per-doc loop.]**
   The #11 note guessed the deces floor was a per-DOC constant-factor battle
   (SIMD FoR decode / branch-lean BM25). **#12 proved that wrong with data**:
   in-memory postings are already decoded (`&[Posting]`), and the dominant cost
   was per-QUERY O(n) setup — a `BTreeMap` doc_len map copied + pointer-chased
   per query, and a `BTreeSet` built from already-sorted postings for the
   candidate set. Replacing them with a dense `Vec<u64>` doc_len (O(1) borrowed,
   incremental `min_doc_len`) + single-token direct candidate `Vec` took deces
   `match`/`bool`/`full` from 40/74/75 ms to **5.3/6.8/7.1 ms** (full p50 ~70 →
   ~7 ms, ~10×; gap vs ES ~17× → ~1.5×). **Corollary: the WP-A codec backlog
   (Roaring, Elias-Fano, BM25 8-bit LUT, recursive-graph-bisection reorder) is
   the WRONG target for deces LATENCY** — it optimises decode/score throughput,
   which was never the bottleneck; it remains relevant for MEMORY/codec size and
   on-disk snapshots, but is deprioritised for the latency goal. Remaining deces
   latency is near the engine-to-engine floor (candidate resolution + small-set
   scoring + hydration + HTTP/JSON + 2-vCPU runner); the "2× faster than ES" bar
   may not be reachable on this probe without result caching (excluded by the
   no-cheat bar), so honest near-parity is the expected landing.
0. **[DONE — `8aae6a1`] Dense-int-docid candidate intersection** — resolved the
   deces residual from `BTreeSet<String>` of public `_id`s to internal `u32`
   doc-ids; public ids resolved only for the final window. Measured 87 → 70 ms
   p50, p95 ÷2. (= backlog #10 from the hunt.)
0a. **[DONE — `a6fa7aa`, NEUTRAL on deces] Leapfrog/galloping conjunction.**
   Lucene `ConjunctionScorer` over the FoR block skip-lists (held-cursor leapfrog
   + materialised fallback), engaged on pure single-term conjunctions. Parity-safe
   (cargo test + ndcg-gate green) but the decomposition showed the conjunction was
   not the deces bottleneck → latency-neutral. Kept (sound for selective
   conjunctions); see the #11 section for the honest write-up. Supersedes the
   "leapfrog is the next lever" note — it was tried and the real lever is 0z above.
0b. **[BACKLOG — KEPT] Block-max WAND for bool `should` true disjunctions**
   (`minimum_should_match < n_should`), reusing `MaxScoreExecutor` for the
   `msm == 1` pure-disjunction case (each single-term `should` = one
   `MaxScoreToken`). **Explicitly retained even though it does NOT help the
   matchID deces workload** (deces is `msm == n_should`, a conjunction handled by
   the should-intersection optimisation). It is a general engine win — any
   disjunctive `bool` query benefits — and Lucene/ES apply block-max WAND here.
   Parity caveat: a `function_score` with functions can produce scores ≤ 0, so
   the WAND upper-bound must fall back to exhaustive in that case (mirrors the
   ES limitation, issue #55222).
1. **Fix the reader/writer concurrency wedge** (search during sustained bulk
   hangs) — table stakes for a production engine + unlocks honest QPS numbers.
   See `docs/wp-a-perf-followups-concurrent-bulk-search-stall.md`.
2. **Engine-level deces benchmark harness** (Surch vs ES direct, representative
   HW) — to actually know where we stand on deces, not via the Node backend.
3. **Reduce rich-mapping analysis cost** (normalise-once for parent + `.raw`).
4. **Prove the tail-latency advantage** (no-GC) cleanly on deces.
5. **Confront memory at scale** (can Surch hold 28M, at what RSS vs ES?).

## Optimisation backlog (candidate hunt)

Adversarially code-verified candidates from the post-#1 hunt. Each was checked
against the actual source (every `file:line` confirmed) for: *is the cost real*,
*is it general* (not deces/matchID-overfit), *is it parity-safe* (bit-stable
results vs ES 8.6.1), and *is the claimed gain plausible*. Numbered as future
optimisations and ordered within each dimension by **leverage = expected_gain ×
confidence ÷ effort**. Magnitudes below are the *honest, de-hyped* figures (the
verifier knocked down several inflated headline numbers — recorded as-corrected).
Effort: S ≈ ½ day, M ≈ 1–3 days, L ≈ a week+. Parity-risk `none/low` = bit-stable
by construction; `flagged` = the candidate *as written* changes results and must
be narrowed before adoption (the safe subset is given).

### Indexing (bulk / analysis)

**#2 — Stop cloning the field name per token in term aggregation.** `S` ·
parity none · conf 0.72 · *highest indexing leverage (S effort).*
`crates/surch-index/src/document_index.rs:620-631,660-668,679-686`.
*Change:* key the per-doc term map on the term `String` only and attach the
constant field name once per unique term, instead of `entry((field.to_owned(),
term))` per token. *Gain vs ES:* removes O(tokens)→O(unique-terms) field-`String`
allocs and drops field bytes from every BTreeMap key comparison; realistic
single-digit-% bulk win, larger on edge_ngram fan-out. *Matches:* Lucene's single
`FieldInvertState` — field identity never re-materialised per token. *General:*
fires on every multi-token text/keyword/edge_ngram field, any corpus.

**#3 — ASCII fast-path for token lowercasing.** `S` · parity none · conf 0.66.
`crates/surch-analysis/src/lib.rs:36,105,115,179,189,446` +
`crates/surch-index/src/mapping.rs:555` (the cited `lib.rs:555` is a test; real
site is the resolved edge_ngram chain in `mapping.rs`).
*Change:* `if s.is_ascii() { s.to_ascii_lowercase() } else { s.to_lowercase() }`
(no `unsafe` — crate is `#![forbid(unsafe_code)]`), skipping the Unicode
SpecialCasing scan on ASCII tokens. *Gain vs ES:* cuts the Unicode-table cost on
the common ASCII token (saves the *scan*, not the alloc — `to_ascii_lowercase`
still allocates one equal-length String); modest but reliable, helps query-time
too. *Matches:* Lucene's JIT-specialised `LowerCaseFilter` ASCII path. *General:*
any ASCII-dominant corpus (French names, English, identifiers, most BEIR);
non-ASCII falls back unchanged → bit-identical.

**#4 — Eliminate the `clone().build()` deep clone on the FST materialize path.**
`M` · parity none · conf 0.78.
`crates/surch-index/src/document_index.rs:255,280` (`postings_builder.clone()
.build()`), build consumes by value at `crates/surch-index/src/postings.rs:128`.
*Change:* make `build(&self) -> TermDictionary` borrow instead of consume, so no
full deep copy of the field/term/postings tree is made and thrown away each
materialize. *Must borrow, not `mem::take`/consume* — the `ensure_terms_ready`
path needs the builder live for the next incremental append (`state.rs:856`).
*Gain vs ES:* removes one O(total_postings) heap-clone pass per materialize and a
transient RSS-doubling spike (clone coexists with original) — an RSS win vs ES,
which never duplicates in-flight postings. (Verifier: "cuts materialize in half"
is *optimistic*; sort + FST build + block_metas remain — the clone is one of
several passes.) *Matches:* Lucene flushes from the live buffer without cloning
it. *General:* every refresh/first-search after any bulk window, any corpus.

**#5 — Parallel per-shard postings build (DWPT-style), merged at materialize.**
`L` · parity low · conf 0.7.
`crates/surch-index/src/document_index.rs:233-258,432-460` (rayon stops at
analysis; the merge `for document in analyzed { merge_analyzed }` and `build()`
are serial), `crates/surch-index/src/postings.rs:101-176`.
*Change:* shard docs into N≈cores partitions, build N independent
`PostingsBuilder`s in parallel (no shared lock), then k-way-merge the sorted
per-field `BTreeMap<term, Vec<Posting>>` streams (terms already lex-ordered,
postings already doc-id-ascending within a shard) into one logical
`TermDictionary` — read path unchanged. *Gain vs ES:* parallelises the
*post-analysis* serial bottleneck (the part rayon #1 left serial), which is what
text-heavy/low-field mappings (trec-covid) hit. (Verifier: headline "3-6×" is
**Amdahl-capped** — analysis is already parallel, so realistic *total-bulk* gain
≈ 1.3–2×.) *Matches/beats:* Lucene's DocumentsWriterPerThread segments + merge,
collapsed to one segment. *General:* targets text-heavy/low-field corpora — the
*opposite* of deces tuning (deces = many short keyword-ish fields).

### Search latency

**#6 — Hoist BM25 idf + config validation out of the per-doc scoring loop.** `M`
· parity low · conf 0.78 · *highest search-latency leverage.*
`crates/surch-search/src/scoring.rs:84-102` (per-call `Bm25Config::new` 4-branch
validate + a transcendental `.ln()` in `bm25_idf`); hot callers
`crates/surch-api/src/search.rs:2085,2041,2614`.
*Change:* a per-term `Bm25TermScorer { idf, k1, b, avg_doc_len }` computed once
per token; the per-doc kernel becomes branch-free, `ln()`-free. *Parity caveat:*
must keep `k1*(1-b + b*lnorm)` **factored** (left-associated) to stay
bit-identical — do *not* collapse to a single `k1*(1-b)` constant (changes float
association). ES oracle tolerance is ~1e-3 so even ULP drift is safe, but aim
bit-stable. *Gain vs ES:* removes one `ln()` + ~6 branches per scored doc and per
128-block bound; meaningful on high-df terms over large corpora, bounded (postings
decode + doc_len lookup still dominate). *Matches:* Lucene's per-term-cached
`BM25Scorer.idf` + length-norm table. *General:* core path for all
match/multi_match/bool/fuzzy scoring.

**#7 — Stop deep-copying the posting list + block_metas per query in
`term_scoring_stats`.** `M`–`L` · parity low (bit-stable by value) · conf 0.72.
`crates/surch-api/src/state.rs:454-505` (fresh `Vec<(u32,u64)>` copy of the whole
list + `block_metas.to_vec()` per query per distinct token, with a `u32→u64`
freq widen); consumed at `crates/surch-api/src/search.rs:2068,2489-2496`;
`crates/surch-search/src/maxscore.rs:55`.
*Change:* `block_metas` is trivially zero-copy-borrowable today — do that half
now (pure win). For the posting list, the literal "borrow `&[Posting]`
zero-copy" is **not possible**: stored `Posting{doc_id,freq:u32,positions:Vec}`
is layout-incompatible with the executor's required `&[(u32,u64)]`. Real routes:
(a) hold a read guard / `Arc<segment>` across scoring and feed a borrowed SoA
(gated on the segment snapshot, #14), or (b) store a `(doc_id,freq)` SoA once at
build time and borrow it (permanent extra index RAM). Keep freq `u32`. *Gain vs
ES:* removes an O(doc_freq) malloc+copy+widen per head term from the hot path; a
real allocator-pressure / p99 / RSS-jitter win on frequent terms (verifier:
"multi-×" overstated — block-skip already prunes; the BM25 loop is the residual
floor; the results byte-cache already absorbs exact repeats). *Matches:* Lucene's
streamed `PostingsEnum`/`ImpactsEnum` — zero per-query list copy. *General:*
per-distinct-token cost ∝ doc_freq, any scoring query, any corpus.

**#8 — Collapse the per-query lock-acquisition storm into one scoped reader.**
`M` · parity none · conf 0.68.
`crates/surch-api/src/search.rs:2471-2496,1781-1892`; the ~2N+ per-query
`store.read()` sites at `crates/surch-api/src/state.rs:1573,1591,1619,1630`
(each `term_scoring_stats` also re-runs `ensure_terms_ready`) + `index_mapping`
at `1161`.
*Change:* hoist `ensure_terms_ready` once up front (it may need the *write* lock
to materialize — must run *before* the long read guard or it deadlocks
`std::RwLock`), then take one read guard / `&IndexData` and thread it through
candidate resolution, scoring-context build, and hydration. Cuts O(tokens)+~4
acquisitions → 1. *Gain vs ES:* a single point-in-time snapshot also removes the
mid-query wedge windows where a bulk writer slips in (p95/p99/max win **under
concurrent bulk**; near-neutral on read-only benches — magnitude unproven, gated
on the concurrent harness). *Matches:* Lucene's one `IndexSearcher`/`LeafReader`
per query reading all stats from one consistent reader. *General:* structural
property of the scoring read path, any corpus.

### Memory (RSS)

**#9 — Drop the per-posting `Vec<u32>` positions from the in-memory index.** `M`
· parity low · conf 0.82 · *highest memory leverage.*
`crates/surch-index/src/postings.rs:19-24` (`Posting{doc_id,freq,positions}`);
sole non-test reader is the RSS accounting at `crates/surch-index/src/memory.rs:157`.
*Change:* shrink `Posting` to `{doc_id:u32, freq:u32}` (8 B, `Copy`). Positions
are still computed during analysis to derive `freq` + doc_len, then discarded —
**no production path reads them** (BM25 reads only freq; `match_phrase`
re-tokenises `_source`; the persisted codec already excludes positions). *Gain vs
ES:* ~32 B→8 B per posting **plus one eliminated heap alloc/Vec header per
posting** (≈40–44 B + allocator metadata); plausibly multiple GiB on a
multi-field 28M-doc corpus. (Verifier: candidate's "48 B" struct figure was
wrong — real size is 32 B; conclusion stands.) *Matches/beats:* Lucene stores
positions only when `index_options ≥ positions`; Surch stores them
unconditionally today. *General:* every field/analyzer pays it now. *Note:* the
SoA-split variant (separate `doc_ids`/`freqs` columns, positions opt-in via
`index_options` plumbing) is the cache-dense superset of this — defer to it only
once `index_options` is wired; the simple drop captures the RSS win immediately.

**#10 — Collapse the three id side-tables into shared `Arc<str>` + a dense Vec.**
`L` · parity low (medium if mis-implemented) · conf 0.8.
`crates/surch-api/src/state.rs:55-57` (`documents` key, `document_ids` key,
`reverse_document_ids` value — the public `_id` String is heap-allocated **three
times** per doc, plus three B-tree node sets).
*Change:* store the id once as `Arc<str>` shared between `documents` and
`document_ids`; replace `reverse_document_ids` with a `Vec<Option<Arc<str>>>`
indexed by dense `doc_id` (O(1) internal→public; `Option` because deletes leave
holes — `next_doc_id` never recycles). Keep a `BTreeMap<Arc<str>,u32>` for
public→internal to **preserve `documents_paginated`'s lexicographic `_id`
order** (`state.rs:1467`) → bit-stable. *Gain vs ES:* removes 2 of 3 id-String
copies + one whole BTreeMap; hundreds of MB→>1 GB at 28M docs; small O(log N)→O(1)
hydration win on top-K. (Honest: invisible to the `stored_fields_bytes` gauge —
measure via process RSS.) *Matches:* Lucene dense int docids + `_id` stored once.
*General:* per-doc id bookkeeping, every index.

**#11 — Store `_source` as a compressed blob with lazy parse.** `L` · parity low
· conf 0.72.
`crates/surch-api/src/state.rs:55` (`documents: BTreeMap<String, Arc<Value>>` —
fully-parsed `serde_json::Value` tree; the team's own comment names this the
"main driver of the matchID INSEE RAM footprint"); bloat model at
`crates/surch-index/src/memory.rs:119-135`.
*Change:* hold `Arc<CompressedSource>` (canonical JSON bytes, LZ4/zstd-1 or the
already-present `flate2`), decompress+parse lazily behind the
`documents()`/`documents_for_ids()` accessors with a small per-query parsed-Value
LRU. *Gain vs ES:* ~3–6× on the `_source` component (eliminates parsed-tree node
bloat *and* compresses text). Honest scoping: `_source` is 1 of 6 RSS components
— *not* always the largest (postings can dominate text-heavy corpora). *Watch:*
the full-corpus-scan fallback (`search.rs:2151`) would decompress every doc —
bounded for posting-backed query shapes (Term/Match/MultiMatch/Bool) but a
latency landmine for non-posting filters; per-hit parse adds retrieval CPU.
*Matches:* Lucene `Lucene90CompressingStoredFieldsFormat` (LZ4/DEFLATE chunks,
decompress on demand). *General:* generic in-memory-engine choice, any JSON corpus.

### Concurrency / QPS

**#12 — Hold one read lock per query** — *same change as #8, scored under the
concurrency dimension* (`conf 0.7`, `L`). The lock-storm collapse is the read-path
half; pairs with the FST-snapshot decoupling (#14). Listed once; do not
double-count effort.

**#13 — Split the global lock into per-index locks.** `M`–`L` · parity none ·
conf 0.6.
`crates/surch-api/src/state.rs:20` (`store: Arc<RwLock<MemoryStore>>` guards
*every* index); a bulk on index A holds the write lock across the whole batch
(`apply_document_writes:1012-1122`, incl. the rayon-parallel rebuild) so a search
on unrelated index B stalls behind it.
*Change:* `DashMap`/outer lock over `Arc<RwLock<InMemoryIndex>>` handles; aliases/
templates to their own small lock. Pure locking-boundary refactor → bit-stable.
*Gain vs ES:* multi-tenant/multi-index QPS scales with cores instead of
serialising. **Honest caveat: zero benefit on the primary single-index deces
benchmark** — bulk+search hit the same lock; only wins under concurrent
multi-index traffic. *Matches:* ES per-shard `IndexWriter` locking. *General:* the
ES isolation model, not matchID-specific (but off the primary bench).

### Highest-leverage structural bet

The candidate hunt surfaced — and the verifier **killed on parity** — the most
tempting structural move: **serve searches from a committed FST snapshot
(`ArcSwap<TermDictionary>`) and build the term dictionary only on `_refresh` / a
background materializer**, taking `materialize_terms()` (the
`postings_builder.clone().build()` full FST rebuild) off the read path entirely
(`crates/surch-api/src/state.rs:835-858`, `document_index.rs:276-283`). The cost
it targets is real, general, and severe — under interleaved bulk+search every
search forces a full O(total-terms) rebuild that the next chunk re-dirties and
discards, collapsing both reads and the bulk loop (the documented wedge). **But
as written it is *not* parity-safe in this codebase**: Surch's enforced contract
is *read-your-writes* (test `bulk_router_makes_batched_documents_searchable`
searches immediately after `_bulk` with **no** `_refresh` and asserts the docs are
visible). A snapshot-on-refresh model returns stale/zero results for those
searches — an observable result change, not a perf no-op. The genuinely
adoptable structural bet is therefore the **parity-preserving sibling: give the
term dictionary its own lock and decouple the FST build from the doc-store writer
lock while *still* materializing on the read path** (kills the write/write thrash
without changing visibility), and, longer term, a **per-index `Arc<segment>`
copy-on-write snapshot** (#13 + #14) that makes readers lock-free. That path —
not the staleness route — is the way to ES-grade NRT concurrency without
sacrificing the bit-stable parity the whole campaign rests on. It also unblocks
the clean (borrow-not-copy) forms of #4 and #7.

### Considered & dropped (verified, rejected — honesty trail)

- **Bulk grouped appends into `PostingsBuilder`** — *not real*: the claimed
  per-token field-`String` clone doesn't exist (the merge **moves** the owned
  String), and the BTreeMap level it removes is the cheap ~2-entry outer map, not
  the costly inner term descent.
- **Skip the build()-time posting re-sort when already doc-id-ordered** — *gain
  not plausible*: Rust 1.93 driftsort already early-returns O(n) on sorted input,
  so the saving is a key-extraction scan, not the claimed n·log n; near-noise vs
  FST build.
- **SIMD bit-packed FoR codec to replace LEB128 varint** — *not real (dead
  code)*: the FoR codec is unwired scaffolding (docs + Cargo comments confirm
  "NOT wired into the postings hot path"); zero production callers, so ~0
  end-to-end search gain.
- **Precompute the `BlockSkipList` at index build instead of per query** — *gain
  not plausible*: the skip list is ~1% of the per-token allocation that
  `term_scoring_stats` already does (and the cited "per-skip cursor allocation"
  doesn't exist — `BlockSkipCursor` is stack-only).
- **Collapse the 3 per-term FST walks / hoist the field lookup** — *gain not
  plausible*: the cited hot-path sites already use the combined single-walk
  accessor, the executor module isn't wired into the API search path, and
  per-request term dedup already exists.
- **Stream FST range scans for prefix (avoid `BTreeSet`/`Vec<String>`)** — *not
  general + gain overstated*: the `sorted_terms` half is off the hot path
  (tests/admin only); the `BTreeSet` half is the narrow matchID-shaped
  non-`index_prefixes` date/year path, tiny for selective prefixes.
- **Replace full FST rebuild with per-batch immutable FST segments** — *not real
  anymore*: the O(K²) per-chunk rebuild it targets was already removed by the
  Lot 1.6 deferred build (one terminal `build()` per load, asserted in tests).
- **Eliminate the duplicate live `PostingsBuilder` across the bulk path** —
  *parity-unsafe + regressing*: consuming the builder loses the source-of-truth
  for incremental append (data loss); periodic finalize trips the full-rebuild
  guard and resurrects the O(n²) cost Lot 1.6 killed.
- **Index analysis+merge off the write lock, swap result under lock** — *not
  general + parity-unsafe*: the expensive FST build is already deferred off this
  path; doc-id allocation runs inside the same write section, so dropping the
  lock mid-batch opens a parity-breaking reorder window; no harness measures the
  concurrent benefit.
- **Immutable in-memory segments + copy-on-write readers (full Lucene NRT)** —
  *parity-unsafe + gain pre-captured*: per-segment IDF would diverge from the
  global-stats BM25 path (the candidate is silent on cross-segment stat
  aggregation), the disk-codec reuse claim is false, and Lot 1.6 already captured
  the headline indexing win. (The *parity-preserving* slice of this idea is the
  structural bet above.)

#### Parity-flagged (real + general, but the change as written changes results — adopt only the narrowed safe subset)

- **Analyze the query string once per (field,value)** (`search.rs:2566`,
  `1957`, `2582`; `state.rs:519`) — redundant 3–4× tokenisation is real, but
  candidate-resolution and scoring use **different analyzers** (search_analyzer
  vs builtin); merging them drifts ranking. *Safe subset:* dedupe only the two
  *scoring* calls (identical builtin analyzer) and cache per-token **counts**
  (not the order-lost `BTreeSet`, which would drop repeated-token boosts).
  Effort M, modest gain.
- **Normalise-once for parent text + `.raw`/`.norm` subfield**
  (`document_index.rs:534,563-577,649-670`; `surch-analysis/src/lib.rs:415-454`)
  — duplicate fold+lowercase passes are real. *Safe subset:* fuse lowercase+
  asciifold into one per-token pass and drop the redundant `Vec<Token>` rebuild
  (bit-stable). *Reject as written:* fold-then-tokenise changes token offsets
  (asciifold changes byte length: ß→ss, Œ→OE) — `_analyze` offsets are an
  ES-parity surface. Effort M.
- **Fold the edge_ngram base token once and slice** (`mapping.rs:549-562`;
  `lib.rs:341-371`) — the per-gram re-fold is genuine O(L²) at the production
  `max_gram=20`. *Safe only via a rolling-fold* (append each new char's fold to
  the prior gram's buffer) keeping Unicode-correct `to_lowercase`; a naive
  pre-folded byte-slice breaks parity. Gate on the existing
  `edge_ngram_subfield_fans_out_prefix_postings` test + a ß/Æ case. Effort M.
- **Postings-driven candidate gathering for Range/Exists/Wildcard** (kills the
  O(N) `_source` full-scan fallback, `search.rs:2174-2178`) — the full-scan is
  real and general. *But:* the Prefix half is **already done**; **numeric Range**
  cannot use a lexicographic FST scan (`'100' < '9'`) — needs a points/BKD index
  not proposed; keyword scan paths normalise (lowercase+fold) so raw FST terms
  need re-normalisation. *Safe subset:* Exists + keyword/date Range +
  literal-prefix Wildcard only, with explicit normalisation and a numeric carve-out.
- **Incremental delete/update via per-segment live-docs** (`state.rs:1052-1111`
  full rebuild on any update/delete; unused `live_docs.rs` codec) — the quadratic
  rebuild on update/delete-heavy bulk is real and general. *Parity-unsafe as
  written:* retained-but-tombstoned postings inflate `doc_freq`/`doc_count`/
  `avg_doc_len` (shifting IDF) and would emit deleted docs as hits unless
  filtered; Surch's current baseline rebuilds so stats reflect deletes
  immediately. Adoptable only by reproducing Lucene's exact stat-staleness +
  refresh/merge timing (far beyond "flip a bit") — pair with the segmented
  structural bet.

## deces search-latency (engine-to-engine vs ES 8.6.1) — the latency front

The engine-to-engine deces latency probe (`surch-eval` CI `latency_engine.sh`,
real backend query shape replayed directly on `_search`, NO Node backend)
exposed the true search-latency gap the confounded artillery had hidden.

**Baseline (`sha-f0a8d11`, run `26609427689`)**: ES 8.6.1 p50 **3.7 ms** vs
Surch p50 **4513 ms** (~1200x). Root cause (code-verified): the deces query
`bool.must[function_score{ bool{should:[match PRENOM, match NOM], msm:2} }]`
(a) failed candidate resolution at the `function_score` wrapper (`_ => None`) →
full 1.36M-doc scan, and (b) the bool `should` path UNIONed + scored the whole
should posting set even though `msm == n_should` is a conjunction.

**Optimisation (2 commits, parity-safe — verified: surch-api suite green, oracles
bit-stable, 0 clippy)**:
- `113e4ef` — intersect `should` clauses when `minimum_should_match == n_should`
  (conjunction) instead of unioning + post-filtering.
- `ec3e999` — resolve candidates through the `function_score` wrapper (functions
  only re-rank, never filter → recurse into `inner`).

**Result (`sha-ec3e999`, run `26616206949`, same probe/corpus)**: Surch p50
**4513 → 87.2 ms (~52x faster)**, p95 166 / p99 197 / max 287, 0 errors. The
deces latency gap vs ES collapses from **~1200x to ~24x** (87 vs 3.7 ms).

**Honest residual**: the remaining 87-vs-3.7 ms is no longer union/disjunction —
it is the public-`_id` `BTreeSet<String>` intersection + per-candidate scoring
(+ the 2-vCPU runner). Next lever for deces is the dense-int-docid id maps
(backlog #10), NOT the bool-disjunction WAND. The bool-disjunction WAND (`msm <
n_should`, reusing the `MaxScoreExecutor` for `msm:1`) remains a valid general
optimisation but targets a query shape the deces probe does not exercise.

### deces latency #10 — dense u32 candidate intersection (`8aae6a1`)
`posting_candidate_ids` + `documents_for_{term,match,prefix}` resolved candidates
as public-`_id` `BTreeSet<String>`, cloning a `String` per matching doc (tens of
thousands per clause). Switched candidate resolution + intersection to internal
`u32` doc-ids (new `term_hits_internal`/`prefix_hits_internal` + AppState
wrappers; `documents_for_match_internal` already existed); public ids resolved
only for the final window. Parity-safe (same doc set; surch-api 37 blocks green,
0 clippy). **Measurement (run `26651526846`, same probe/corpus)**: Surch deces
p50 `87.2 → 69.9 ms` (−20%), **p95 `166 → 84.7 ms` (÷2)**, p99 `197 → 120.5`,
max `287 → 150`. Cumulative deces latency **4513 → 70 ms (~64x)**; gap vs ES
3.7 ms now **~19x** (was ~1200x). Residual: candidate resolution still
materialises both full posting lists before intersecting (O(df)/clause) — a
leapfrog/galloping skip-list intersection is the next deces lever (diminishing
returns) + the 2-vCPU runner cap.

### deces latency #11 — leapfrog/galloping conjunction (`a6fa7aa`) — NEUTRAL on deces (honest)
Hypothesis (from #10's residual): the bool conjunction materialised BOTH clauses'
full posting lists before intersecting (O(df)/clause); a leapfrog/galloping
intersection (Lucene `ConjunctionScorer`) driving the rarest term and
`advance_to`-ing the others over the FoR block skip-lists would avoid touching
the full lists. Implemented: `conjunction_hits_internal` (held-cursor leapfrog —
`advance_to` returns-and-consumes, so each iterator's current doc-id is held and
only re-advanced when the driver target exceeds it; falls back to an exact
materialised `BTreeSet` intersection when a list has no skip list),
`conjunction_leapfrog` (single-token gate), and a `posting_candidate_ids` Bool
fast-path engaging on pure single-term conjunctions (`must`/`filter` + `should`
when `msm == n_should`).

**Parity-safe, verified**: `cargo test --workspace` green incl. a new multi-block
parity test (`bool_conjunction_leapfrog_matches_btreeset_intersection_across_blocks`,
>128 postings so the skip path is exercised); ci-k8s `ndcg-gate` green — SciFact
Surch `0.6576` (OS `0.6537`), TREC-COVID Surch `0.4750` (OS `0.4902`), **unchanged
vs pre-#11 → no relevance regression**.

**Measurement (run `26668292578`, sha-`a6fa7aa`, same probe/corpus, es+surch in
the SAME run)**: deces full p50 Surch **78.2 ms** / ES **4.6 ms** (17x). Decompose:

| p50 (ms) | match (1 term) | bool (PRENOM ∧ NOM) | full |
|----------|---------------:|--------------------:|-----:|
| ES 8.6.1            | 3.6  | 3.1  | 2.7  |
| Surch #10 (prev run)| 36.1 | 68.2 | 68.7 |
| Surch #11 (this run)| 39.7 | 74.4 | 75.2 |

**Honest verdict: #11 is latency-NEUTRAL on deces.** The tell: `match` (a single
term — the leapfrog path is NOT engaged for it, `pairs.len() < 2`) moved
36.1 → 39.7 (+10%) with ZERO code change to that path, i.e. this run's 2-vCPU
runner is simply ~10% slower; applying that same +10% to #10's bool (68.2 × 1.10
≈ 75) fully accounts for the 74.4 measured. So the conjunction intersection
strategy changed bool latency by ~0%. The conjunction was **not** the deces
bottleneck.

**What the decomposition (re)proves is the real lever**: a single `match` on a
common term costs **~40 ms ≈ 11× ES** (3.6 ms), and `bool ≈ 2× match`. The
dominant cost is the **per-term O(df) hot loop** — FoR-decode + BM25-score the
whole posting list for a common term — NOT how clauses are intersected. ES never
pays it: block-max WAND top-K scores only enough for the top-20 (its `bool` 3.1 ms
is even cheaper than its `match` 3.6 ms). The next deces lever is therefore to make
Surch's bare-`match` top-K (`run_topk_search`/`maxscore_match`) actually **skip**
on this corpus so a common term stops costing O(df); that also fixes `bool` (which
is ~2× `match`). #11 stays in (parity-safe, sound for selective conjunctions where
`df_rarest ≪ df_other` and single-token clauses) but is honestly logged as not
moving the deces number.

### deces latency #12 — per-QUERY O(n) setup elimination (`de19a9c`+`dfb6c25`) — the breakthrough
The #11 note guessed the lever was the per-DOC hot loop (FoR-decode + BM25 per
scored doc, a "constant-factor ~11× battle"). **That diagnosis was wrong, and the
data says so.** Reading the path showed the in-memory postings are already
decoded (`&[Posting]`, no per-doc FoR decode on the read path), and the real
cost was **per-QUERY O(n) setup**, paid once per query regardless of how few docs
match:
1. `SearchScoringContext::new` copied the ENTIRE per-doc length map into a
   `Vec<(u32,u64)>` by walking a `BTreeMap<u32,u64>` — O(n) pointer-chasing over
   ~all 1.36M docs, for every query touching a norms field — and then probed it
   per scored doc with an O(log n) binary search.
2. `match_hits_internal` built a `BTreeSet<u32>` from the (already sorted, unique)
   postings just to return the candidate set — O(df log df) inserts + a node
   allocation per doc on a common term.

Two parity-trivial changes:
- **`de19a9c`** — store per-doc length as a dense `Vec<u64>` indexed by doc_id
  (`0` = absent) instead of a `BTreeMap`; the per-query build is a flat memcpy
  and the lookup is an O(1) cache-friendly index.
- **`dfb6c25`** — single-token `match` candidate resolution collects posting
  doc_ids straight into a `Vec` (postings are already ascending+unique), skipping
  the `BTreeSet` round-trip.

**Measurement (run `26696446460`, sha-`dfb6c25`, same probe/corpus 1.36M)**:

| Surch p50 (ms) | match | bool | full | full probe (2000) |
|----------------|------:|-----:|-----:|------------------:|
| before (#10/#11) | 36–40 | 68–74 | 69–75 | 70–78 |
| **after #12**    | **5.3** | **6.8** | **7.1** | **6.9** |

**deces full p50 ~70 → 6.9 ms (~10× faster); the gap vs ES (~4.6 ms stable
baseline; the es job flaked this run on the wikidata fetch) collapses from ~17×
to ~1.5×.** Parity-safe: identical doc_len values + identical candidate set →
bit-identical BM25 (`cargo test` oracles green; ndcg-gate pending). This is the
breakthrough on the deces front — the bottleneck was per-query allocation/
pointer-chasing setup, not per-doc arithmetic. Remaining to reach the "2× faster
than ES" bar (Surch ≤ ~2.3 ms): the zero-copy `doc_len` borrow (removes the
per-query flat copy entirely, in flight), then the residual `bool`/`function_score`
wrapper overhead and the WAND scoring loop.

### deces latency #13 — setup-cost batch 2 (`3bfec8f`+`2c59e91`) — **2× FASTER THAN ES (p50) — criterion MET**
Two more per-query setup eliminations on top of #12: **`3bfec8f`** borrows the
dense `doc_len` slice zero-copy (no per-query flat copy at all — deces touches
PRENOM+NOM, so two full-corpus length arrays were copied per query), and
**`2c59e91`** precomputes `min_doc_len` incrementally so the WAND upper bound
stops re-scanning the whole dense slice per query.

**Measurement (run `26697199003`, sha-`2c59e91`, same probe/corpus 1.36M, with a
CLEAN same-run ES baseline — the es job did not flake this time)**:

| deces p50 (ms) | match | bool | full | full probe (2000) |
|----------------|------:|-----:|-----:|------------------:|
| ES 8.6.1          | 3.8 | 3.1 | 2.7 | **4.9** |
| **Surch batch 2** | **1.7** | **1.8** | **1.9** | **2.0** |

**Surch p50 2.0 ms vs ES 4.9 ms = 2.45× FASTER — the user's "at least 2× faster
than ES" criterion is MET on the median** (and on the mean: 3.8 vs 5.5 ms). On
the decompose, Surch beats ES on every shape's p50 (match 1.7 vs 3.8, bool 1.8 vs
3.1, full 1.9 vs 2.7). Cumulative deces journey: **4513 ms → 2.0 ms p50** across
#1→#13, gap vs ES from ~1200× SLOWER to **2.45× FASTER**. Parity-safe throughout
(`cargo test` oracles + ndcg-gate green; bit-identical BM25).

**Honest caveat — the new front is the TAIL.** Surch's median wins, but the upper
percentiles trail ES: p95 14.3 vs 10.6, p99 20.8 vs 15.2, max 62.8 vs 21.8 (the
decompose shows it is the `bool`/`full` shapes: p95 ~14 / max ~60, while the bare
`match` tail is already better than ES — p95 3.0 / max 8.8 vs 8.0 / 17.9). Cause:
high-`df` name pairs whose `bool`/`function_score` query runs the full-scan
`run_search` (scores the whole candidate set + sorts), where ES prunes with
block-max WAND top-K. **Next deces lever = extend the WAND/top-K shortcut to
`bool` (minimum_should_match) + `function_score`** so the common-name tail stops
scoring the full intersection — the structural read-path optimisation that
tightens p95/p99 toward (and past) ES.

### deces TAIL #14 — root-caused to full-candidate HYDRATION (two hypotheses refuted by data)
The p50 criterion is met; the front is the `bool`/`function_score` p95/p99 tail.
Two data-driven attempts, each MEASURED on the cluster, each parity-safe but
NOT the tail:
- **`2e7186b`** — thread internal doc-ids to scoring (drop the per-doc public-`_id`
  String-hashmap round-trip). Tail unchanged (bool p95 14.3 → 15.0).
- **`97a0ca0`** — skip `query_matches` when the postings candidate set is
  provably exact (`candidate_set_is_exact`; the deces conjunction is exact, so
  the per-doc 2-field re-analysis is a no-op). Tail unchanged (bool p95 → 15.2).
  Parity CONFIRMED by the b1 deces oracle: **0 divergences vs ES** (the ci-k8s
  `b1-oracle-gate` workflow went red only on a report-copy step — "no benchmark
  summary file copied from /reports" — an artifact glitch to fix in WP-B/Track E,
  NOT a parity issue; the oracle exited 0).

**Root cause (isolated by elimination + the match-vs-bool contrast)**: the
`run_search` path **hydrates EVERY candidate** (`documents_with_internal_ids`:
2 hashmap lookups + 2 `String` allocations per doc) over the high-`df`
intersection, then scores + sorts, then paginates to 20. The bare-`match`
`maxscore` path (good tail, p95 3.0) scores all `df` but hydrates only the final
top-20 — so scoring is NOT the tail, full hydration is. The deces `full` query is
routed onto the full-hydration path by `min_score:0` + `function_score` +
`track_total_hits` (each disqualifies `run_topk_search`).

**The real fix** (substantial, accumulated): a top-K path for the exact
`bool`/`function_score` case — score from internal ids (BM25 reads term stats,
not `_source`; an empty `function_score` is transparent), keep a K-sized heap,
hydrate ONLY the window. Parity-critical (must preserve `total` for
`track_total_hits`, `min_score` semantics, and the function_score source-need
when functions reference fields), so it warrants a focused, oracle-gated pass.
The two optimisations above stay (sound, parity-safe; `candidate_set_is_exact`
is the reusable gate for that path).

### deces TAIL #15 — the top-K path landed (parity-clean) but the tail STILL did not move
`f3ff8ca` added `run_topk_exact_bool`: the exact `bool`/`function_score` case
(deces shape) now scores straight from internal ids, keeps a K-sized heap, and
hydrates ONLY the result window — no full-candidate hydration, no full sort.
Parity CONFIRMED: b1 deces oracle **0 divergences** vs ES (+ `matchid_compat`
green). **Yet the tail is unchanged** (bool p95 15.2 → 15.5, full 14.6 → 15.4;
p50 still 1.9 vs ES 4.9 = 2.6× faster). So full hydration was NOT the tail
either — the THIRD code-level hypothesis refuted by measurement (after the id
round-trip and `query_matches`).

**Honest stop-and-assess**: three parity-safe code changes, each targeting a
plausible O(n) cost, NONE moved the p95/p99. The remaining candidates can no
longer be distinguished by reading code — it is either (a) the leapfrog
*finding* cost itself (`advance_to` × `df_rarest`, independent of the
intersection size, so unaffected by hydration/scoring changes) or (b)
infrastructural CPU-oversubscription jitter (the probe runs 4 workers on a
2-vCPU runner = 2× oversubscription, and Surch's p50 is so low — 1.9 ms — that
scheduling jitter dominates the *ratio* p50→p95 far more than for ES's 4.9 ms
p50). **Next step is DIAGNOSIS, not another blind fix** (the #11 lesson): either
a WORKERS=2 probe run (isolates the oversubscription hypothesis cheaply) or
timing instrumentation of the bool/full path on the cluster. All three
optimisations stay — they are sound, parity-clean, and reduce real per-query
work (the engine is leaner even if this particular tail is elsewhere).

**Bottom line on deces: the "2× faster than ES" criterion is MET and stable
(p50 ~2.0 ms vs ES ~4.9 ms, ~2.5–2.6× across four independent runs, mean also
ahead, parity bit-clean). The p95/p99 tail remains the one axis where ES leads,
pending a proper diagnosis of its (now narrowed) cause.**
