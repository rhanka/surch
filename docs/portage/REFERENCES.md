# Portage References

Date: 2026-05-04

## Clones

OpenSearch:

- Path: `/tmp/surch-portage-references/opensearch`
- Commit: `fead3a928236b61f9c759c61e738b541a148ab9f`
- Full graphify detect: 12,952 files, approximately 9,050,705 words
- Full AST extraction: 148,786 nodes, 365,877 edges
- Full report finalization was too large for a practical interactive run, so a targeted graph was produced.

Lucene:

- Path: `/tmp/surch-portage-references/lucene`
- Commit: `7691b7ef9cfe3b87178646f4f32b3854afa0a567`
- Full graphify detect: 5,998 files, approximately 5,725,114 words
- Full AST extraction: 70,874 nodes, 150,878 edges
- Full report finalization was too large for a practical interactive run, so a targeted graph was produced.

## Targeted Graphify Outputs

Lucene core corpus:

- Corpus path: `/tmp/surch-portage-graph-corpora/lucene-core`
- Included files: 204
- Graph: 2,964 nodes, 4,701 edges
- Report copied to `docs/portage/graphify/lucene-core/GRAPH_REPORT.md`
- Runtime proof copied to `docs/portage/graphify/lucene-core/runtime.json`
- Full interactive graph remains at `/tmp/surch-portage-graph-corpora/lucene-core/.graphify/graph.html`
- Raw graph remains at `/tmp/surch-portage-graph-corpora/lucene-core/.graphify/graph.json`

OpenSearch core corpus:

- Corpus path: `/tmp/surch-portage-graph-corpora/opensearch-core`
- Included files: 238
- Graph: 3,813 nodes, 5,996 edges
- Report copied to `docs/portage/graphify/opensearch-core/GRAPH_REPORT.md`
- Runtime proof copied to `docs/portage/graphify/opensearch-core/runtime.json`
- Full interactive graph remains at `/tmp/surch-portage-graph-corpora/opensearch-core/.graphify/graph.html`
- Raw graph remains at `/tmp/surch-portage-graph-corpora/opensearch-core/.graphify/graph.json`

Both runtime proofs state `"runtime": "typescript"`.

## Graphify Findings Used In The Plan

Lucene god nodes:

- `IndexWriter`
- `SegmentInfos`
- `RegExp`
- `IndexWriterConfig`
- `FieldInfo`
- `FieldType`
- `SegmentCommitInfo`
- `IndexSearcher`
- `MemorySegmentIndexInput`
- `Automaton`

OpenSearch god nodes:

- `InternalEngine`
- `Engine`
- `SearchRequestBuilder`
- `UpdateRequest`
- `QueryBuilders`
- `AbstractSearchAsyncAction`
- `SearchModule`
- `SearchRequest`
- `IndexRequest`
- `FieldMapper`

These nodes define the first backlog anchors: storage/index writer, segment metadata, automata/fuzzy, engine lifecycle, query registry, action routing, request parsing, and field mapping.
