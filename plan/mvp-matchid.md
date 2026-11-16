# MVP — Surch replaces Elasticsearch in matchID

## Finality

Serve the matchID `deces` index (~29.3 M INSEE death records, monthly
reload) from Surch instead of Elasticsearch 8.6, with deces-backend unchanged
(same `@elastic` client, same request bodies), on a node half the memory of
the current reference node.

Owner order 2026-11-15 (relayed by the matchid session): "finaliser surch pour
remplacer elasticsearch au passage dans matchid ... occuper 2x moins de
mémoire".

## Primary track

Track D — matchID. Lots L1 and L3 report on Track A / B, L4 on Track C.

## Branch and ownership

- Branch: `main` (short-lived feature branches per lot when work runs in
  parallel; worktrees removed right after merge).
- Conductor: surch session. Counterpart: matchid session (integration study,
  double run, all matchID clusters). Surch never touches matchID clusters.
- L1: #3 SearchEngine (`crates/surch-search`, `crates/surch-api/src/search.rs`,
  `crates/surch-index` term dictionary access).
- L2: #3 SearchEngine + #4 APIServer (aggs, client compat), fixtures under
  `tests/matchid_compat/`.
- L3: Track B harness (`deploy/bench-local/`).
- L4: #4 APIServer (`crates/surch-api/src/snapshot_es/`, `aliases.rs`).

## Exit criteria (agreed with matchid 2026-11-15)

Target node class: 2 vCPU / 7–8 GB (b2-7 / b3-8). Budget left for Surch,
estimated by matchid (unverified until k8s measures allocatable):
~4.5 GiB memory including page cache, ~1 CPU core.

- [ ] 29.3 M docs served under cgroup caps memory 4.5 GiB AND CPU 1 core.
- [ ] `deces_v2` query corpus passes the parity gate against ES 8.6.1.
- [ ] Search p95 and bulk-link throughput (rows/s) measured against ES 8.6.1
  under the same caps, real deces-backend query shapes.
- [ ] Restore time from S3 and artefact size measured at 29.3 M.

Owner decisions relayed by matchid (2026-11-16):

- Parity is judged on deces-backend outputs, not on raw engine responses:
  same hit selection under `min_score: 5`, same top-k order, same totals,
  and every score that reaches the backend output (`maxScoreES` raw,
  `scores.es` rounded in steps, score.ts). `maxScoreES` raw keeps the
  exact-parity bar unless the owner masks it (matchID decision D18). L7
  therefore still targets bit-exact `_score` / `max_score`.
- Bench targets both OVH b3-8 (2 vCPU / 8 GB) and d2-8 (4 shared vCPU /
  8 GB); the OVH migration waits on that result.
- The Node backend is inside the envelope: "half the memory and load"
  applies to the whole matchID stack, not only the engine.
- GEOPOINT_NAISSANCE / GEOPOINT_DECES are WKT strings `POINT (lon lat)`
  (dataprep `geopoint_2d`); Surch must index, query and return them
  verbatim.

## Interface decision

- [ ] Owner ratifies option (a): Surch exposes the ES-compatible subset used
  by deces-backend; no adapter layer in matchID. Default (a), accepted by
  matchid. Contract = frozen `deces_v2` query corpus generated from
  `packages/deces-backend/src/buildRequest.ts` with synthetic inputs (no
  production queries, no user CSV).

## Baseline (state on 2026-11-15, `main` 37314e6)

- Parity: B1 oracle 30/30, 0 divergence vs ES 8.6.1 (`ci-k8s` 26192816780),
  1 000-doc corpus only.
- Memory at 28.9 M (2026-07-13, 8 engine CPUs): anon steady state 2 751 MB
  vs ES 3 835 MB; Surch survives a 4 GiB cgroup cap 2/2. Never measured at
  1 core.
