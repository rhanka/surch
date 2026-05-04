# Graph Report - .  (2026-05-04)

## Corpus Check
- Large corpus: 204 files · ~221,403 words. Semantic extraction will be expensive (many Claude tokens). Consider running on a subfolder, or use --no-semantic to run AST-only.

## Summary
- 2964 nodes · 4701 edges · 87 communities detected
- Extraction: 100% EXTRACTED · 0% INFERRED · 0% AMBIGUOUS
- Token cost: 0 input · 0 output


## Input Scope
- Requested: all
- Resolved: all (source: cli)
- Included files: 204 · Candidates: recursive
- Excluded: 0 untracked · 0 ignored · 7 sensitive · 0 missing committed
## God Nodes (most connected - your core abstractions)
1. `IndexWriter` - 164 edges
2. `SegmentInfos` - 55 edges
3. `RegExp` - 48 edges
4. `IndexWriterConfig` - 44 edges
5. `FieldInfo` - 41 edges
6. `FieldType` - 39 edges
7. `SegmentCommitInfo` - 39 edges
8. `IndexSearcher` - 39 edges
9. `MemorySegmentIndexInput` - 36 edges
10. `Automaton` - 31 edges

## Surprising Connections (you probably didn't know these)
- `Analyzer` --implements--> `Closeable`  [EXTRACTED]
  lucene/core/src/java/org/apache/lucene/analysis/Analyzer.java →   _Bridges community 31 → community 1_
- `StringTokenStream` --inherits--> `TokenStream`  [EXTRACTED]
  lucene/core/src/java/org/apache/lucene/analysis/Analyzer.java →   _Bridges community 31 → community 5_
- `FieldInfos` --implements--> `Iterable`  [EXTRACTED]
  lucene/core/src/java/org/apache/lucene/index/FieldInfos.java →   _Bridges community 23 → community 5_
- `IndexWriter` --implements--> `Closeable`  [EXTRACTED]
  lucene/core/src/java/org/apache/lucene/index/IndexWriter.java →   _Bridges community 0 → community 1_
- `IndexWriter` --implements--> `Accountable`  [EXTRACTED]
  lucene/core/src/java/org/apache/lucene/index/IndexWriter.java →   _Bridges community 0 → community 4_

## Communities

### Community 0 - "Index Writer"
Cohesion: 0.03
Nodes (27): AddIndexesMergeSource, afterSegmentsFlushed(), deleteUnusedFiles(), DocModifier, DocStats, EventQueue, flushFailed(), getNextMerge() (+19 more)

### Community 1 - "Store IO"
Cohesion: 0.02
Nodes (26): AutomatonProvider, Closeable, FilterDirectory, FilterIndexOutput, IOException, Scorable, Collector, Query (+18 more)

### Community 2 - "Index Writer"
Cohesion: 0.02
Nodes (14): Checksum, ChecksumIndexInput, DataInput, DataOutput, IndexOutput, Provider, SortField, SortFieldProvider (+6 more)

### Community 3 - "Terms Postings"
Cohesion: 0.02
Nodes (19): PointTransitions, Comparable, Builder, PhraseQuery, PostingsAndFreq, BM25Scorer, BM25Similarity, BooleanSimilarity (+11 more)

### Community 4 - "Store IO"
Cohesion: 0.03
Nodes (10): Accountable, CompiledAutomaton, DState, NFARunAutomaton, RunAutomaton, Transition, Term, AutomatonQuery (+2 more)

### Community 5 - "Field Metadata"
Cohesion: 0.03
Nodes (11): Document, BinaryTokenStream, Field, StringTokenStream, FieldInvertState, IndexableField, IndexableField, Iterable (+3 more)

### Community 6 - "Similarity Scoring"
Cohesion: 0.03
Nodes (7): Axiomatic, AxiomaticF1EXP, AxiomaticF1LOG, AxiomaticF2EXP, AxiomaticF2LOG, AxiomaticF3EXP, AxiomaticF3LOG

### Community 7 - "Segment Metadata"
Cohesion: 0.03
Nodes (7): BaseCompositeReader, StoredFieldsFormat, Directory, DirectoryReader, SegmentInfo, BaseDirectory, FilterDirectory

