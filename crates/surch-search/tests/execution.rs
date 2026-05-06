use std::collections::BTreeMap;

use surch_index::postings::{PostingsBuilder, TermDictionary};
use surch_search::collector::ScoreDoc;
use surch_search::execution::{TermQueryExecutionError, TermQueryExecutor, TermQueryStats};
use surch_search::query::TermQuery;

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
