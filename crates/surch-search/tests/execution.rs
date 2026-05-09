use std::collections::BTreeMap;

use surch_index::postings::{PostingsBuilder, TermDictionary};
use surch_search::collector::ScoreDoc;
use surch_search::execution::{
    BooleanQueryExecutionError, BooleanQueryExecutor, TermQueryExecutionError, TermQueryExecutor,
    TermQueryStats,
};
use surch_search::query::{BooleanQuery, Query, TermQuery};

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