### Community 8 - "Stored Fields"
Cohesion: 0.04
Nodes (12): ByteRunAutomaton, CharacterRunAutomaton, TooComplexToDeterminizeException, ByteRunnable, IndexReader, RunAutomaton, RuntimeException, IndexSearcher (+4 more)

### Community 9 - "Store IO"
Cohesion: 0.05
Nodes (6): Arena, MemorySegmentAccessInput, MemorySegmentIndexInput, MultiSegmentImpl, SingleSegmentImpl, RefCountedSharedArena

### Community 10 - "Similarity Scoring"
Cohesion: 0.04
Nodes (10): BasicStats, Axiomatic, DFISimilarity, DFRSimilarity, IBSimilarity, CollectionModel, DefaultCollectionModel, LMSimilarity (+2 more)

### Community 11 - "Index Writer"
Cohesion: 0.03
Nodes (5): StringField, TextField, Field, IndexWriterConfig, LiveIndexWriterConfig

### Community 12 - "Index Writer"
Cohesion: 0.07
Nodes (3): FindSegmentsFile, SegmentInfos, SegmentCommitInfo

### Community 13 - "Segment Metadata"
Cohesion: 0.05
Nodes (8): Codec, Holder, DocValuesFormat, Holder, Holder, PostingsFormat, NamedSPI, NamedSPILoader

### Community 14 - "Store IO"
Cohesion: 0.05
Nodes (14): FSLockFactory, Lock, LockFactory, FSLockFactory, NativeFSLock, NativeFSLockFactory, NoLock, NoLockFactory (+6 more)

### Community 15 - "Store IO"
Cohesion: 0.06
Nodes (5): StoredFields, IndexInput, ByteBuffersIndexInput, ChecksumIndexInput, FilterIndexInput

### Community 16 - "Automata Fuzzy"
Cohesion: 0.11
Nodes (2): MakeRegexGroup, RegExp

### Community 17 - "Terms Postings"
Cohesion: 0.07
Nodes (7): BooleanClause, Query, BooleanQuery, Builder, TermQuery, TermWeight, Weight

### Community 18 - "Terms Postings"
Cohesion: 0.06
Nodes (8): State, StringsToAutomaton, BytesRefIterator, postings(), prepareSeekExact(), seekCeil(), seekExact(), TermsEnum

### Community 19 - "Store IO"
Cohesion: 0.06
Nodes (7): BaseDirectory, OutputStreamIndexOutput, apply(), ByteBuffersDirectory, FileEntry, FSDirectory, FSIndexOutput

### Community 20 - "Automata Fuzzy"
Cohesion: 0.09
Nodes (4): Automaton, Builder, swap(), swapOne()

### Community 21 - "Doc Values"
Cohesion: 0.07
Nodes (1): FieldInfo

### Community 22 - "Terms Postings"
Cohesion: 0.06
Nodes (5): LeafReader, TopFieldDocs, TopScoreDocCollector, TopDocs, TopDocsCollector

### Community 23 - "Index Writer"
Cohesion: 0.08
Nodes (4): FieldInfo, Builder, FieldInfos, FieldNumbers

### Community 24 - "Index Writer"
Cohesion: 0.11
Nodes (3): Operations, PointTransitionSet, TransitionList

### Community 25 - "Doc Values"
Cohesion: 0.1
Nodes (2): FieldType, IndexableFieldType

### Community 26 - "Terms Postings"
Cohesion: 0.09
Nodes (5): DocIdSetIterator, IntersectVisitor, PointTree, PointValues, PostingsEnum

### Community 27 - "Segment Metadata"
Cohesion: 0.06
Nodes (1): SegmentCommitInfo

### Community 28 - "Collectors Sorting"
Cohesion: 0.07
Nodes (7): Collector, LeafCollector, SimpleCollector, PagingFieldCollector, SimpleFieldCollector, TopFieldCollector, TopFieldLeafCollector

