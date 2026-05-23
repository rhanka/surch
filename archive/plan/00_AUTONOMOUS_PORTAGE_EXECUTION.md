# Autonomous Portage Execution Plan

Date: 2026-05-04

> Required sub-skill for implementation: `superpowers:subagent-driven-development` for independent branch work, or `superpowers:executing-plans` for inline batches.

## Goal

Turn the upstream OpenSearch and Lucene references into an executable Rust porting backlog where subagents can implement one parity feature at a time without guessing architecture or acceptance criteria.

## Backlog Schema

Each ticket must use this shape:

```yaml
id: LUCENE-store-DataInput-001
title: Port DataInput variable-length integer decoding
owner: StorageEngine
priority: Critical
upstream_ref:
  repo: lucene
  commit: 7691b7ef9cfe3b87178646f4f32b3854afa0a567
  files:
    - lucene/core/src/java/org/apache/lucene/store/DataInput.java
  symbols:
    - readVInt
    - readVLong
    - readZLong
parity_level: P1 behavior
dependencies: []
allowed_paths:
  - crates/surch-store/**
  - crates/surch-codec/**
  - tests/lucene_parity/**
forbidden_paths:
  - surch-api/**
golden_tests_required:
  - Java fixture emits encoded bytes and expected decoded values
  - Rust test consumes fixture and matches Java behavior
gates:
  - cargo fmt --all
  - cargo test -p surch-store data_input
  - cargo clippy --workspace --all-targets --all-features
status: discovered
```

## Phase -1: Clean Restart Housekeeping

Owner: Conductor

Purpose: make the active repository intentionally blank for the new port without losing recoverability.

Steps:

1. Record the current state:

```bash
mkdir -p docs/portage/reset
git status --short > docs/portage/reset/git-status-before-reset.txt
git diff --stat > docs/portage/reset/git-diff-stat-before-reset.txt
git worktree list > docs/portage/reset/git-worktrees-before-reset.txt
```

2. Save the dirty prototype patch:

```bash
git diff --binary > docs/portage/reset/prototype-dirty-worktree.patch
```

3. Create an archive branch from the current `develop` tip before destructive cleanup:

```bash
git branch archive/prototype-before-portage-2026-05-04 develop
```

4. Check stale worktrees before removal:

```bash
git worktree list
git branch --contains feature/BR-01-spec-harvest-index-doc-api
git branch --contains feature/BR-02-spec-harvest-search-query-dsl
git branch --contains feature/BR-03-storage-wal-segments-docstore
git branch --contains feature/BR-04-indexer-mappings-analyzers-bulk
git branch --contains feature/BR-05-search-query-exec-fuzzy
git branch --contains feature/BR-06-api-index-document-compat
```

5. Remove stale old-plan worktrees only after the conductor confirms they are archived:

```bash
git worktree remove .worktrees/br-01-spec-harvest
git worktree remove .worktrees/br-02-spec-harvest
git worktree remove .worktrees/br-03-storage-wal
git worktree remove .worktrees/br-04-indexer
git worktree remove .worktrees/br-05-search-fuzzy
git worktree remove .worktrees/br-06-api-index-doc
```

6. Delete runtime/build artifacts from the active tree:

```bash
rm -rf target data .worktrees/runtime-surch
```

7. Remove or isolate prototype crates before blank workspace creation:

```bash
mkdir -p archive/legacy-prototype
git mv surch-core archive/legacy-prototype/surch-core
git mv surch-api archive/legacy-prototype/surch-api
```

8. Remove old generated parity scripts and fixtures from the default path unless they are reintroduced as golden harness inputs:

```bash
mkdir -p archive/legacy-prototype/matchid
git mv scripts/matchid_* archive/legacy-prototype/matchid/
git mv tests/matchid_parity archive/legacy-prototype/matchid/tests
```

9. Rewrite governance references:

```bash
rg -n "SPEC_OS_|SPEC_MATCHID_|BRANCH_|SUBAGENT_PROMPT|SPEC_EVOL" PLAN.md rules docs plan spec README.md CHANGELOG.md
```

Every hit must be either removed, rewritten to the new plan/spec, or documented in `docs/portage/reset/RESET_INVENTORY.md`.

10. Exit criteria:

```bash
git status --short
```

Expected result: only the intentional reset files, archived prototype moves, deleted old plans/specs, and new portage docs are present.

## Phase 0: Reference Harness And Blank Workspace Reset

