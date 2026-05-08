# OpenSearch Lucene Rust Portage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Surch as an upstream-traceable Rust port of OpenSearch plus Lucene behavior.

**Architecture:** Split the prototype into focused crates for types, analysis, codec, store, index, search, and API. Every implementation ticket starts with an upstream reference and a golden parity test.

**Tech Stack:** Rust 1.75+, edition 2021, Cargo workspace, Axum for REST, OpenSearch REST/YAML fixtures, Java Lucene/OpenSearch golden oracles, graphify reference reports.

---

### Task 0: Clean Restart Housekeeping

**Files:**
- Create: `docs/portage/reset/RESET_INVENTORY.md`
- Create: `docs/portage/reset/git-status-before-reset.txt`
- Create: `docs/portage/reset/git-diff-stat-before-reset.txt`
- Create: `docs/portage/reset/git-worktrees-before-reset.txt`
- Create: `docs/portage/reset/prototype-dirty-worktree.patch`
- Move: `surch-core/` to `archive/legacy-prototype/surch-core/`
- Move: `surch-api/` to `archive/legacy-prototype/surch-api/`
- Move: `tests/matchid_parity/` to `archive/legacy-prototype/matchid/tests/`
- Move: `scripts/matchid_*` to `archive/legacy-prototype/matchid/`
- Modify: `README.md`
- Modify: `rules/MASTER.md`

- [ ] Record the dirty state before cleanup.

Run:

```bash
mkdir -p docs/portage/reset
git status --short > docs/portage/reset/git-status-before-reset.txt
git diff --stat > docs/portage/reset/git-diff-stat-before-reset.txt
git worktree list > docs/portage/reset/git-worktrees-before-reset.txt
```

Expected: the three reset inventory files exist under `docs/portage/reset/`.

- [ ] Save the current prototype patch.

Run:

```bash
git diff --binary > docs/portage/reset/prototype-dirty-worktree.patch
```

Expected: `docs/portage/reset/prototype-dirty-worktree.patch` contains the current uncommitted prototype/docs diff.

- [ ] Create the archive branch.

Run:

```bash
git branch archive/prototype-before-portage-2026-05-04 develop
```

Expected: `git branch --list archive/prototype-before-portage-2026-05-04` prints the archive branch.

- [ ] Validate stale worktrees before removal.

Run:

```bash
git worktree list
git branch --contains feature/BR-01-spec-harvest-index-doc-api
git branch --contains feature/BR-02-spec-harvest-search-query-dsl
git branch --contains feature/BR-03-storage-wal-segments-docstore
git branch --contains feature/BR-04-indexer-mappings-analyzers-bulk
git branch --contains feature/BR-05-search-query-exec-fuzzy
git branch --contains feature/BR-06-api-index-document-compat
```

Expected: every old feature branch is reachable from a named branch or explicitly recorded in `RESET_INVENTORY.md`.

- [ ] Remove stale old-plan worktrees after conductor confirmation.

Run after confirmation:

```bash
git worktree remove .worktrees/br-01-spec-harvest
git worktree remove .worktrees/br-02-spec-harvest
git worktree remove .worktrees/br-03-storage-wal
git worktree remove .worktrees/br-04-indexer
git worktree remove .worktrees/br-05-search-fuzzy
git worktree remove .worktrees/br-06-api-index-doc
```

Expected: `git worktree list` no longer shows `.worktrees/br-*`.

- [ ] Archive the prototype implementation from the active workspace.

Run:

```bash
mkdir -p archive/legacy-prototype
git mv surch-core archive/legacy-prototype/surch-core
git mv surch-api archive/legacy-prototype/surch-api
```

Expected: `Cargo.toml` no longer references `surch-core` or `surch-api` after Task 2 rewrites the workspace.

- [ ] Archive MatchID-specific prototype harnesses.

Run:

```bash
mkdir -p archive/legacy-prototype/matchid
git mv tests/matchid_parity archive/legacy-prototype/matchid/tests
git mv scripts/matchid_* archive/legacy-prototype/matchid/
```

Expected: default `tests/` and `scripts/` no longer contain MatchID-specific prototype files.

- [ ] Remove local runtime/build artifacts.

Run:

```bash
rm -rf target data .worktrees/runtime-surch
```

Expected: `git status --short --ignored` shows no tracked deletion for those generated artifacts.

- [ ] Rewrite stale governance references.

Run:

```bash
rg -n "SPEC_OS_|SPEC_MATCHID_|BRANCH_|SUBAGENT_PROMPT|SPEC_EVOL" PLAN.md rules docs plan spec README.md CHANGELOG.md
```

