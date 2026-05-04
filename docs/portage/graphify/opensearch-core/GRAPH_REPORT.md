# Graph Report - .  (2026-05-04)

## Corpus Check
- Large corpus: 238 files · ~215,629 words. Semantic extraction will be expensive (many Claude tokens). Consider running on a subfolder, or use --no-semantic to run AST-only.

## Summary
- 3813 nodes · 5996 edges · 81 communities detected
- Extraction: 100% EXTRACTED · 0% INFERRED · 0% AMBIGUOUS
- Token cost: 0 input · 0 output


## Input Scope
- Requested: all
- Resolved: all (source: cli)
- Included files: 238 · Candidates: recursive
- Excluded: 0 untracked · 0 ignored · 0 sensitive · 0 missing committed
## God Nodes (most connected - your core abstractions)
1. `InternalEngine` - 125 edges
2. `Engine` - 85 edges
3. `SearchRequestBuilder` - 53 edges
4. `UpdateRequest` - 51 edges
5. `QueryBuilders` - 50 edges
6. `AbstractSearchAsyncAction` - 47 edges
7. `SearchModule` - 47 edges
8. `SearchRequest` - 46 edges
9. `IndexRequest` - 45 edges
10. `FieldMapper` - 45 edges

## Surprising Connections (you probably didn't know these)
- `BackoffPolicy` --implements--> `Iterable`  [EXTRACTED]
  server/src/main/java/org/opensearch/action/bulk/BackoffPolicy.java →   _Bridges community 0 → community 10_
- `BulkItemResponse` --implements--> `Writeable`  [EXTRACTED]
  server/src/main/java/org/opensearch/action/bulk/BulkItemResponse.java →   _Bridges community 36 → community 0_
- `BulkItemResponse` --implements--> `StatusToXContentObject`  [EXTRACTED]
  server/src/main/java/org/opensearch/action/bulk/BulkItemResponse.java →   _Bridges community 36 → community 4_
- `Failure` --implements--> `ToXContentFragment`  [EXTRACTED]
  server/src/main/java/org/opensearch/action/bulk/BulkItemResponse.java →   _Bridges community 36 → community 16_
- `BulkProcessor` --implements--> `Closeable`  [EXTRACTED]
  server/src/main/java/org/opensearch/action/bulk/BulkProcessor.java →   _Bridges community 31 → community 13_

## Communities

### Community 0 - "Bulk Actions"
Cohesion: 0.01
Nodes (83): AbstractRunnable, ActionListener, ActionListenerResponseHandler, ActionRunnable, BiConsumer, BackoffPolicy, ConstantBackoff, ConstantBackoffIterator (+75 more)

### Community 1 - "Bulk Actions"
Cohesion: 0.01
Nodes (81): Accountable, AnalyzerWrapper, BaseNodesRequest, BulkItemRequest(), readPrimaryResponse(), BytesRefIterator, CheckedRunnable, Collector (+73 more)

### Community 2 - "Mapping Fields"
Cohesion: 0.01
Nodes (19): Delete, DeleteResult, Engine, EventListener, Get, GetResult, Index, IndexResult (+11 more)

### Community 3 - "Bulk Actions"
Cohesion: 0.02
Nodes (21): AcknowledgedRequest, ActionRequest, BaseNodesResponse, BulkRequest, CompositeIndicesRequest, Item, MultiGetRequest, Failure (+13 more)

### Community 4 - "Bulk Actions"
Cohesion: 0.02
Nodes (14): ActionResponse, BulkResponse, BulkItemResponse, DocumentField, GetResponse, MultiGetShardResponse, ClearScrollResponse, CreatePitResponse (+6 more)

### Community 5 - "REST Search"
Cohesion: 0.03
Nodes (5): EngineMergeScheduler, ExternalReaderManager, InternalEngine, OpenSearchConcurrentMergeScheduler, ReferenceManager

### Community 6 - "Query DSL"
Cohesion: 0.02
Nodes (10): AbstractQueryBuilder, MultiTermQueryBuilder, ExistsQueryBuilder, FuzzyQueryBuilder, MatchAllQueryBuilder, MatchNoneQueryBuilder, PrefixQueryBuilder, RangeQueryBuilder (+2 more)