- Disk at 28.9 M (2026-07-13): 12 296 MiB vs ES 9 115 MiB, before D1 (block
  `_source`, -67 % `_source` at 1.36 M) and D2 (postings bit-packing).
- Build: ~12 k doc/s at 28.9 M.
- Known performance blocker: `fuzzy`, `match`+`fuzziness`, `range`,
  `geo_distance` and root `function_score` have no postings-backed candidate
  set; `match_documents_for_index` falls back to a full scan of every stored
  document (`crates/surch-api/src/search.rs`, `match_documents_for_index`,
  `posting_candidate_ids`). Aggregations evaluate on matched documents'
  `_source`.
- Reload: S3 snapshot repository, `_restore`, atomic `_aliases` exist; no
  `rename_pattern`; never exercised at 29 M. ES Lucene snapshots are not
  readable by Surch: the Surch index must be built in matchID dataprep from the
  same source NDJSON.

## Lots

- [x] L0 — Refresh conductor plans: this file + `PLAN.md` MVP section.
- [ ] L1 — Fuzzy and non-term clauses served from the index (blocker).
  - [x] Fuzzy term expansion over the per-field FST term dictionary with a
    custom Damerau-Levenshtein `fst::Automaton` (the `fst` levenshtein
    feature has no transpositions); wire the unused
    `surch_search::expand_fuzzy_terms` semantics (`prefix_length`,
    `max_expansions`). Fuzzy is ON by default in deces-backend
    (`requestInput.ts`), so this covers default UI search, `q`, `/agg`,
    CSV export scroll pages and link rows.
    Branch `worktree-agent-af16e91d23b04f656`: `b4ebe9a` (automaton
    `surch_index::fuzzy`, `TermDictionary::fuzzy_terms`,
    `DocumentIndex::fuzzy_terms` over every segment, Lucene top-terms
    selection in `surch_search::fuzzy`), `c79231f` (property tests vs
    `bounded_damerau_levenshtein`, single / multi-segment / disk-backed FST
    walk).
  - [x] Postings-backed candidates for `match`+`fuzziness` and `fuzzy`
    (`e53df7f` state accessors, `b746f0f` search wiring + exact candidate
    set + BM25 over expansions, `632c9fd` oracle parity tests on RAM /
    multi-segment / disk layouts).
  - [x] Candidate push-down through root `function_score` (inner query),
    including with aggregations / custom sort / `min_score`; candidate
    resolution returns `(ids, exact)` decided during resolution (`81473ed`,
    branch `worktree-agent-a38723b4f2ef11543`).
  - [x] `range` on text / keyword / numeric fields from the term
    dictionary (FST byte range or numeric term walk, every segment, RAM and
    disk-backed; `range_field_matches` applies the same per-type ES plan, so
    the set is exact) and `geo_distance` / `geo_bounding_box` /
    `geo_polygon` from the `geo_point` term dictionary (one term per
    distinct point); both resolve last inside a `bool`, restricted to the
    sibling intersection, and next to <= 2 048 sibling candidates they are
    checked on those documents' `_source` (`48bc647`, `81473ed`,
    `7bfb33a`). `match_all`-equivalent wrappers served as `match_all`.
  - [x] Guard: no full-scan path reachable from any `deces_v2` shape
    (counter + test).
    - [x] Counter `surch_dbg_full_scan_fallback_total{query_type}` on every
      `_source` full-scan fallback of `match_documents_for_index`, and a
      test proving fuzzy / bool-of-fuzzy shapes never reach it (`a480f2a`).
    - [x] Extend the test to every `deces_v2` shape: module
      `l1b_build_request_guard_tests` (`crates/surch-api/src/search_l1b_guard_tests.rs`)
      ports `buildRequest.ts` / `queries.ts` and emits 47 shapes (simple
      `q`, `id` via `_msearch`, advanced fuzzy / exact, date prefix / range /
      limits, age, geo, link blocks via `_msearch`, `/agg` terms /
      date_histogram / composite + cardinality / `after`, no-criteria
      `/agg`); RAM, multi-segment and disk-backed: 0 fallbacks and match
      set equal to brute force (`81473ed`, `7bfb33a`).
  - [x] Pre-existing bug fixed on the way: keyword `prefix` matched nothing
    on a disk-backed layout (`48bc647`).
  - [x] High-df fuzzy OR scoring cost (`q` "jean pierre": 42 501 hits of
    200 k, 43.7 ms; ~21 % of 29 M would be scored). MaxScore / top-k
    pruning does not apply to the deces `q` and advanced shapes
    (`track_total_hits: true` + `min_score` require scoring every match,
    in ES too); merged fuzzy pass delivered by L1c below (`39bdebc`).
  - L1b 200 k synthetic release measurement
    (`tests/l1b_measure.rs`, local, median of 5, before = `1422bd1`):
    birth range alone 968 -> 0.87 ms; age range alone 926 -> 0.81 ms;
    death > 2000 + sex 513 -> 11.3 ms; geo 5 km alone 896 -> 1.3 ms;
    name + geo 25.2 -> 4.1-4.4 ms; name + birth range 20.0 -> 4.3-5.0 ms;
    link name + birth range 56.9 -> 4.4-4.8 ms; `q` stop words only
    (`match_all`, `min_score` 5) 864 ms -> 7 us; unchanged: name-only
    shapes (21-31 ms both, noise), link name + full date (4.6-5.2 ms),
    `/agg` without criteria 950-990 ms (whole-index `_source`
    aggregation, lot L2 item "avoid whole-index materialisation").
  - [ ] Proof: B1 oracle unchanged, unit/parity tests, 1.36 M smoke latency
    per form.
    - [x] Unit/parity tests (fuzzy part): `matchid_compat` and existing
      fuzzy tests green; 200 k synthetic measurement
      (`fuzzy_postings::measure_candidates_vs_full_scan_on_200k_docs`,
      release, local, 5 runs): `match nom dupont` AUTO 3 615 candidates
      (1.8 %) median 8.7 ms; `bool must[fuzzy nom, fuzzy prenoms] +
      filter` 145 candidates median 3.3 ms; `match prenoms "jean pierre"`
      AUTO OR 30 802 candidates (15.4 %) median 139.8 ms (scoring cost
      O(candidates x expanded terms), follow-up); full-scan baseline
      (`wildcard`) median 151.8 ms.
    - [ ] B1 oracle re-run on `ci-k8s` and 1.36 M smoke latency per form.
  - ES fuzzy deviations accepted (L1): scoring over the expanded terms with
    Lucene's per-term similarity boost and blended (max) doc freq per token
    follows ES's `top_terms_blended_freqs_N`; since L7 the kernel is ES 8.6.1
    `LegacyBM25Similarity` in f32 (absolute scores lucene-source-derived,
    ES gate pending); the
    `fuzzy` query still analyses its `value` and ANDs the tokens (ES takes
    one raw term); a `_source` re-check of a fuzzy clause (only under
    `must_not` or a should-disjunction superset) does not apply
    `max_expansions`; `match` `minimum_should_match` with `fuzziness` is not
    parsed.
  - ES range / geo deviations accepted (L1b): a non-numeric bound on a
    numeric field matches nothing (ES: HTTP 400); `float`/`double` compare
    as f64 (ES: field precision); `gt`+`gte` both given intersect (ES: the
    last one wins; Surch behaviour unchanged); a numeric bound on a
    text/keyword field is compared as its decimal text (Java `toString` for
    integers, Rust shortest repr otherwise); `range` on a `date` field has
    no term walk (counted full scan unless <= 2 048 sibling candidates; not
    emitted by `buildRequest.ts`); a `geo_point` indexed as a `[lon, lat]`
    array declines to the counted full scan (the indexer flattens it).
    `range_field_matches` now follows the term-level ES semantics on
    `_source` too (analysed tokens for text, normalised bound for keyword,
    numeric order for numbers, multi-valued); geo `_source` checks accept
    arrays of points.
