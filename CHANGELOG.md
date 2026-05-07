# Changelog

## [Unreleased]

### Added
- Added an abstract OpenSearch replay runner with BAN tiny replay coverage and stricter BAN CSV validation.
- Added BAN tiny oracle import helpers with CSV parsing, acquisition profiles, and generated OpenSearch bulk fixtures.
- Added BM25-aware OpenSearch oracle replay comparison rules and BAN data.gouv dataset governance.
- Added an offline OpenSearch oracle fixture harness with dataset manifests, replay manifests, safe fixture loading, and normalized JSON response comparison.
- Added P0 document indexing pipeline from `SimpleAnalyzer` to stored fields and postings.
- Added BM25-backed term query execution over postings into `TopDocs`.
- Added OpenSearch-compatible document index bootstrap route.
- Added `MemoryDirectory` sync and metadata sync state tracking.
- Added StopAnalyzer bootstrap with stop filtering and Lucene-like position increments.
- Added live docs deletion mask bootstrap with idempotent deletes.
- Added TopDocs collector bootstrap with score ordering and tie-breaking.
- Added OpenSearch-compatible `_search` bootstrap route with match_all parsing.
- Added lowercase filtering and SimpleAnalyzer bootstrap for Lucene-like analysis.
- Added in-memory stored fields reader/writer bootstrap with deterministic document ordering.
- Added Lucene BM25 scoring formula bootstrap with validation and parity fixture.
- Added OpenSearch-compatible `_count` bootstrap route with match_all parsing.
- Added Lucene-like keyword and whitespace analyzer token-stream bootstrap with offset fixtures.
- Added Lucene-like in-memory lock factory acquire/release semantics.
- Added deterministic in-memory term dictionary and postings enumeration bootstrap.
- Added OpenSearch-compatible root endpoint and reusable error envelope.
- Added Lucene-compatible `SegmentInfos` generation and `segments_N` file-name handling in `surch-index`.
- Added empty Lucene `segments_N` commit write/read support with footer validation in `surch-index`.
- Added `SegmentInfos` commit user data round-trip support.
- Added `SegmentInfos` segment commit metadata round-trip support.
- Added Lucene-compatible `FieldInfo`/`FieldInfos` validation bootstrap in `surch-index`.
- Added deterministic bootstrap `FieldInfos` binary codec with local Lucene parity fixture.
- Added segment-scoped `FieldInfos` file wrapper for `_N.fnm` metadata.
- Added Lucene-compatible `DataInput`/`DataOutput` string, string map, and string set encodings in `surch-store`.
- Added Lucene-like in-memory `IndexInput`/`IndexOutput` primitives with checksum and local parity fixture.
- Added Lucene-like in-memory `Directory` file lifecycle primitives with local manifest fixture.
- Added `Directory` `IndexOutput` persistence and `IndexInput` reopening coverage with local manifest fixture.
- Added single-segment manifest assembly for `_N.fnm` and `segments_N` bootstrap bundles.
- Added bounded Damerau-Levenshtein fuzzy distance primitives in `surch-search`.
- Added fuzzy `AUTO`/fixed edit configuration parsing with local classic fuzzy fixture.
- Added deterministic fuzzy term expansion with prefix filtering and max expansions.
- Added exact/fuzzy query wrappers and fuzzy rewrite-to-term-query coverage.
- Added OpenSearch `_bulk` NDJSON parser bootstrap in `surch-api`.
- Added OpenSearch-like `_bulk` response builder with local NDJSON/JSON compatibility fixtures.
- Added Axum `_bulk` handler bootstrap with local HTTP compatibility fixtures.
- Added reusable Axum API router exposing the P0 `_bulk` route.
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