### Community 29 - "Terms Postings"
Cohesion: 0.07
Nodes (6): MultiTermQuery, RewriteMethod, TopTermsBlendedFreqScoringRewrite, TopTermsBoostOnlyBooleanQueryRewrite, TopTermsScoringBooleanQueryRewrite, TopTermsRewrite

### Community 30 - "Segment Metadata"
Cohesion: 0.08
Nodes (3): CodecReader, getKey(), SegmentReader

### Community 31 - "Similarity Scoring"
Cohesion: 0.09
Nodes (6): Analyzer, getReusableComponents(), ReuseStrategy, setReusableComponents(), StringTokenStream, TokenStreamComponents

### Community 32 - "Store IO"
Cohesion: 0.11
Nodes (2): ByteBufferRecycler, ByteBuffersDataOutput

### Community 33 - "Store IO"
Cohesion: 0.14
Nodes (2): BufferedIndexInput, SlicedIndexInput

### Community 34 - "Similarity Scoring"
Cohesion: 0.08
Nodes (5): Normalization, NormalizationH1, NormalizationH2, NormalizationH3, NormalizationZ

### Community 35 - "Automata Fuzzy"
Cohesion: 0.13
Nodes (1): Automata

### Community 36 - "Similarity Scoring"
Cohesion: 0.09
Nodes (6): CollectionModel, LMSimilarity, IndriCollectionModel, IndriDirichletSimilarity, LMDirichletSimilarity, LMJelinekMercerSimilarity

### Community 37 - "Store IO"
Cohesion: 0.09
Nodes (5): BufferedIndexInput, FSDirectory, MMapDirectory, NIOFSDirectory, NIOFSIndexInput

### Community 38 - "Store IO"
Cohesion: 0.18
Nodes (1): CodecUtil

### Community 39 - "Terms Postings"
Cohesion: 0.1
Nodes (6): getDocCount(), getSumDocFreq(), getSumTotalTermFreq(), iterator(), size(), Terms

### Community 40 - "Terms Postings"
Cohesion: 0.1
Nodes (4): DocValuesIterator, NumericDocValues, SortedDocValues, SortedSetDocValues

### Community 41 - "Store IO"
Cohesion: 0.13
Nodes (4): Cloneable, RandomAccessInput, DataInput, MemorySegmentAccessInput

### Community 42 - "Bulkscorer"
Cohesion: 0.14
Nodes (6): BulkScorer, ScorerSupplier, DefaultBulkScorer, DefaultScorerSupplier, Weight, SegmentCacheable

### Community 43 - "Basicmodel"
Cohesion: 0.09
Nodes (5): BasicModel, BasicModelG, BasicModelIF, BasicModelIn, BasicModelIne

### Community 44 - "Store IO"
Cohesion: 0.24
Nodes (1): ByteBuffersDataInput

### Community 45 - "Abstractdocidsetiterator"
Cohesion: 0.14
Nodes (3): AbstractDocIdSetIterator, DocIdSetIterator, RangeDocIdSetIterator

### Community 46 - "Automata Fuzzy"
Cohesion: 0.12
Nodes (4): AutomatonQuery, PrefixQuery, RegexpQuery, WildcardQuery

### Community 47 - "Doc Values"
Cohesion: 0.11
Nodes (1): IndexableFieldType

### Community 48 - "Store IO"
Cohesion: 0.16
Nodes (1): FileSwitchDirectory

### Community 49 - "Index Writer"
Cohesion: 0.16
Nodes (3): BufferedOutputStream, OutputStreamIndexOutput, XBufferedOutputStream

### Community 50 - "Automata Fuzzy"
Cohesion: 0.15
Nodes (3): FrozenIntSet, StateSet, IntSet

### Community 51 - "Automata Fuzzy"
Cohesion: 0.29
Nodes (12): all(), binary(), build(), end(), FSA, getUTF8Rest(), main(), Adds edge from n1-n2, utf8 byte range v1-v2. (+4 more)

### Community 52 - "Index Writer"
Cohesion: 0.16
Nodes (6): LessThan, PriorityQueue, ScoreLessThan, ShardRef, ShardRefLessThan, TopDocs

### Community 53 - "Automata Fuzzy"
Cohesion: 0.22
Nodes (3): UTF32ToUTF8, UTF8Byte, UTF8Sequence