### Community 7 - "REST Search"
Cohesion: 0.02
Nodes (23): BaseRestHandler, CreatePitRequest, DeletePitRequest, QueryRewriter, QueryRewriterRegistry, QueryBuilder, RestClearScrollAction, RestCountAction (+15 more)

### Community 8 - "Query DSL"
Cohesion: 0.03
Nodes (30): FieldValueConverter, BooleanFieldMapper, Builder, Builder, bitmapQuery(), Builder, createFields(), doubleRangeQuery() (+22 more)

### Community 9 - "Query DSL"
Cohesion: 0.03
Nodes (9): FieldMapper, Builder, PhraseFieldMapper, PhraseFieldType, PrefixFieldMapper, PrefixFieldType, TextFieldMapper, TextFieldType (+1 more)

### Community 10 - "Mapping Fields"
Cohesion: 0.05
Nodes (5): Iterable, Builder, CopyTo, FieldMapper, MultiFields

### Community 11 - "Search Actions"
Cohesion: 0.03
Nodes (5): IndicesRequest, QuerySearchRequest, Replaceable, CreatePitRequest, SearchRequest

### Community 12 - "Bulk Actions"
Cohesion: 0.03
Nodes (12): ActionRequestBuilder, BulkRequestBuilder, DeleteRequestBuilder, MultiGetRequestBuilder, IndexRequestBuilder, InstanceShardOperationRequestBuilder, ReplicationRequestBuilder, ClearScrollRequestBuilder (+4 more)

### Community 13 - "Mapping Fields"
Cohesion: 0.04
Nodes (6): AbstractIndexComponent, Closeable, DelegatingAnalyzerWrapper, MapperAnalyzerWrapper, MapperService, TranslogManager

### Community 14 - "Bulk Actions"
Cohesion: 0.03
Nodes (4): BulkShardRequest, DeleteRequest, IndexRequest, ReplicatedWriteRequest

### Community 15 - "Search Actions"
Cohesion: 0.03
Nodes (4): Item, QueryBuilders, Item, MultiSearchResponse

### Community 16 - "Search Actions"
Cohesion: 0.04
Nodes (7): BaseNodeResponse, Builder, DocumentMapper, GetAllPitNodeResponse, ListPitInfo, SearchResponseSections, ToXContentFragment

### Community 17 - "Query DSL"
Cohesion: 0.05
Nodes (9): QueryRewriter, BooleanFlatteningRewriter, ClauseAdder, MatchAllRemovalRewriter, MustNotToShouldRewriter, MustToFilterRewriter, ClauseAdder, TermsInfo (+1 more)

### Community 18 - "Mapping Fields"
Cohesion: 0.07
Nodes (6): Cloneable, Mapper, Builder, Nested, ObjectMapper, TypeParser

### Community 19 - "Query DSL"
Cohesion: 0.05
Nodes (6): BaseTermQueryBuilder, ComplementAwareQueryBuilder, MatchQueryBuilder, TermQueryBuilder, fromString(), TermsQueryBuilder

### Community 20 - "Query DSL"
Cohesion: 0.06
Nodes (8): Builder, clampToValidRange(), convert(), DateFieldMapper, DateFieldType, numericType(), toInstant(), type()

### Community 21 - "Bulk Actions"
Cohesion: 0.05
Nodes (18): ActionType, BulkAction, DeleteAction, GetAction, MultiGetAction, IndexAction, ClearScrollAction, CreatePitAction (+10 more)

### Community 22 - "Search Actions"
Cohesion: 0.07
Nodes (1): SearchRequestBuilder

### Community 23 - "Search Actions"
Cohesion: 0.09
Nodes (3): AbstractSearchAsyncAction, PendingExecutions, SearchPhaseContext

### Community 24 - "Search Actions"
Cohesion: 0.05
Nodes (9): Comparable, Countable, compare(), extractShardId(), FieldDocAndSearchHit, SearchResponseMerger, ShardIdAndClusterAlias, SearchShard (+1 more)

### Community 25 - "Index Actions"
Cohesion: 0.06
Nodes (3): InstanceShardOperationRequest, UpdateRequest, WriteRequest

