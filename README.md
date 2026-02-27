# Surch

A 100% Rust search engine replicating OpenSearch/Lucene functionality.

## Features

- **Indexation**: Full-text indexing with configurable analyzers
- **Search**: Query DSL with match, term, range, bool, fuzzy queries
- **Fuzzy Search**: Damerau-Levenshtein distance ≤ 2 (Lucene signature)
- **REST API**: OpenSearch/Elasticsearch compatible

## Quick Start

```bash
# Build
cargo build --release

# Run
cargo run --release

# Test
cargo test
```

## API Examples

### Create Index
```bash
curl -X PUT "localhost:9200/my-index"
```

### Index Document
```bash
curl -X PUT "localhost:9200/my-index/_doc/1" \
  -H "Content-Type: application/json" \
  -d '{"title": "Hello World", "content": "This is a test"}'
```

### Search
```bash
curl -X POST "localhost:9200/my-index/_search" \
  -H "Content-Type: application/json" \
  -d '{"query": {"match": {"content": "test"}}}'
```

### Fuzzy Search
```bash
curl -X POST "localhost:9200/my-index/_search" \
  -H "Content-Type: application/json" \
  -d '{"query": {"fuzzy": {"content": {"value": "tesh", "fuzziness": 2}}}}'
```

## Architecture

- **surch-core**: Core search engine (storage, indexer, search)
- **surch-api**: REST API server

## License

Apache 2.0