### Community 54 - "Doc Values"
Cohesion: 0.24
Nodes (1): DocValues

### Community 55 - "Terms Postings"
Cohesion: 0.15
Nodes (2): MultiTermQuery, FuzzyQuery

### Community 56 - "Similarity Scoring"
Cohesion: 0.13
Nodes (4): BulkSimScorer, DefaultBulkSimScorer, Similarity, SimScorer

### Community 57 - "Store IO"
Cohesion: 0.23
Nodes (1): DataOutput

### Community 58 - "Automata Fuzzy"
Cohesion: 0.15
Nodes (5): Lev1ParametricDescription, Lev1TParametricDescription, Lev2ParametricDescription, Lev2TParametricDescription, ParametricDescription

### Community 59 - "Independence"
Cohesion: 0.13
Nodes (4): Independence, IndependenceChiSquared, IndependenceSaturated, IndependenceStandardized

### Community 60 - "Filterdocidsetiterator"
Cohesion: 0.18
Nodes (3): FilterDocIdSetIterator, TwoPhaseIterator, TwoPhaseIteratorAsDocIdSetIterator

### Community 61 - "Document Fields"
Cohesion: 0.14
Nodes (1): BasicStats

### Community 62 - "Search Explanation"
Cohesion: 0.21
Nodes (1): Explanation

### Community 63 - "Aftereffect"
Cohesion: 0.18
Nodes (3): AfterEffect, AfterEffectB, AfterEffectL

### Community 64 - "Lambda"
Cohesion: 0.18
Nodes (3): Lambda, LambdaDF, LambdaTTF

### Community 65 - "Automata Fuzzy"
Cohesion: 0.23
Nodes (2): LevenshteinAutomata, ParametricDescription

### Community 66 - "Nativeaccess"
Cohesion: 0.23
Nodes (2): NativeAccess, PosixNativeAccess

### Community 67 - "Store Ratelimiter"
Cohesion: 0.2
Nodes (2): RateLimiter, SimpleRateLimiter

### Community 68 - "Distribution"
Cohesion: 0.18
Nodes (3): Distribution, DistributionLL, DistributionSPL

### Community 69 - "Similarity Scoring"
Cohesion: 0.22
Nodes (2): NoNormalization, Normalization

### Community 70 - "Index Writer"
Cohesion: 0.2
Nodes (2): FileOpenHint, IOContext

### Community 71 - "Store Randomaccessinput"
Cohesion: 0.22
Nodes (1): RandomAccessInput

### Community 72 - "Similarity Scoring"
Cohesion: 0.25
Nodes (2): ClassicSimilarity, TFIDFSimilarity

### Community 73 - "Automata Fuzzy"
Cohesion: 0.39
Nodes (2): FiniteStringsIterator, PathNode

### Community 74 - "Automata Fuzzy"
Cohesion: 0.48
Nodes (1): IntSet

### Community 75 - "Search Similarities"
Cohesion: 0.33
Nodes (1): AfterEffect

### Community 76 - "Search Similarities"
Cohesion: 0.33
Nodes (1): BasicModel

### Community 77 - "Search Similarities"
Cohesion: 0.4
Nodes (1): Distribution

### Community 79 - "Automata Fuzzy"
Cohesion: 0.47
Nodes (1): ByteRunnable

### Community 80 - "Automata Fuzzy"
Cohesion: 0.33
Nodes (2): LimitedFiniteStringsIterator, FiniteStringsIterator

### Community 81 - "Automata Fuzzy"
Cohesion: 0.33
Nodes (1): StatePair

### Community 82 - "Automata Fuzzy"
Cohesion: 0.33
Nodes (1): TransitionAccessor

### Community 84 - "Search Similarities"
Cohesion: 0.4
Nodes (1): Independence

### Community 85 - "Search Similarities"
Cohesion: 0.4
Nodes (1): Lambda

### Community 86 - "Search Scoredoc"
Cohesion: 0.5
Nodes (1): ScoreDoc

### Community 87 - "Illegalstateexception"
Cohesion: 0.5
Nodes (2): IllegalStateException, AlreadyClosedException

