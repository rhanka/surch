# Changelog

## [Unreleased]

### Added
- Added Lucene-compatible `SegmentInfos` generation and `segments_N` file-name handling in `surch-index`.
- Added empty Lucene `segments_N` commit write/read support with footer validation in `surch-index`.
- Added `SegmentInfos` commit user data round-trip support.
- Added `SegmentInfos` segment commit metadata round-trip support.
- Added Lucene-compatible `FieldInfo`/`FieldInfos` validation bootstrap in `surch-index`.
- Added Lucene-compatible `DataInput`/`DataOutput` string, string map, and string set encodings in `surch-store`.
- Added bounded Damerau-Levenshtein fuzzy distance primitives in `surch-search`.
- Added OpenSearch `_bulk` NDJSON parser bootstrap in `surch-api`.
- Added Lucene-compatible `CodecUtil` header/footer validation and CRC32 checksum primitives in `surch-codec`.
- Added Lucene-compatible `DataInput`/`DataOutput` VInt, VLong, and ZLong primitive encodings in `surch-store`.
- Added Rust parity vectors for Lucene variable-length integer boundary behavior.

### Changed
- Replaced the previous branch/spec roadmap with a clean OpenSearch + Lucene Rust portage plan.
- Added upstream reference tracking for cloned OpenSearch and Lucene commits.
- Added graphify reports for focused LuceneCore and OpenSearchCore reference corpora.
- Defined a golden-test-first autonomous execution model for function-level parity work.
- Added a clean restart housekeeping phase for archiving the prototype, stale worktrees, runtime artifacts, and old governance references before rebuilding from a blank workspace.

## [0.1.0] - 2026-04-25

### Added
- Initial Surch prototype. The OpenSearch/Lucene compatibility claims from this phase were superseded by the 2026-05-04 portage reset and now require golden parity proof.
- Governance framework with conductor `PLAN.md`, numbered branch files, spec-first execution, and subagent rules
- OpenSearch compatibility specs for index/document APIs, search/query DSL, security baseline, and MatchID Elastic parity exit criteria
- Persisted Write-Ahead Log replay with on-disk `wal.jsonl`
- Persisted segment storage with document round-trip recovery
- Mapping validation for MVP field types and analyzer declarations
- Analyzer behavior coverage for `standard`, `simple`, `stop`, and `keyword`
- Core query semantics for `term`, `terms`, `range`, `exists`, `bool`, `match`, `match_phrase`, `multi_match`, `prefix`, `wildcard`, and `fuzzy`
- Fuzzy transposition handling and prefix-length behavior up to distance 2
- OpenSearch-like index/document compatibility handlers
- OpenSearch-like `_search` request parsing with pagination, sorting, and unsupported `regexp` rejection
- NDJSON `_bulk` ingestion path with malformed input rejection
- Refresh and flush shard-summary responses
- API and crate-level compatibility tests for storage, indexer, search, and REST surface

### Features
- Index CRUD operations
- Document single and bulk indexing
- Query DSL (`match`, `match_phrase`, `multi_match`, `term`, `terms`, `range`, `exists`, `bool`, `prefix`, `wildcard`, `fuzzy`)
- Sorting and pagination
- TF-IDF and BM25 scoring

### Architecture
- Workspace with surch-core and surch-api crates
- Axum-based HTTP server
- Tokio async runtime
- parking_lot for concurrency
- Rustls-based API test client to avoid OpenSSL system dependency in dev tests