Expected: remaining hits are either absent or listed in `docs/portage/reset/RESET_INVENTORY.md` with a migration decision.

- [ ] Commit the cleanup baseline.

Run:

```bash
git add PLAN.md plan spec docs CHANGELOG.md README.md rules archive Cargo.toml
git commit -m "chore(portage): reset repository for opensearch lucene port"
```

Expected: reset commit contains only governance, archive moves, cleanup inventory, and blank-workspace preparation.

### Task 1: Reference Ledger And Backlog Generator

**Files:**
- Create: `tools/portage-ledger/README.md`
- Create: `tests/lucene_parity/README.md`
- Create: `tests/opensearch_compat/README.md`
- Modify: `Cargo.toml`

- [ ] Create the reference ledger format from `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`.
- [ ] Generate initial discovered tickets from Lucene selected classes and OpenSearch REST specs.
- [ ] Add a CI check that rejects tickets without `upstream_ref`, owner, gates, and golden tests.
- [ ] Commit with `docs(portage): add upstream reference ledger`.

### Task 2: Workspace Reset

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/surch-types/Cargo.toml`
- Create: `crates/surch-analysis/Cargo.toml`
- Create: `crates/surch-codec/Cargo.toml`
- Create: `crates/surch-store/Cargo.toml`
- Create: `crates/surch-index/Cargo.toml`
- Create: `crates/surch-search/Cargo.toml`
- Create: `crates/surch-api/Cargo.toml`

- [ ] Add the target crates with empty compileable libraries.
- [ ] Confirm Task 0 archived the existing prototype outside the active workspace.
- [ ] Run `cargo fmt --all`.
- [ ] Run `cargo test --workspace`.
- [ ] Commit with `chore(workspace): create portage crate layout`.

### Task 3: Lucene Store And Codec Foundation

**Files:**
- Create under `crates/surch-store/src/`
- Create under `crates/surch-codec/src/`
- Create under `tests/lucene_parity/store/`

- [ ] Write Java-generated golden fixtures for `DataInput/DataOutput`.
- [ ] Port vint, vlong, zig-zag, strings, maps, sets.
- [ ] Port `IndexInput`, `IndexOutput`, `Directory`, locks, checksum.
- [ ] Port `CodecUtil` headers, footers, CRC validation.
- [ ] Run targeted store/codec tests.
- [ ] Commit with `feat(storage): port lucene store primitives`.

### Task 4: Lucene Segment Metadata

**Files:**
- Create under `crates/surch-codec/src/segment/`
- Create under `crates/surch-index/src/metadata/`
- Create under `tests/lucene_parity/segment/`

- [ ] Port `FieldInfo(s)`, `SegmentInfo`, `SegmentCommitInfo`, `SegmentInfos`.
- [ ] Add golden fixtures for file names, generations, and `segments_N`.
- [ ] Add corruption and version mismatch tests.
- [ ] Commit with `feat(storage): add lucene segment metadata`.

### Task 5: Lucene Index Core

**Files:**
- Create under `crates/surch-index/src/`
- Create under `tests/lucene_parity/index/`

- [ ] Port term dictionary interfaces, `TermsEnum`, `PostingsEnum`.
- [ ] Add postings docs/frequencies/positions/offsets.
- [ ] Add stored fields, live docs, and doc values foundations.
- [ ] Add reader round-trip tests.
- [ ] Commit with `feat(indexer): add lucene index primitives`.

### Task 6: Analysis, Search, And Fuzzy

**Files:**
- Create under `crates/surch-analysis/src/`
- Create under `crates/surch-search/src/`
- Create under `tests/lucene_parity/search/`

- [ ] Port analyzer/token stream model and `StandardAnalyzer` behavior.
- [ ] Port query, weight, scorer, collector, top docs, and BM25 foundations.
- [ ] Port automata and `FuzzyQuery` with maximum edit distance 2.
- [ ] Add fuzzy `AUTO`, transposition, prefix, and expansion golden tests.
- [ ] Commit with `feat(search): add lucene query and fuzzy core`.

### Task 7: OpenSearch API Compatibility P0

**Files:**
- Create under `crates/surch-types/src/opensearch/`
- Create under `crates/surch-api/src/`
- Create under `tests/opensearch_compat/`

- [ ] Port root/index/mapping/settings document APIs.
- [ ] Port `_bulk`, `_search`, and `_count` P0 contracts.
- [ ] Implement OpenSearch-compatible error envelopes.
- [ ] Replay upstream REST fixtures for status, shape, hits, order, and score tolerance.
- [ ] Commit with `api(search): add opensearch p0 compatibility`.

### Task 8: Security, Performance, And Release Gates

**Files:**
- Create under `tests/security/`
- Create under `tests/perf/`
- Modify: CI configuration when present

- [ ] Add body size, NDJSON, bool depth, wildcard, regexp, fuzzy expansion, and pagination negative tests.
- [ ] Add performance baseline harness for indexing and search.
- [ ] Add dependency audit/deny tooling.
- [ ] Commit with `security(api): add compatibility safety gates`.

### Task 9: Demo Subproject And BAN Comparison Harness

**Status:** design direction pending user validation before scaffolding `demo/`.

**Files:**
- Create under `demo/`
- Create or modify under `docs/poc/`
- Create or modify under `crates/surch-demo/` or a future benchmark crate
- Optionally create shell-only lifecycle helpers under `scripts/bench/`

- [ ] Create `demo/` as a TypeScript-only SvelteKit/Svelte 5 app using `@sveltejs/adapter-node`.
- [ ] Keep Python forbidden in the demo, backend, benchmark, and data tooling.
- [ ] Use SvelteKit `+server.ts` endpoints as the demo backend for the first version.
- [ ] Configure engine targets with `SURCH_URL` and `OPENSEARCH_URL`.
- [ ] Expose a fixed BAN demo surface:
  - `GET /api/engines`
  - `GET /api/health`
  - `GET /api/demo/fixture`
  - `POST /api/demo/reset`
  - `POST /api/count`
  - `POST /api/search`
  - `POST /api/compare`
- [ ] Build the first UI as an operational demo, not a landing page:
  - engine status for Surch and OpenSearch;
  - switcher for `Surch`, `OpenSearch`, and `Compare`;
  - BAN load/reset action;
  - predefined queries for count, match label, bool address, and fuzzy typo;
  - compact JSON editor;
  - result panel with total hits, IDs, raw JSON, and normalized diff in compare mode.
- [ ] Add strict backend validation:
  - engine enum only;
  - index fixed to `ban_tiny` for V1;
  - request body size limits;
  - short upstream timeouts;
  - no browser-side direct OpenSearch proxy;
  - no arbitrary upstream URLs.
- [ ] Add demo gates:
  - TypeScript/Svelte check;
  - unit tests for engine selection, config validation, response normalization, and BAN NDJSON parsing;
  - API tests with mocked upstream engines;
  - optional real-engine smoke tests gated by `SURCH_URL` and `OPENSEARCH_URL`;
  - Playwright flow for load, search, engine switch, and compare mode.
- [ ] Add or reuse a Surch HTTP server binary before the demo depends on `SURCH_URL`.
- [ ] Commit with `feat(demo): add ban engine switch demo`.

#### BAN OpenSearch vs Surch Benchmark Positioning

The BAN benchmark is a compatibility and reproducibility benchmark until Surch
uses the complete indexing, storage, scoring, and HTTP server path. It must not
be published as an engine-performance comparison while Surch is still measured
as an in-process in-memory API router.

- [ ] Reuse `tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson` and `tests/opensearch_compat/oracle/replays/ban_tiny_search.json`.
- [ ] Measure the same operation sequence:
  - `PUT /ban_tiny`
  - `POST /_bulk`
  - `POST /ban_tiny/_refresh`
  - `GET /ban_tiny/_count`
  - `POST /ban_tiny/_search` for match, bool, and fuzzy requests.
- [ ] For OpenSearch, orchestrate a single-node local runtime with shell only:
  - fixed image/version or digest;
  - security disabled for local benchmark;
  - fixed heap;
  - healthcheck before loading;
  - index cleanup before each measured run.
- [ ] For Surch, report runtime mode explicitly:
  - `Surch in-process` for the current smoke benchmark;
  - `Surch HTTP` only after a server binary exists.
- [ ] Record metrics:
  - ingestion duration and docs/s;
  - min, p50, p95, p99, max latency;
  - error count;
  - total hits and top hit ID;
  - host, OS, CPU, memory, OpenSearch version/image, heap, Surch commit, Rust profile, dataset size.
- [ ] Reject benchmark output when responses do not pass the oracle.
- [ ] Publish guardrails with every result:
  - `ban_tiny` is a 3-document smoke dataset;
  - no global Surch/OpenSearch ratio until runtime paths are symmetric;
  - no production throughput claim;
  - no scoring comparison while the Surch API path returns `max_score: null`.
- [ ] Commit with `test(perf): add ban reproducibility benchmark harness`.
