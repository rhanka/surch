# Spec: Integral OpenSearch + Lucene Rust Portage

Date: 2026-05-04

## Scope

Surch must become a Rust implementation of the observable OpenSearch plus Lucene behavior used by OpenSearch-compatible clients. The project is not a clean-room search toy; every implemented function, API, parser, codec, query, or response contract must trace to an upstream source reference and a parity test.

The active implementation workspace starts from a blank Rust crate layout. Existing Surch prototype code is historical input only and must be archived or isolated before new porting work begins.

Initial compatibility target:

- OpenSearch commit `fead3a928236b61f9c759c61e738b541a148ab9f`
- Lucene commit `7691b7ef9cfe3b87178646f4f32b3854afa0a567`
- Rust 1.75+, edition 2021
- no `unsafe` unless documented, reviewed, and locally justified

## Non-Goals For The First Release

Distributed cluster replication, security plugins, snapshots, remote store, vector search, analytics dashboards, ingest pipelines, and every optional OpenSearch plugin are outside the first release. They remain backlog items with explicit upstream references.

The first release may be single-node, but it must render compatible OpenSearch responses for supported APIs.

## Lucene Porting Domains

Port in dependency order:

1. `store`: `DataInput`, `DataOutput`, `IndexInput`, `IndexOutput`, `Directory`, `FSDirectory`, `MMapDirectory`, locks, checksum, `IOContext`.
2. `codecs`: `CodecUtil`, headers, footers, CRC, `Codec`, postings/stored/docvalues/norms/live docs/compound abstractions.
3. `index metadata`: `FieldInfo(s)`, `SegmentInfo`, `SegmentCommitInfo`, `SegmentInfos`, file names, generations, commit points.
4. `terms and postings`: `Terms`, `TermsEnum`, `PostingsEnum`, impacts, block tree term dictionary, postings docs/positions/payloads.
5. `stored fields and doc values`: `_source`, stored fields, norms, numeric/sorted/sorted-set doc values.
6. `writer and reader`: `IndexWriter`, flush, commit, deletes, NRT reader reopen, `DirectoryReader`, `SegmentReader`, `LeafReader`.
7. `analysis`: `Analyzer`, `TokenStream`, `Tokenizer`, `StandardAnalyzer`, lowercase, stop filter, UAX#29 behavior.
8. `search`: `Query`, `Weight`, `Scorer`, `IndexSearcher`, collectors, sort, term/bool/phrase/multi-term queries.
9. `automaton and fuzzy`: automata, regexp, compiled automata, Levenshtein automata, `FuzzyQuery`.
10. `similarities`: BM25 first, then Boolean and Classic similarities.

Fuzzy compatibility is mandatory: maximum supported edit distance is `2`, default transpositions are enabled, and fuzzy search must enumerate terms through the term dictionary/automata path rather than comparing the query string to whole field text.

## OpenSearch Compatibility Domains

P0 REST surface:

- root info and index existence
- create/delete index
- mappings and settings
- index/create/get/source/delete/update document
- `_bulk`
- `_search` and `_count`

P0 Query DSL:

- `match_all`, `match_none`, `term`, `terms`, `range`, `exists`, `ids`, `bool`
- `match`, `multi_match`, `match_phrase`
- `prefix`, `wildcard`, `fuzzy`

P1 compatibility:

- `_mget`, `_msearch`, `_explain`, `_validate/query`, `_field_caps`, `_analyze`
- `_refresh`, `_flush`, `_stats`, `_segments`, `_cat/indices`, `_cluster/health`
- `query_string`, `simple_query_string`, `regexp`, `constant_score`, `dis_max`

P2 compatibility:

- scroll/PIT, templates, aliases, ingest/search pipelines, tasks, snapshots, data streams, nested/span/interval/function-score queries, geo, scripting.

## Golden Test Strategy

OpenSearch tests:

- derive API matrix from `rest-api-spec/api/*.json`
- derive scenarios from `rest-api-spec/test/**/*.yml`
- replay equivalent requests against upstream OpenSearch and Surch
- normalize non-deterministic fields: `took`, generated IDs unless asserted, timestamps, version strings, shard details
- compare status code, JSON envelope, error shape, required fields, hit totals, hit order, `_score` epsilon

Lucene tests:

- analyzer snapshots: tokens, positions, offsets, types
- storage/index tests: round-trip, corruption, checksum, commit, delete replay
- query tests: matching doc IDs, ordering, explanations, BM25 tolerances
- fuzzy tests: distances `0`, `1`, `2`, rejection above `2`, `AUTO`, `prefix_length`, `max_expansions`, transpositions
- codec tests: Rust-produced indexes accepted by a Java Lucene checker once the codec stage reaches binary parity

## Security Requirements

- all API inputs are bounded and validated
- JSON and NDJSON parsers must reject malformed input with OpenSearch-compatible errors
- bool depth, wildcard length, regexp determinization, fuzzy expansions, body size, pagination, and sort cardinality must have explicit limits
- logs must never expose secrets or entire user documents by default
- every public API boundary has negative tests

## Reset Requirements

Before any feature branch starts:

- the old roadmap files are removed from the active plan/spec directories
- stale branch worktrees are inventoried, checked for unique commits, then removed or archived
- current dirty changes are saved as a patch in `docs/portage/reset/`
- prototype crates are removed from the active Cargo workspace or moved behind an explicit legacy facade
- runtime and build artifacts are excluded from the active reset commit
- docs and rules no longer claim completed OpenSearch/Lucene compatibility without golden proof
- the conductor records the reset inventory and exact cleanup decisions

## Done Criteria

A feature is done only when:

- upstream reference is documented
- golden test fails before implementation and passes after
- module unit tests pass
- integration test covers API or cross-crate behavior
- docs/changelog are updated for public behavior
- no blocking feedback remains open
- conductor review accepts parity level and residual gaps