### Community 26 - "Search Actions"
Cohesion: 0.05
Nodes (7): OpenSearchLogMessage, SearchRequestSlowLog, SearchRequestSlowLogMessage, SearchRequestStats, StatsHolder, SearchTaskRequestOperationsListener, SearchRequestOperationsListener

### Community 27 - "Search Actions"
Cohesion: 0.08
Nodes (1): SearchModule

### Community 28 - "Search Actions"
Cohesion: 0.08
Nodes (5): Response, CCSActionListener, SearchAsyncActionProvider, SearchTimeProvider, TransportSearchAction

### Community 29 - "Bulk Actions"
Cohesion: 0.09
Nodes (4): BulkOperation, ConcreteIndices, TransportBulkAction, BulkRequest

### Community 30 - "REST Search"
Cohesion: 0.07
Nodes (9): AbstractModule, ActionModule, DynamicActionRegistry, SearchShardTask, SearchTask, StreamTransportSearchAction, SearchBackpressureTask, TransportSearchAction (+1 more)

### Community 31 - "Bulk Actions"
Cohesion: 0.09
Nodes (5): BulkProcessor, Flush, Runnable, ClearScrollController, SearchScrollAsyncAction

### Community 32 - "Search Actions"
Cohesion: 0.06
Nodes (7): AbstractSearchAsyncAction, ArraySearchPhaseResults, CanMatchPreFilterSearchPhase, CanMatchSearchPhaseResults, SearchDfsQueryThenFetchAsyncAction, SearchQueryThenFetchAsyncAction, SearchPhaseResults

### Community 33 - "Query DSL"
Cohesion: 0.09
Nodes (5): MultiMatchQueryBuilder, parse(), parseField(), readFromStream(), writeTo()

### Community 34 - "PIT Scroll"
Cohesion: 0.07
Nodes (7): CollapsingTopDocsCollectorContext, EmptyTopDocsCollectorContext, ScrollingTopDocsCollectorContext, SimpleTopDocsCollectorContext, TopDocsCollectorContext, QueryCollectorContext, RescoringQueryCollectorContext

### Community 35 - "Search Actions"
Cohesion: 0.12
Nodes (4): ArraySearchPhaseResults, PendingReduces, QueryPhaseResultConsumer, ReduceTask

### Community 36 - "Bulk Actions"
Cohesion: 0.09
Nodes (4): BulkItemResponse, Failure, fromSourceType(), getSourceType()

### Community 37 - "Query DSL"
Cohesion: 0.12
Nodes (2): KeywordFieldMapper, KeywordFieldType

### Community 38 - "Query DSL"
Cohesion: 0.09
Nodes (1): AbstractQueryBuilder

### Community 39 - "Search Response"
Cohesion: 0.09
Nodes (1): QuerySearchResult

### Community 40 - "Get Actions"
Cohesion: 0.08
Nodes (5): DocRequest, GetRequest, MultiGetShardRequest, RealtimeRequest, SingleShardRequest

### Community 41 - "Search Actions"
Cohesion: 0.11
Nodes (3): ReducedQueryPhase, SearchPhaseController, TopDocsStats

### Community 42 - "Bulk Actions"
Cohesion: 0.15
Nodes (1): BulkPrimaryExecutionContext

### Community 43 - "Search Actions"
Cohesion: 0.09
Nodes (5): QueryPhaseExecutionException, SearchPhaseExecutionException, ShardSearchFailure, SearchException, ShardOperationFailedException

### Community 44 - "Search Actions"
Cohesion: 0.11
Nodes (5): DfsQueryPhase, ExpandSearchPhase, FetchSearchPhase, WrappingSearchAsyncActionPhase, SearchPhase

### Community 45 - "Search Actions"
Cohesion: 0.09
Nodes (2): SearchRequestContext, toString()

### Community 46 - "Search Actions"
Cohesion: 0.09
Nodes (2): CompositeListener, SearchRequestOperationsListener

### Community 47 - "Query DSL"
Cohesion: 0.12
Nodes (1): BoolQueryBuilder

### Community 48 - "Get Actions"
Cohesion: 0.1
Nodes (3): TransportGetAction, TransportShardMultiGetAction, TransportSingleShardAction

