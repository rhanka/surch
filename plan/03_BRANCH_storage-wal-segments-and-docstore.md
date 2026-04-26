# Branch: BR-03 - Storage WAL Segments And Docstore

## Objective
- Deliver a usable storage foundation: WAL append and replay, segment metadata, and document store read/write behavior for single-node MVP.

## Scope / Guardrails
- Storage only, with minimal shared type changes if required
- Do not implement search semantics here
- Keep persistence model simple and explicit

## Spec Sources
- Required:
  - `spec/SPEC_OS_INDEX_AND_DOCUMENT_APIS.md`
  - `spec/SPEC_SECURITY_AND_TESTING_BASELINE.md`

## Allowed Paths
- `surch-core/src/storage/**`
- `surch-core/src/common/**`
- `surch-core/tests/**`
- `plan/03_BRANCH_storage-wal-segments-and-docstore.md`

## Forbidden Paths
- `surch-core/src/search/**`
- `surch-core/src/indexer/**`
- `surch-api/**`
- `rules/**`

## Conditional Paths
- `spec/**` only if implementation reveals a storage assumption that changes documented behavior

## Dependency Gates
- [x] BR-01 syntax constraints reviewed
- [x] storage file layout approach agreed

## Environment Mapping
- Worktree: `tmp/br-03-storage-wal`
- Data path: `data/test/br-03/`
- Mode: local Rust + isolated data

## Plan / Lots / Todo

- [x] **Lot 0 - Read and confirm contract**
  - [x] Review current `wal.rs`, `segment.rs`, `reader.rs`, `writer.rs`, `index_store.rs`
  - [x] Confirm on-disk shape to keep MVP minimal

- [x] **Lot 1 - WAL append and replay**
  - [x] Make WAL append explicit and deterministic
  - [x] Add replay path for restart/recovery
  - [x] Add unit tests for append, flush, replay, and empty log
  - [x] Lot gate:
    - [x] `cargo test -p surch-core storage::wal`

- [ ] **Lot 2 - Segment metadata and docstore**
  - [x] Tighten segment metadata behavior
  - [x] Make doc write/read path deterministic
  - [x] Add unit tests for segment document persistence shape
  - [x] Lot gate:
    - [x] `cargo test -p surch-core storage::segment`

- [ ] **Lot 3 - Integration path**
  - [x] Add `surch-core/tests/storage_wal_recovery.rs`
  - [x] Add `surch-core/tests/storage_docstore_roundtrip.rs`
  - [x] Final gate:
    - [x] `cargo fmt --all`
    - [ ] `cargo clippy --workspace --all-targets --all-features`
    - [x] `cargo test -p surch-core`

## Feedback Loop
- Raise `attention` if a required shared type change expands beyond `surch-core/src/common/**`
- attention: WAL persistence shape for MVP is a fixed `wal/wal.jsonl` file with one JSON entry per line; segment persistence is now present and may evolve later if compaction changes the file layout.
- attention: branch-local storage verification is green on `surch-core`; workspace-level clippy remains pending because the crate still contains pre-existing warnings outside the BR-03 slice.

## Tests Required
- Unit: WAL append/replay, segment metadata, docstore round-trip
- Integration: `surch-core/tests/storage_wal_recovery.rs`, `surch-core/tests/storage_docstore_roundtrip.rs`

## Security Checks
- [x] No path traversal or unsafe file naming introduced
- [x] Corruption and empty-state handling reviewed

## Merge Checklist
- [x] WAL replay works
- [x] Docstore round-trip works
- [x] Storage tests pass
