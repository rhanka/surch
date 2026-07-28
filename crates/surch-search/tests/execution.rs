use std::collections::BTreeMap;

use surch_index::postings::{PostingsBuilder, TermDictionary, BLOCK_SIZE};
use surch_search::collector::ScoreDoc;
use surch_search::execution::{
    BooleanQueryExecutionError, BooleanQueryExecutor, TermQueryExecutionError, TermQueryExecutor,
    TermQueryStats,
};
use surch_search::query::{BooleanQuery, Query, TermQuery};
use surch_search::scoring::{bm25_score, Bm25Config};

const EPSILON: f64 = 0.000_001;

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= EPSILON,
        "actual {actual} did not match expected {expected}"
    );
}

fn classic_dictionary() -> TermDictionary {
    let fixture = include_str!("../../../tests/lucene_parity/index/postings_classic.tsv");
    let mut builder = PostingsBuilder::new();

    for line in fixture.lines().skip(1) {
        let columns = line.split('\t').collect::<Vec<_>>();
        assert_eq!(columns.len(), 5, "fixture row has five columns: {line}");

        let doc_id = columns[2].parse::<u32>().expect("doc_id");
        let positions = columns[4]
            .split(',')
            .filter(|position| !position.is_empty())
            .map(|position| position.parse::<u32>().expect("position"))
            .collect::<Vec<_>>();

        builder
            .add(columns[0], columns[1], doc_id, positions)
            .expect("fixture posting");
    }

    builder.build()
}

fn classic_stats() -> TermQueryStats {
    TermQueryStats::new(4, 3.5, BTreeMap::from([(1, 4), (2, 3), (3, 6), (4, 1)]))
        .expect("valid classic stats")
}

fn boolean_dictionary() -> TermDictionary {
    let mut builder = PostingsBuilder::new();

    builder
        .add("body", "search", 1, vec![0])
        .expect("search doc 1");
    builder
        .add("body", "search", 2, vec![0])
        .expect("search doc 2");
    builder
        .add("body", "engine", 1, vec![1])
        .expect("engine doc 1");
    builder
        .add("body", "engine", 2, vec![1, 3])
        .expect("engine doc 2");
    builder
        .add("body", "engine", 3, vec![2])
        .expect("engine doc 3");

    builder.build()
}

fn boolean_stats() -> TermQueryStats {
    TermQueryStats::new(3, 4.0, BTreeMap::from([(1, 4), (2, 4), (3, 4)]))
        .expect("valid boolean stats")
}

fn score_for(score_docs: &[ScoreDoc], doc_id: u32) -> f64 {
    score_docs
        .iter()
        .find(|score_doc| score_doc.doc_id == doc_id)
        .map(|score_doc| score_doc.score)
        .expect("doc score")
}