### Community 49 - "Index Actions"
Cohesion: 0.17
Nodes (5): ContextFields, lenientFromString(), Result, toString(), UpdateHelper

### Community 50 - "Search Actions"
Cohesion: 0.13
Nodes (1): SearchTransportService

### Community 51 - "Search Actions"
Cohesion: 0.13
Nodes (1): SearchPhaseContext

### Community 52 - "Search Response"
Cohesion: 0.17
Nodes (1): SearchProgressListener

### Community 53 - "Update Actions"
Cohesion: 0.18
Nodes (2): Builder, UpdateResponse

### Community 54 - "Mapping Fields"
Cohesion: 0.13
Nodes (3): Builder, SourceFieldMapper, MetadataFieldMapper

### Community 55 - "Bulk Actions"
Cohesion: 0.21
Nodes (3): Retry, RetryHandler, BulkResponse

### Community 56 - "Delete Actions"
Cohesion: 0.15
Nodes (3): DeleteSearchPipelineTransportAction, PutSearchPipelineTransportAction, TransportClusterManagerNodeAction

### Community 57 - "Query Queryphase"
Cohesion: 0.2
Nodes (2): DefaultQueryPhaseSearcher, QueryPhase

### Community 58 - "Update Actions"
Cohesion: 0.21
Nodes (2): TransportInstanceSingleOperationAction, TransportUpdateAction

### Community 59 - "Get Actions"
Cohesion: 0.17
Nodes (2): GetRequestBuilder, SingleShardOperationRequestBuilder

### Community 60 - "PIT Scroll"
Cohesion: 0.15
Nodes (3): SearchScrollQueryAndFetchAsyncAction, SearchScrollQueryThenFetchAsyncAction, SearchScrollAsyncAction

### Community 61 - "Bulk Actions"
Cohesion: 0.18
Nodes (1): Builder

### Community 62 - "Query DSL"
Cohesion: 0.27
Nodes (2): BooleanFieldType, TermBasedFieldType

### Community 63 - "Query Querycollectorcontext"
Cohesion: 0.23
Nodes (1): QueryCollectorContext

### Community 64 - "Search Actions"
Cohesion: 0.2
Nodes (1): SearchPhaseResults

### Community 65 - "Query Querycollectorarguments"
Cohesion: 0.2
Nodes (2): Builder, QueryCollectorArguments

### Community 66 - "Search Response"
Cohesion: 0.22
Nodes (2): StreamSearchTransportService, SearchTransportService

### Community 67 - "Search Response"
Cohesion: 0.36
Nodes (1): BottomSortValuesCollector

### Community 68 - "PIT Scroll"
Cohesion: 0.36
Nodes (1): CreatePitController

### Community 69 - "Get Actions"
Cohesion: 0.29
Nodes (2): GetSearchPipelineTransportAction, TransportClusterManagerNodeReadAction

### Community 70 - "PIT Scroll"
Cohesion: 0.25
Nodes (1): ParsedScrollId

### Community 71 - "PIT Scroll"
Cohesion: 0.29
Nodes (1): PitService

### Community 72 - "Action Search"
Cohesion: 0.29
Nodes (1): SearchContextId

### Community 73 - "Action Search"
Cohesion: 0.32
Nodes (2): StreamSearchActionListener, SearchActionListener

### Community 74 - "Bulk Actions"
Cohesion: 0.48
Nodes (1): BulkRequestParser

### Community 75 - "Bulk Actions"
Cohesion: 0.38
Nodes (3): TransportDeleteAction, TransportIndexAction, TransportSingleItemBulkWriteAction

### Community 76 - "Action Search"
Cohesion: 0.33
Nodes (2): ClusterManagerNodeReadRequest, GetSearchPipelineRequest

### Community 77 - "Action Search"
Cohesion: 0.4
Nodes (1): CountedCollector

### Community 78 - "Search Actions"
Cohesion: 0.4
Nodes (1): SearchRequestOperationsCompositeListenerFactory

### Community 81 - "Query Earlyterminatinglistener"
Cohesion: 0.67
Nodes (1): EarlyTerminatingListener