### Community 88 - "Automata Fuzzy"
Cohesion: 0.5
Nodes (1): CaseFolding

## Knowledge Gaps
- **3 isolated node(s):** `FileOpenHint`, `UTF8Byte`, `Adds edge from n1-n2, utf8 byte range v1-v2.`
  These have ≤1 connection - possible missing edges or undocumented components.
- **Thin community `Automata Fuzzy`** (2 nodes): `MakeRegexGroup`, `RegExp`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Doc Values`** (1 nodes): `FieldInfo`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Doc Values`** (2 nodes): `FieldType`, `IndexableFieldType`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Segment Metadata`** (1 nodes): `SegmentCommitInfo`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Store IO`** (2 nodes): `ByteBufferRecycler`, `ByteBuffersDataOutput`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Store IO`** (2 nodes): `BufferedIndexInput`, `SlicedIndexInput`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (1 nodes): `Automata`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Store IO`** (1 nodes): `CodecUtil`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Store IO`** (1 nodes): `ByteBuffersDataInput`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Doc Values`** (1 nodes): `IndexableFieldType`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Store IO`** (1 nodes): `FileSwitchDirectory`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Doc Values`** (1 nodes): `DocValues`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Terms Postings`** (2 nodes): `MultiTermQuery`, `FuzzyQuery`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Store IO`** (1 nodes): `DataOutput`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Document Fields`** (1 nodes): `BasicStats`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Explanation`** (1 nodes): `Explanation`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (2 nodes): `LevenshteinAutomata`, `ParametricDescription`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Nativeaccess`** (2 nodes): `NativeAccess`, `PosixNativeAccess`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Store Ratelimiter`** (2 nodes): `RateLimiter`, `SimpleRateLimiter`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Similarity Scoring`** (2 nodes): `NoNormalization`, `Normalization`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Index Writer`** (2 nodes): `FileOpenHint`, `IOContext`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Store Randomaccessinput`** (1 nodes): `RandomAccessInput`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Similarity Scoring`** (2 nodes): `ClassicSimilarity`, `TFIDFSimilarity`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (2 nodes): `FiniteStringsIterator`, `PathNode`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (1 nodes): `IntSet`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Similarities`** (1 nodes): `AfterEffect`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Similarities`** (1 nodes): `BasicModel`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Similarities`** (1 nodes): `Distribution`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (1 nodes): `ByteRunnable`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (2 nodes): `LimitedFiniteStringsIterator`, `FiniteStringsIterator`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (1 nodes): `StatePair`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (1 nodes): `TransitionAccessor`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Similarities`** (1 nodes): `Independence`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Similarities`** (1 nodes): `Lambda`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Search Scoredoc`** (1 nodes): `ScoreDoc`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Illegalstateexception`** (2 nodes): `IllegalStateException`, `AlreadyClosedException`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.
- **Thin community `Automata Fuzzy`** (1 nodes): `CaseFolding`
  Too small to be a meaningful cluster - may be noise or needs more connections extracted.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `IndexWriter` connect `Index Writer` to `Store IO`, `Store IO`?**
  _High betweenness centrality (0.074) - this node is a cross-community bridge._
- **Why does `SegmentInfos` connect `Index Writer` to `Index Writer`, `Store IO`, `Field Metadata`?**
  _High betweenness centrality (0.037) - this node is a cross-community bridge._
- **What connects `FileOpenHint`, `UTF8Byte`, `Adds edge from n1-n2, utf8 byte range v1-v2.` to the rest of the system?**
  _3 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Index Writer` be split into smaller, more focused modules?**
  _Cohesion score 0.03 - nodes in this community are weakly interconnected._
- **Should `Store IO` be split into smaller, more focused modules?**
  _Cohesion score 0.02 - nodes in this community are weakly interconnected._
- **Should `Index Writer` be split into smaller, more focused modules?**
  _Cohesion score 0.02 - nodes in this community are weakly interconnected._
- **Should `Terms Postings` be split into smaller, more focused modules?**
  _Cohesion score 0.02 - nodes in this community are weakly interconnected._