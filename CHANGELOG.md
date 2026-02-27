# Changelog

## [0.1.0] - 2024-01-01

### Added
- Initial release of Surch - a 100% Rust OpenSearch/Lucene clone
- Core storage layer with Write-Ahead Log (WAL) and segment management
- Basic indexer with analyzer pipeline (standard, simple, stop, keyword)
- Query DSL with match, term, range, bool queries
- Fuzzy search with Damerau-Levenshtein distance (≤ 2)
- REST API compatible with OpenSearch/Elasticsearch
- Document indexing and search endpoints

### Features
- Index CRUD operations
- Document single and bulk indexing
- Query DSL (match, term, range, bool, fuzzy, prefix, wildcard)
- Sorting and pagination
- TF-IDF and BM25 scoring

### Architecture
- Workspace with surch-core and surch-api crates
- Axum-based HTTP server
- Tokio async runtime
- parking_lot for concurrency