Owner: Conductor

- Create `crates/` workspace skeleton.
- Confirm Phase -1 has archived or isolated the existing prototype.
- Add `tests/lucene_parity/` and `tests/opensearch_compat/`.
- Add scripts that can run upstream Java/OpenSearch or consume recorded golden fixtures.
- Generate the first backlog from upstream AST symbols and OpenSearch REST specs.
- Require every branch plan to list exact upstream references.

Exit criteria:

- a `cargo test --workspace` baseline exists
- first Lucene golden fixture passes against Java and fails against unimplemented Rust
- first OpenSearch REST fixture passes against OpenSearch and fails against Surch

## Phase 1: Lucene Store, Codec, Metadata

Owner: StorageEngine

Critical tickets:

- `DataInput/DataOutput`: vint, vlong, zig-zag, string, map/set encoding.
- `IndexInput/IndexOutput`: random access, slices, clone semantics, checksum.
- `Directory`: filesystem implementation, locks, temp files, sync.
- `CodecUtil`: headers, footers, magic, version checks, CRC validation.
- `FieldInfo(s)`, `SegmentInfo`, `SegmentCommitInfo`, `SegmentInfos`.

Golden tests:

- Java-generated binary fixtures for primitive encodings.
- corrupt footer/header fixtures.
- `segments_N` metadata fixtures.

## Phase 2: Lucene Index And Persistence

Owner: StorageEngine + Indexer

Critical tickets:

- term dictionary and `TermsEnum`
- postings docs/frequencies/positions/offsets
- stored fields and `_source`
- numeric/sorted/sorted-set doc values
- live docs, hard deletes, commit points
- writer flush/commit and reader reopen

Golden tests:

- Java indexes read by Rust where format support exists.
- Rust indexes validated by a Java checker once writer parity exists.
- round-trip index/search fixtures per field type.

## Phase 3: Lucene Analysis And Search

Owner: Indexer + SearchEngine

Critical tickets:

- token stream model
- `StandardAnalyzer` UAX#29 behavior
- term, boolean, phrase, prefix, wildcard, regexp, automaton, fuzzy queries
- BM25 scoring with tolerance
- collectors, sort, pagination, explanations

Golden tests:

- analyzer token snapshots
- query doc IDs and score order
- fuzzy edit-distance matrix with `AUTO`, prefix, expansions, transpositions

## Phase 4: OpenSearch API Contracts

Owner: APIServer

Critical tickets:

- route table for P0 endpoints
- request parsing and validation
- OpenSearch error envelopes
- response rendering for `_shards`, `_seq_no`, `_primary_term`, `_version`, `result`, `hits`
- mappings/settings model

Golden tests:

- REST spec fixtures from upstream YAML.
- negative tests for malformed JSON, malformed NDJSON, unknown params, unsupported features.

## Phase 5: Documents, Bulk, Search

Owner: APIServer + StorageEngine + SearchEngine

Critical tickets:

- index/create/get/source/delete/update
- `_bulk` strict NDJSON and item-level errors
- refresh visibility
- `_search` and `_count`
- source filtering
- sorting, pagination, `track_total_hits`
- P0 Query DSL parser and execution

Golden tests:

- OpenSearch replay harness comparing status, body shape, hit totals, order, and score epsilon.
- MatchID parity fixtures retained as external client regression tests.

## Phase 6: P1/P2 Expansion

Owner: Conductor assigns by dependency

Backlog groups:

- `_mget`, `_msearch`, `_field_caps`, `_analyze`
- admin compatibility: refresh, flush, stats, segments, cat indices, cluster health
- query string, simple query string, regexp, dis_max, constant score
- PIT/scroll, aliases, templates, ingest/search pipelines

Each group starts only when its LuceneCore dependencies are done.

## Feedback Loop

Feedback item states:

```text
open -> assigned -> fix_in_progress -> ready_for_review -> resolved | deferred | blocked
```

Blocking types:

- `CODE_REVIEW`
- `INTEGRATION_FAIL`
- `SPEC_MISMATCH`
- `SECURITY_ALERT`
- `BLOCKED`

`SECURITY_ALERT` and `SPEC_MISMATCH` block merge unless the conductor documents a deferral.

## Branch Policy

- one branch per feature ticket
- one feature per subagent
- PR target is `develop`
- no direct commit to `main`
- max four active subagents
- branch must update changelog or parity ledger when public behavior changes
