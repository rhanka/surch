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
  - [ ] Candidate push-down through root `function_score` (inner query).
  - [ ] `range` on date keyword and `geo_distance` as filters over a
    candidate set instead of a full scan.
  - [ ] Guard: no full-scan path reachable from any `deces_v2` shape
    (counter + test).
    - [x] Counter `surch_dbg_full_scan_fallback_total{query_type}` on every
      `_source` full-scan fallback of `match_documents_for_index`, and a
      test proving fuzzy / bool-of-fuzzy shapes never reach it (`a480f2a`).
    - [ ] Extend the test to every `deces_v2` shape once the corpus
      generator exists (root `function_score` + aggs, `range`,
      `geo_distance` still full-scan today).
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
  - ES fuzzy deviations accepted (L1): BM25 kernel is Surch's (keeps
    `(k1+1)`, f64, lot L7) applied over the expanded terms with Lucene's
    per-term similarity boost and blended (max) doc freq per token — ranking
    follows ES's `top_terms_blended_freqs_N`, absolute scores do not; the
    `fuzzy` query still analyses its `value` and ANDs the tokens (ES takes
    one raw term); a `_source` re-check of a fuzzy clause (only under
    `must_not` or a should-disjunction superset) does not apply
    `max_expansions`; `match` `minimum_should_match` with `fuzziness` is not
    parsed.
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
  - [ ] `GET /_cat/indices/{index}?h=creation.date.string` + stored creation
    date (`/version` endpoint).
  - [ ] Analysis parity: `pattern_replace` char_filter (`alphanum`),
    configurable normalizer chain, real `deces_index.yml` mapping
    (DATE_* are `text` + `index_prefixes{4,6}`, `PRENOMS_NOM`, `PRENOM_NOM`,
    edge_ngram `max_gram` 10).
  - [x] Fuzzy semantics: OR over tokens (not AND), BM25 on expanded terms,
    `boost` honoured inside `match` objects (L1 `b746f0f`, tests `632c9fd`).
  - [ ] `date_histogram` keys as epoch ms + empty buckets.
  - [ ] Avoid whole-index materialisation for sort/agg on `.raw` and for
    `/version` (top-1 by date over 29 M docs).
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
  - [ ] Segment files + atomic manifest in a data directory; reopen at
    startup (FST, term entries, block directories, doc maps, live docs,
    mapping, settings, aliases).
  - [ ] Snapshot = copy of sealed segment files (S3 artefact built offline,
    loaded without re-indexing).
  - [ ] Proof: restart at 29 M under the target caps, time to first query.
- [ ] L7 — Lucene score parity (needed for the byte-identical bar).
  - [ ] BM25 as Lucene 9 `BM25Similarity`: f32, no `(k1+1)` factor,
    256-entry norm cache; Surch scores are ~2.2x ES today, which changes
    `min_score: 5` full-text result sets (`buildRequest.ts:506`).
  - [ ] Boolean scorer sums (double accumulation then f32 cast per scorer).
  - [ ] Fuzzy `TopTermsBlendedFreqScoringRewrite` (max_expansions 50,
    blended df/ttf, per-term boost `1 - ed/min(len)`).
  - [ ] `function_score` arithmetic as ES; `_score` serialized as f32 the
    way ES 8.6.1 (Jackson/JDK) prints it.
  - [ ] Tie order: index Surch in the `sort:_doc` order exported from the
    reference ES index (1 shard) so docid tie-breaks match.
  - [ ] Differential gate: `_score` per hit vs ES 8.6.1, with ES `explain`
    to localise each gap.

## Out of scope

- `script_score`, `random_score`, `linear`/`exp` decay unless
  `buildRequest.ts` uses them.
- Any write to matchID clusters or the k8s migration.

## Merge status

- [x] L0 committed on `main`: `cd018b9` (local, not pushed).
- [x] L2/L4 quick compat fixes fast-forwarded on `main`: `4c1a83e`,
  `ea2cd47` (local, not pushed). L1 WIP on worktree branch
  `worktree-agent-af16e91d23b04f656` (`b4ebe9a`, uncompiled).