- [ ] L1c — deces-backend `/agg` and high-df fuzzy at 29 M scale (Track A/D,
  #3 SearchEngine). Branch `worktree-agent-a7571aeec3a07e136`, base `39ceb03`,
  not merged to `main`.
  - [x] Index-time JSON value kinds per top-level scalar field (one
    non-blank term equal to the read value, or an integer `|n| <= 2^53`
    indexed as its decimal text), recomputed by a full rebuild, OR-ed by
    appends; `META` format 3 (format 2 still opens, kinds unknown);
    `IndexReader` doc-values accessors (`edeafe1`).
  - [x] `size: 0` aggregations (terms, date_histogram, cardinality,
    composite incl. `after`) served from sub-field columns (`.raw`) or term
    dictionary x postings over the matched ids, never `_source`; same
    finishing code as the `_source` path. Declines: field not provably
    rebuildable, superset candidates, index missing live documents,
    <= 2 048 matches (`39bdebc`).
  - [x] Whole-index `/agg` counts from term document frequencies (term
    metadata only); a declined candidate set is reused by the general path;
    `min_score <= 0` skips per-document scoring (`d8a8642`).
  - [x] Merged fuzzy scoring pass: each fuzzy clause occurrence scored for
    the whole candidate batch in one walk of its expanded postings,
    bit-identical (`39bdebc`).
  - [x] `bool.should` with `minimum_should_match: 1` resolves to the exact
    union (deces `nameQuery`, two-name `q`): no `_source` re-check
    (`39bdebc`).
  - [x] Tests: doc values vs brute force on RAM / multi-segment /
    disk-backed / disk multi-segment layouts with deletes, composite paging
    to exhaustion, declining fields, restart from the data directory;
    merged fuzzy pass vs per-document scorer (f32 bits + full responses)
    (`search_l1c_tests.rs`, `39bdebc`, `d8a8642`).
  - [x] Measurement `tests/l1c_measure.rs` (`bbd9618`), release, local,
    median of 5, real `deces_index.yml` analysis, prod env (disk postings,
    block `_source`, 256 MiB flush budget), 4.5 GiB cgroup cap. Response
    digests identical before/after on every shape. 200 k (before -> after):
    `/agg` terms no criteria 1 186 -> 1.6 ms; terms `NOM.raw` 1 130 -> 1.8 ms;
    date_histogram 1 134 -> 9.5 ms; composite sex x dept 1 565 -> 3.2 ms;
    composite page 2 1 213 -> 3.3 ms; composite city x date page 2
    1 328 -> 42 ms; composite with fuzzy prenoms criteria 168 -> 4.2 ms;
    `q` two names 25.9 -> 3.9 ms; advanced fuzzy prenoms 4.9 -> 1.7 ms;
    RSS peak 1 367 -> 710 MiB. 1.36 M (before -> after): `/agg` terms no
    criteria did not finish its first shape in 13 min (RSS 4.2 GiB at the
    cap, reclaim thrash) -> 11.6 ms; date_histogram -> 52 ms; composite
    -> 23 ms; city x date page 2 -> 211 ms; composite with fuzzy criteria
    1 206 -> 38 ms; `/agg` name criteria 209 -> 47 ms; `q` two names
    214 -> 33 ms; fuzzy prenoms 62 -> 9 ms; RSS steady 1.23 GiB. Builtin
    tokenizing analysis (L1 shape, 291 k fuzzy matches of 1.36 M): fuzzy
    prenoms 216 -> 62-75 ms, `q` two names 415 -> 92-130 ms, `q` jean
    pierre 74-113 -> 85-119 ms (no gain, within the run-to-run noise).
  - [ ] 29 M measurement under caps (estimate only, linear in documents:
    `/agg` terms ~0.25 s, date_histogram ~1 s, composite ~0.5 s,
    composite on two high-cardinality fields ~4.5 s, composite transient
    4 B x 29 M per source; not measured).
  - [ ] Remaining general-path shapes: aggregations with hits (`size > 0`),
    field sorts with a real query on `.raw` (whole-index projection map),
    composite over > 4 sources.
  - [ ] Finding for #2 Indexer (not fixed, out of L1c ownership): a value
    analysed to a blank term (`"--"` or `" "` through the deces `norm`
    normalizer) makes `PostingsBuilder::add` return `EmptyTerm`, and
    `DocumentIndex::add_documents_with_mapping_*` stops the batch merge
    there (`merge_analyzed(document)?`), silently leaving that document
    and the rest of the batch out of the index while their `_source` is
    served. L1c guards its own path (`index_covers_live_documents`,
    test `an_index_missing_live_documents_keeps_the_source_path`).
- [ ] L2 — `deces_v2` contract.
  - [x] DSL gap map deces-backend ↔ Surch with file:line evidence
    (matchID main `6732755`, Surch `37314e6`; kept outside the repo at
    `~/.cache-tmp/h2a-lanes/surch-cond/dsl-gap-map.md`, to be promoted under
    `docs/wp-d-matchid/` with the contract). `missing` agg and nested
    sub-aggs are NOT used by deces-backend: dropped.
  - [x] Composite agg emits `after_key` on every non-empty page
    (deces-backend `/agg` looped forever otherwise): `ea2cd47`.
  - [x] Empty `bool` (`bool{}`, `bool{should:[]}`) behaves as `match_all`
    instead of HTTP 400 (`ea2cd47`) (breaks `/agg` without criteria and `_msearch` link
    chunks).
  - [x] `GET /_cat/indices/{index}?h=creation.date.string` + stored creation
    date (`/version` endpoint): `index.creation_date` in the persisted
    settings, `strict_date_time` UTC print, `h` / `v` / `format`, patterns
    and aliases, text table for the restore-job forms (`72a32e6`, branch
    `worktree-agent-a4fe86f4ac68f17e0`, not merged to `main`).
  - [ ] L2b — ES 8.6.1 response bytes (`docs/wp-d-matchid/l2b-response-shapes.md`,
    every expectation `es-source-derived`, no ES run).
    - [x] Key order of search / hit / msearch / scroll / count / aggs
      (workspace serde_json `preserve_order`, ordered envelope structs),
      `_source` in document key order, `sort` values on field sorts,
      `_score` always written, `max_score` null on field sort / `size: 0`
      (`72a32e6`).
    - [x] Root `match_all` / no query score the boost 1.0 and honour
      `min_score` (`72a32e6`).
    - [x] Aggregations in request order; terms `doc_count_error_upper_bound`
      + `sum_other_doc_count`; composite `after_key` first, numeric keys
      sorted by value; keyword keys normalised (`72a32e6`).
    - [x] Golden byte tests `tests/es_response_shapes.rs` (11/12
      es-source-derived) + walk-vs-general-path guard on 3 layouts
      (`72a32e6`).
    - [x] `match_phrase`: not emitted by deces-backend (grep matchID
      `6732755`), positions left out of scope.
    - [ ] ES 8.6.1 differential run of the golden families (bytes,
      `took` / `_scroll_id` masked).
    - [ ] Residual deviations (doc §"Deviations kept"): exact `cardinality`
      vs HLL++ above 3000, `_score: null` on root `term`-like queries (L7),
      error envelopes without `root_cause`, `_cat` placeholders and `s`.
  - [ ] Analysis parity: `pattern_replace` char_filter (`alphanum`),
    configurable normalizer chain, real `deces_index.yml` mapping
    (DATE_* are `text` + `index_prefixes{4,6}`, `PRENOMS_NOM`, `PRENOM_NOM`,
    edge_ngram `max_gram` 10). Branch `worktree-agent-a095842a58b580b53`
    (base `292c037`), not merged to `main`.
    - [x] Found by the L1b guard: Surch's default `text` analyzer is
      `simple` (drops digits) where ES uses `standard`, so DATE_* without
      an explicit analyzer index no term; and a `match` on a keyword
      sub-field (`DATE_NAISSANCE.raw`, `COMMUNE_NAISSANCE.raw`) analyses
      the query with that same `simple` default (`IndexMapping::analyzer`
      only looks up top-level fields): digits dropped, hyphens split, so
      `match DATE_NAISSANCE.raw "19290107"` and `match
      COMMUNE_NAISSANCE.raw "Saint-Etienne"` match nothing (exact-mode
      advanced search and the `.raw` boost of `fuzzyShouldTermQuery`).
      Fixed: default `text` analyzer = ES `standard` (UAX#29, Java
      lowercase, `max_token_length` 255) `95a9e0b`; every field and
      sub-field indexed and queried through its declared chain, keyword
      `match`/`term` through the normalizer `5e1e9b7`.
    - [x] `pattern_replace` char filter (Java regex translated to the
      `regex` crate, Lucene offset correction) and configurable
      `char_filter -> tokenizer -> filter` chains for analyzers and
      normalizers, compiled from `settings.analysis`; unsupported
      referenced components fail index creation `95a9e0b`, `5e1e9b7`.
    - [x] Real mapping fixture `tests/matchid_compat/deces/mapping_real.json`
      (matchID `6732755`, conversion rules in the deces README) `5e1e9b7`,
      `e184d0d`.
    - [x] `_analyze` parity table `analyze_parity.json` (53 rows: 1 es-doc,
      49 spec-derived, 2 es-unverified, 1 known-deviation; Surch matches
      all) `e184d0d`.
    - [x] L1b guard on the real mapping (`PUT /deces` with the fixture):
      47 shapes x 3 layouts, 0 full-scan fallbacks, match sets equal to
      brute force, L2 empty-set exception removed `5e1e9b7`, `013e73b`.
    - [x] Persistence: `META` format version 2 (custom normalizer name,
      analysis stored as declared JSON); version-1 data directories fail
      closed and must be re-indexed `5e1e9b7`.
    - [ ] ES 8.6.1 oracle replay of `analyze_parity.json` (no row run
      against ES yet).
    - [ ] Snapshot restore re-parses `settings.analysis` (today
      `snapshot.rs` rebuilds the mapping from `properties` only, so a
      restored index falls back to the legacy builtin `norm`; Track C).
  - ES analysis deviations accepted (L2): emoji dropped by the standard
    tokenizer (Lucene emits `<EMOJI>`); South-East Asian scripts split per
    UAX#29 defaults; a lone `No` character (`²`) kept as a token; empty
    keyword values not indexed (ES indexes `""`); unknown analyzer /
    normalizer names in a mapping fall back to `standard` / verbatim on
    both sides (ES rejects the mapping); a `normalizer` on a `text` field
    is rejected as in ES; `term`/`prefix` on a `text` field still use the
    first search-analyzer term (ES does not analyse them); Java regex
    constructs without a `regex` crate equivalent (look-around,
    back-references, possessive quantifiers, `\p{InBlock}`) are rejected.
  - [x] Fuzzy semantics: OR over tokens (not AND), BM25 on expanded terms,
    `boost` honoured inside `match` objects (L1 `b746f0f`, tests `632c9fd`).
  - [x] `date_histogram` keys as epoch ms + `key_as_string` (request format,
    else field format) + empty buckets between first and last
    (`min_doc_count` 0); composite date source keys formatted or epoch ms
    (`72a32e6`).
  - [ ] Avoid whole-index materialisation for sort/agg on `.raw` and for
    `/version` (top-1 by date over 29 M docs).
    - [x] `/version` (`sort` SOURCE desc + `DATE_DECES.raw` desc, no query,
      size 1): term-dictionary walk with postings intersection, only the
      window hydrated; equal to the general path on 3 layouts
      (`72a32e6`). Not measured at 29 M.
    - [ ] Field sorts with a real query and aggregations on `.raw`
      (`size: 0` aggregations done by L1c `39bdebc`; field sorts and
      aggregations with hits remain)
      (whole-index projection map, `_source` hydration).
  - [ ] `@elastic` client 8.18.2: ship with `SURCH_ELASTIC_PRODUCT_COMPAT=1`
    in the matchID deployment (header required on every 2xx).
  - [ ] Query corpus generator from `buildRequest.ts` + synthetic inputs,
    covering P1/P3/P4 link shapes (B1 used a simplified mapping).
  - [ ] ES 8.6.1 oracle gate on `deces_v2`.
- [ ] L3 — 29.3 M campaign under caps 4.5 GiB / 1 core, ES vs Surch.
  - [ ] Real query mix per form (search, link, aggs), no aggregate ratio.
  - [ ] Memory breakdown (anon, file cache) and reduction if over budget.
  - [ ] Infra: ephemeral VM (owner decision), destroyed after each run.
- [ ] L4 — Monthly reload path.
  - [x] `POST /_aliases` atomic across actions (single lock), `remove_index`
    action, `index: "*"` wildcard in remove: `4c1a83e`.
  - [ ] Alias persistence across restart: NOT persisted (state is
    in-memory, see L6).
  - [ ] `_restore` `rename_pattern` / `rename_replacement`.
  - [ ] S3 repo settings parity (`path_style_access`, `protocol`,
    `readonly`), credentials via env/secret.
  - [ ] `_cluster/health/<idx>?wait_for_status=green`, marker index write.
  - [ ] Offline build → S3 → restore → alias swap timed at 29.3 M.
- [ ] L5 — Double run ES/Surch with matchid (matchid owns infra). The owner
  widened it on 2026-11-15 to a multi-engine bench (ES, Postgres, DuckDB,
  Surch, others) on ephemeral OVH GRA b3-8 nodes created by k8s, replaying
  real production search + bulk requests (bench servers in France, purged
  afterwards), with byte-identical API responses (timing fields excluded) as
  the parity bar. Design study owned by matchid.
- [ ] L6 — Persistent index (blocker found 2026-11-15). Surch keeps no data
  directory: `AppRouterState::default()` starts empty, postings / `_source` /
  sub-field spills are `TMPDIR` tempfiles, and `_snapshot` stores documents
  for re-ingest. A pod restart means re-indexing 29 M docs, hours on 1 core.
  Design exists, never implemented:
  `docs/paper/persistence-iceberg-architecture.md`.
  - [x] Segment files + atomic manifest in a data directory; reopen at
    startup (FST, term entries, block directories, doc maps, live docs,
    mapping, settings, aliases). Step 1 on branch
    `worktree-agent-ae0f04c13f8ade733`: `9c3b335` (framed blake3 files,
    segment / term-dictionary encode + reopen), `3191623`
    (`SURCH_DATA_DIR`, manifest per index, persist on refresh / create /
    mapping / alias / delete, crash recovery, round-trip + crash +
    version tests), `e42da5b` (ops doc), `f19d8ad` (out-of-refresh
    commit seals pending writes, fallocate retry guard). Layout and
    crash-safety argument:
    `docs/ops/persistence.md`. 200 k local proof: restart to first query
    46 ms, responses identical, RSS after reopen 30.8 MiB.
  - [ ] Step 2 follow-ups found by step 1: index / component templates
    not persisted; `_bulk?refresh=true` and `_restore` do not commit
    (no `_refresh`); id maps rewritten whole per changing refresh
    (append-only layout); no translog; writes after a reopen take the
    existing full `rebuild_index()` path.
  - [ ] Snapshot = copy of sealed segment files (S3 artefact built offline,
    loaded without re-indexing).
  - [ ] Proof: restart at 29 M under the target caps, time to first query.
- [ ] L7 — Lucene score parity (needed for the byte-identical bar).
  Branch `worktree-agent-a3c9adc9e9b4682ab` (base `01f2d94`), not merged to
  `main`. Every expectation is `lucene-source-derived` until the ES gate runs.
  - [x] BM25 kernel in strict f32 as Lucene 9.4.2 `BM25Similarity` (idf,
    `avgFieldLength`, `LENGTH_TABLE` norm cache, `weight - weight / (1f +
    freq * normInverse)`): `6805d18`. Finding: ES 8.6.1's default similarity
    is `LegacyBM25Similarity`, which passes `boost * (1 + k1)` to Lucene
    (`SimilarityService.java:48`, `LegacyBM25Similarity.java:66-67`), so ES
    scores KEEP the `(k1+1)` factor; the "~2.2x" premise was wrong and the
    `min_score: 5` result sets were already on the right scale (deces_v1
    replay: totals and the 50-hit order are unchanged by L7).
  - [x] Boolean combination (`ConjunctionScorer` / `DisjunctionSumScorer` /
    `WANDScorer` double sum then `(float)`, `ReqOptSumScorer` float add,
    FILTER-only `0`, empty `bool` = `match_all`), top-down boost threading,
    duplicate-token weight boost, constant-score clauses, numeric `match`
    constant, keyword `freq = 1` / no norms, multi-field sub-field stats
    (`NOM.raw`), `min_score` as float, MaxScore admission on the suffix
    bound compared in f32, fused / streamed paths rounded once: `70d3e6a`.
  - [x] Fuzzy `TopTermsBlendedFreqScoringRewrite`: per-term boost
    `1.0f - (float) ed / (float) min(cp(term), termLength)` (clamped `>= 0`
    at clause build), blended df = max, SHOULD double sum: `70d3e6a`
    (scoring) + `d3a2694` (tests).
  - [x] `function_score` arithmetic as ES 8.6.1 (`CombineFunction` in
    double then float, `LOG1P = log10`, AVG over weights; no-function
    wrapper threads its boost): `70d3e6a` + `e9a74a0` (tests).
  - [x] `_score` / `max_score` serialized as Jackson 2.13.3 +
    JDK 19.0.1 `Float.toString` (`1.0E-4`, `0.33333334`, `1.2345678E7`):
    `07264ec`.
  - [ ] Tie order: Surch breaks ties on ascending internal id like
    `HitQueue`; index Surch in the `sort:_doc` order exported from the
    reference ES index (1 shard, force-merged) so docid tie-breaks match.
  - [x] Differential gate specified (request list, comparator spec in Rust
    `b1_oracle`, explain-leaf localisation):
    `docs/wp-d-matchid/l7-score-parity-gate.md`.
  - [ ] Differential gate RUN vs ES 8.6.1 (per-hit `_score` bits + token
    text + `explain`), then flip the `lucene-source-derived` marks.
  - [ ] Residuals (gate doc §6): 3+ non-power-of-two boost products,
    repeated keyword value in one doc; `match_phrase` bag-of-words (not
    emitted by deces-backend). Root `match_all` `_score` and hit key order
    closed by L2b `72a32e6`.

## Out of scope

- `script_score`, `random_score`, `linear`/`exp` decay unless
  `buildRequest.ts` uses them.
- Any write to matchID clusters or the k8s migration.

## Merge status

- [x] L0 committed on `main`: `cd018b9` (local, not pushed).
- [x] L2/L4 quick compat fixes fast-forwarded on `main`: `4c1a83e`,
  `ea2cd47` (local, not pushed). L1 WIP on worktree branch
  `worktree-agent-af16e91d23b04f656` (`b4ebe9a`, uncompiled).