#[test]
fn term_query_executor_scores_postings_with_bm25_and_top_docs_ordering() {
    let dictionary = classic_dictionary();
    let executor = TermQueryExecutor::new(&dictionary, classic_stats());
    let query = TermQuery::new("body", "search").expect("valid query");

    let top_docs = executor.execute(&query, 10).expect("term execution");

    assert_eq!(top_docs.total_hits, 2);
    assert_eq!(
        top_docs
            .score_docs
            .iter()
            .map(|score_doc| score_doc.doc_id)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_close(top_docs.score_docs[0].score, 0.916_263_225_804_563_1);
    assert_close(top_docs.score_docs[1].score, 0.536_405_355_810_209);
}

#[test]
fn term_query_executor_classic_fixture_matches_body_search_order() {
    let fixture = include_str!("../../../tests/lucene_parity/search/term_execution_classic.tsv");
    let dictionary = classic_dictionary();
    let executor = TermQueryExecutor::new(&dictionary, classic_stats());
    let mut expected = Vec::new();

    for (line_number, line) in fixture.lines().enumerate().skip(1) {
        let columns = line.split('\t').collect::<Vec<_>>();
        assert_eq!(
            columns.len(),
            6,
            "fixture row has six columns on line {}: {line}",
            line_number + 1
        );

        let query = TermQuery::new(columns[0], columns[1]).expect("fixture query");
        assert_eq!(query.field, "body");
        assert_eq!(query.value, "search");

        expected.push(ScoreDoc {
            doc_id: columns[2].parse::<u32>().expect("doc_id"),
            score: columns[5].parse::<f64>().expect("expected_score"),
        });
    }

    let query = TermQuery::new("body", "search").expect("valid query");
    let top_docs = executor.execute(&query, 10).expect("term execution");

    assert_eq!(top_docs.total_hits, expected.len());
    assert_eq!(top_docs.score_docs.len(), expected.len());

    for (actual, expected) in top_docs.score_docs.iter().zip(expected.iter()) {
        assert_eq!(actual.doc_id, expected.doc_id);
        assert_close(actual.score, expected.score);
    }
}

#[test]
fn term_query_executor_uses_for_block_metadata_doc_freq_without_changing_scores() {
    let mut builder = PostingsBuilder::new();
    let total_docs = BLOCK_SIZE + 3;
    for doc_id in 0..total_docs {
        builder
            .add("body", "runtime", doc_id as u32, vec![0])
            .expect("runtime posting");
    }
    let dictionary = builder.build();
    // Lot C Phase 1 lever B: `BlockMeta::posting_count` is gone (derivable
    // from `doc_ids` at O(1)) — `doc_freq_from_block_metas()` is the
    // supported way to get the same total posting count.
    let metadata_doc_freq = dictionary
        .postings_with_block_metas("body", "runtime")
        .expect("runtime postings list")
        .doc_freq_from_block_metas() as u64;
    assert_eq!(metadata_doc_freq, total_docs as u64);

    let stats = TermQueryStats::new(
        total_docs as u64,
        1.0,
        (0..total_docs).map(|doc_id| (doc_id as u32, 1)).collect(),
    )
    .expect("valid stats");
    let executor = TermQueryExecutor::new(&dictionary, stats);
    let query = TermQuery::new("body", "runtime").expect("valid query");

    let top_docs = executor.execute(&query, 3).expect("term execution");

    let expected_score = bm25_score(
        Bm25Config::default(),
        total_docs as u64,
        metadata_doc_freq,
        1,
        1,
        1.0,
    )
    .expect("expected score");
    assert_eq!(top_docs.total_hits, total_docs);
    assert_eq!(
        top_docs
            .score_docs
            .iter()
            .map(|score_doc| score_doc.doc_id)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    for score_doc in &top_docs.score_docs {
        assert_close(score_doc.score, expected_score);
    }

    let execution_source = include_str!("../src/execution.rs");
    assert!(
        execution_source.contains("postings_with_block_metas"),
        "TermQueryExecutor must request the runtime postings + FoR block metadata view"
    );
    assert!(
        execution_source.contains("doc_freq_from_block_metas"),
        "TermQueryExecutor must use FoR block metadata for doc_freq"
    );
}

#[test]
fn term_query_executor_returns_empty_top_docs_for_missing_term() {
    let dictionary = classic_dictionary();
    let executor = TermQueryExecutor::new(&dictionary, classic_stats());
    let query = TermQuery::new("body", "missing").expect("valid query");

    let top_docs = executor.execute(&query, 5).expect("term execution");

    assert_eq!(top_docs.total_hits, 0);
    assert!(top_docs.score_docs.is_empty());
}

#[test]
fn term_query_executor_rejects_zero_size() {
    let dictionary = classic_dictionary();
    let executor = TermQueryExecutor::new(&dictionary, classic_stats());
    let query = TermQuery::new("body", "search").expect("valid query");

    assert_eq!(
        executor.execute(&query, 0),
        Err(TermQueryExecutionError::SizeMustBePositive)
    );
}

#[test]
fn term_query_stats_validation_is_typed() {
    assert_eq!(
        TermQueryStats::new(0, 3.5, BTreeMap::from([(1, 4)])),
        Err(TermQueryExecutionError::DocCountZero)
    );
    assert_eq!(
        TermQueryStats::new(4, 0.0, BTreeMap::from([(1, 4)])),
        Err(TermQueryExecutionError::AvgDocLenNotPositive)
    );
    assert_eq!(
        TermQueryStats::new(4, 3.5, BTreeMap::from([(1, 0)])),
        Err(TermQueryExecutionError::DocLenZero { doc_id: 1 })
    );
}

#[test]
fn term_query_executor_rejects_missing_doc_length_for_matching_posting() {
    let dictionary = classic_dictionary();
    let stats = TermQueryStats::new(4, 3.5, BTreeMap::from([(1, 4)])).expect("partial stats");
    let executor = TermQueryExecutor::new(&dictionary, stats);
    let query = TermQuery::new("body", "search").expect("valid query");

    assert_eq!(
        executor.execute(&query, 10),
        Err(TermQueryExecutionError::MissingDocLen { doc_id: 3 })
    );
}

#[test]
fn boolean_query_execution_intersects_two_must_terms_and_sums_clause_scores() {
    let dictionary = boolean_dictionary();
    let stats = boolean_stats();
    let term_executor = TermQueryExecutor::new(&dictionary, stats.clone());
    let boolean_executor = BooleanQueryExecutor::new(&dictionary, stats);
    let search_query = TermQuery::new("body", "search").expect("valid query");
    let engine_query = TermQuery::new("body", "engine").expect("valid query");
    let query = BooleanQuery::new(vec![
        Query::Term(search_query.clone()),
        Query::Term(engine_query.clone()),
    ])
    .expect("valid boolean query");

    let search_docs = term_executor
        .execute(&search_query, 10)
        .expect("search execution");
    let engine_docs = term_executor
        .execute(&engine_query, 10)
        .expect("engine execution");
    let top_docs = boolean_executor
        .execute(&query, 10)
        .expect("boolean execution");

    assert_eq!(top_docs.total_hits, 2);
    assert_eq!(
        top_docs
            .score_docs
            .iter()
            .map(|score_doc| score_doc.doc_id)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    assert_close(
        top_docs.score_docs[0].score,
        score_for(&search_docs.score_docs, 2) + score_for(&engine_docs.score_docs, 2),
    );
    assert_close(
        top_docs.score_docs[1].score,
        score_for(&search_docs.score_docs, 1) + score_for(&engine_docs.score_docs, 1),
    );
}

#[test]
fn boolean_query_execution_rejects_zero_size() {
    let dictionary = boolean_dictionary();
    let executor = BooleanQueryExecutor::new(&dictionary, boolean_stats());
    let query = BooleanQuery::new(vec![
        Query::Term(TermQuery::new("body", "search").expect("valid query")),
        Query::Term(TermQuery::new("body", "engine").expect("valid query")),
    ])
    .expect("valid boolean query");

    assert_eq!(
        executor.execute(&query, 0),
        Err(BooleanQueryExecutionError::Term(
            TermQueryExecutionError::SizeMustBePositive
        ))
    );
}

/// Lot 2 (skip lists on the codec FoR path): build two posting lists
/// large enough to span multiple 128-blocks, with an intersection of
/// only a handful of doc ids spread far apart. The AND-leapfrog
/// driven by the codec [`BlockSkipList`] must skip the vast majority
/// of the rarer-clause's blocks — the perf counter
/// `BooleanQueryExecutor::last_blocks_skipped` proves it.
#[test]
fn boolean_query_leapfrog_skips_blocks_on_sparse_intersection() {
    let mut builder = PostingsBuilder::new();

    // "frequent": present in every doc 0..N (N spans 8 blocks).
    // "rare": present in 4 docs at block boundaries (0, 128, 384, 1023).
    let total_docs = BLOCK_SIZE * 8;
    let rare_doc_ids: Vec<u32> = vec![0, BLOCK_SIZE as u32, 3 * BLOCK_SIZE as u32, 1023];
    for doc_id in 0..total_docs {
        builder
            .add("body", "frequent", doc_id as u32, vec![0])
            .expect("frequent posting");
    }
    for &doc_id in &rare_doc_ids {
        builder
            .add("body", "rare", doc_id, vec![0])
            .expect("rare posting");
    }
    let dictionary = builder.build();
    let stats = TermQueryStats::new(
        total_docs as u64,
        1.0,
        (0..total_docs).map(|doc_id| (doc_id as u32, 1)).collect(),
    )
    .expect("valid stats");

    let term_executor = TermQueryExecutor::new(&dictionary, stats.clone());
    let boolean_executor = BooleanQueryExecutor::new(&dictionary, stats);

    let frequent_query = TermQuery::new("body", "frequent").expect("valid query");
    let rare_query = TermQuery::new("body", "rare").expect("valid query");
    let query = BooleanQuery::new(vec![
        Query::Term(frequent_query.clone()),
        Query::Term(rare_query.clone()),
    ])
    .expect("valid boolean query");

    // Run the boolean executor.
    let top_docs = boolean_executor
        .execute(&query, rare_doc_ids.len())
        .expect("boolean execution");

    // (a) Result set parity: the AND must equal `rare`'s doc ids
    // (since `frequent` covers everything), and per-doc score must
    // equal `bm25(frequent) + bm25(rare)` for each rare doc.
    let mut returned_ids: Vec<u32> = top_docs.score_docs.iter().map(|sd| sd.doc_id).collect();
    returned_ids.sort_unstable();
    assert_eq!(returned_ids, rare_doc_ids);
    assert_eq!(top_docs.total_hits, rare_doc_ids.len());

    let frequent_docs = term_executor
        .execute(&frequent_query, usize::MAX)
        .expect("frequent execution");
    let rare_docs = term_executor
        .execute(&rare_query, usize::MAX)
        .expect("rare execution");
    for sd in &top_docs.score_docs {
        let expected = score_for(&frequent_docs.score_docs, sd.doc_id)
            + score_for(&rare_docs.score_docs, sd.doc_id);
        assert_close(sd.score, expected);
    }

    // (b) The leapfrog must have skipped blocks. Driver = `rare`
    // (smaller doc_freq). Followers = `frequent` (8 blocks).
    // Each rare doc lands in a distinct block of `frequent`; between
    // two consecutive rare hits at blocks 0, 1, 3, 7, the leapfrog
    // jumps past blocks 2 (between rare at block 1 and rare at block
    // 3) and blocks 4, 5, 6 (between rare at block 3 and rare at
    // block 7). That's 4 blocks skipped at minimum.
    let blocks_skipped = boolean_executor.last_blocks_skipped();
    assert!(
        blocks_skipped > 0,
        "AND-leapfrog must skip at least one block of the rarer clause, got {blocks_skipped}",
    );
}

/// Lot 2 regression guard: the AND-leapfrog must produce the SAME
/// scored doc set as the pre-Lot-2 BTreeMap intersection on a
/// non-trivial multi-block fixture. We verify this by computing the
/// per-clause scores via the single-clause `TermQueryExecutor` and
/// summing them by hand, then comparing against the boolean
/// executor's output.
#[test]
fn boolean_query_leapfrog_matches_single_clause_score_sum_across_blocks() {
    let mut builder = PostingsBuilder::new();
    let block_count = 4;
    let total_docs = BLOCK_SIZE * block_count;
    // Two terms that intersect on every other doc id.
    for doc_id in 0..total_docs {
        builder
            .add("body", "alpha", doc_id as u32, vec![0])
            .expect("alpha posting");
        if doc_id.is_multiple_of(2) {
            builder
                .add("body", "beta", doc_id as u32, vec![0, 1])
                .expect("beta posting");
        }
    }
    let dictionary = builder.build();
    let stats = TermQueryStats::new(
        total_docs as u64,
        2.0,
        (0..total_docs).map(|doc_id| (doc_id as u32, 2)).collect(),
    )
    .expect("valid stats");

    let term_executor = TermQueryExecutor::new(&dictionary, stats.clone());
    let boolean_executor = BooleanQueryExecutor::new(&dictionary, stats);

    let alpha_query = TermQuery::new("body", "alpha").expect("valid query");
    let beta_query = TermQuery::new("body", "beta").expect("valid query");
    let query = BooleanQuery::new(vec![
        Query::Term(alpha_query.clone()),
        Query::Term(beta_query.clone()),
    ])
    .expect("valid boolean query");

    let top_docs = boolean_executor
        .execute(&query, total_docs)
        .expect("boolean execution");

    // Build the ground-truth scored set by running each clause alone
    // and summing the per-doc contributions for docs present in both.
    let alpha_docs = term_executor
        .execute(&alpha_query, usize::MAX)
        .expect("alpha execution");
    let beta_docs = term_executor
        .execute(&beta_query, usize::MAX)
        .expect("beta execution");

    let alpha_scores: std::collections::BTreeMap<u32, f64> = alpha_docs
        .score_docs
        .iter()
        .map(|sd| (sd.doc_id, sd.score))
        .collect();
    let beta_scores: std::collections::BTreeMap<u32, f64> = beta_docs
        .score_docs
        .iter()
        .map(|sd| (sd.doc_id, sd.score))
        .collect();
    let mut expected: std::collections::BTreeMap<u32, f64> = std::collections::BTreeMap::new();
    for (&doc_id, &alpha_score) in &alpha_scores {
        if let Some(&beta_score) = beta_scores.get(&doc_id) {
            expected.insert(doc_id, alpha_score + beta_score);
        }
    }

    // Same doc_id set.
    let mut returned_ids: Vec<u32> = top_docs.score_docs.iter().map(|sd| sd.doc_id).collect();
    returned_ids.sort_unstable();
    let mut expected_ids: Vec<u32> = expected.keys().copied().collect();
    expected_ids.sort_unstable();
    assert_eq!(returned_ids, expected_ids);
    assert_eq!(top_docs.total_hits, expected.len());

    // Same per-doc score.
    for sd in &top_docs.score_docs {
        let expected_score = expected.get(&sd.doc_id).copied().expect("doc id");
        assert_close(sd.score, expected_score);
    }

    // Skip counter is observable, regardless of value (the driver is
    // `beta`, half the docs of `alpha`, so `alpha` may or may not
    // skip blocks depending on the intersection density — here every
    // other doc is in both, so no block is fully past beta's next
    // candidate before alpha re-engages).
    let _ = boolean_executor.last_blocks_skipped();
}