### Community 82 - "Query Rescoringquerycollectorcontext"
Cohesion: 0.67
Nodes (1): RescoringQueryCollectorContext

## Knowledge Gaps
- **9 isolated node(s):** `Fields`, `StatsHolder`, `ContextFields`, `Defaults`, `Values` (+4 more)
  These have ≤1 connection - possible missing edges or undocumented components.
- **Thin community `Search Actions`** (1 nodes): `SearchRequestBuilder`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Actions`** (1 nodes): `SearchModule`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query DSL`** (2 nodes): `KeywordFieldMapper`, `KeywordFieldType`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query DSL`** (1 nodes): `AbstractQueryBuilder`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Response`** (1 nodes): `QuerySearchResult`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Bulk Actions`** (1 nodes): `BulkPrimaryExecutionContext`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Actions`** (2 nodes): `SearchRequestContext`, `toString()`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Actions`** (2 nodes): `CompositeListener`, `SearchRequestOperationsListener`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query DSL`** (1 nodes): `BoolQueryBuilder`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Actions`** (1 nodes): `SearchTransportService`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Actions`** (1 nodes): `SearchPhaseContext`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Response`** (1 nodes): `SearchProgressListener`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Update Actions`** (2 nodes): `Builder`, `UpdateResponse`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query Queryphase`** (2 nodes): `DefaultQueryPhaseSearcher`, `QueryPhase`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Update Actions`** (2 nodes): `TransportInstanceSingleOperationAction`, `TransportUpdateAction`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Get Actions`** (2 nodes): `GetRequestBuilder`, `SingleShardOperationRequestBuilder`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Bulk Actions`** (1 nodes): `Builder`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query DSL`** (2 nodes): `BooleanFieldType`, `TermBasedFieldType`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query Querycollectorcontext`** (1 nodes): `QueryCollectorContext`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Actions`** (1 nodes): `SearchPhaseResults`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query Querycollectorarguments`** (2 nodes): `Builder`, `QueryCollectorArguments`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Response`** (2 nodes): `StreamSearchTransportService`, `SearchTransportService`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Response`** (1 nodes): `BottomSortValuesCollector`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `PIT Scroll`** (1 nodes): `CreatePitController`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Get Actions`** (2 nodes): `GetSearchPipelineTransportAction`, `TransportClusterManagerNodeReadAction`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `PIT Scroll`** (1 nodes): `ParsedScrollId`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `PIT Scroll`** (1 nodes): `PitService`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Action Search`** (1 nodes): `SearchContextId`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Action Search`** (2 nodes): `StreamSearchActionListener`, `SearchActionListener`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Bulk Actions`** (1 nodes): `BulkRequestParser`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Action Search`** (2 nodes): `ClusterManagerNodeReadRequest`, `GetSearchPipelineRequest`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Action Search`** (1 nodes): `CountedCollector`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Actions`** (1 nodes): `SearchRequestOperationsCompositeListenerFactory`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query Earlyterminatinglistener`** (1 nodes): `EarlyTerminatingListener`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Query Rescoringquerycollectorcontext`** (1 nodes): `RescoringQueryCollectorContext`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `InternalEngine` connect `REST Search` to `Bulk Actions`?**
  _High betweenness centrality (0.061) - this node is a cross-community bridge._
- **Why does `Engine` connect `Mapping Fields` to `Mapping Fields`?**
  _High betweenness centrality (0.041) - this node is a cross-community bridge._
- **Why does `SearchRequestBuilder` connect `Search Actions` to `REST Search`, `Bulk Actions`?**
  _High betweenness centrality (0.025) - this node is a cross-community bridge._
- **What connects `Fields`, `StatsHolder`, `ContextFields` to the rest of the system?**
  _9 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Bulk Actions` be split into smaller, more focused modules?**
  _Cohesion score 0.01 - nodes in this community are weakly interconnected._
- **Should `Bulk Actions` be split into smaller, more focused modules?**
  _Cohesion score 0.01 - nodes in this community are weakly interconnected._
- **Should `Mapping Fields` be split into smaller, more focused modules?**
  _Cohesion score 0.01 - nodes in this community are weakly interconnected._