use surch_search::collector::{CollectorError, ScoreDoc, TopDocs, TopDocsCollector};

#[test]
fn top_docs_collector_rejects_zero_size() {
    assert!(matches!(
        TopDocsCollector::new(0),
        Err(CollectorError::SizeMustBePositive)
    ));
}

#[test]
fn top_docs_collector_sorts_by_score_desc_then_doc_id_asc_and_truncates() {
    let mut collector = TopDocsCollector::new(3).expect("valid collector size");

    collector.collect(9, 0.25).expect("finite score");
    collector.collect(3, 1.5).expect("finite score");
    collector.collect(7, 1.5).expect("finite score");
    collector.collect(1, 2.0).expect("finite score");
    collector.collect(5, 1.5).expect("finite score");

    assert_eq!(
        collector.finish(),
        TopDocs {
            total_hits: 5,
            score_docs: vec![
                ScoreDoc {
                    doc_id: 1,
                    score: 2.0
                },
                ScoreDoc {
                    doc_id: 3,
                    score: 1.5
                },
                ScoreDoc {
                    doc_id: 5,
                    score: 1.5
                },
            ],
        }
    );
}

#[test]
fn top_docs_collector_rejects_non_finite_scores_without_counting_them() {
    let mut collector = TopDocsCollector::new(2).expect("valid collector size");

    collector.collect(1, 1.0).expect("finite score");
    assert_eq!(
        collector.collect(2, f64::NAN),
        Err(CollectorError::NonFiniteScore { doc_id: 2 })
    );
    assert_eq!(
        collector.collect(3, f64::INFINITY),
        Err(CollectorError::NonFiniteScore { doc_id: 3 })
    );

    assert_eq!(
        collector.finish(),
        TopDocs {
            total_hits: 1,
            score_docs: vec![ScoreDoc {
                doc_id: 1,
                score: 1.0
            }],
        }
    );
}

#[test]
fn top_docs_collector_classic_fixture_matches_expected_order() {
    let fixture = include_str!("../../../tests/lucene_parity/search/top_docs_classic.tsv");
    let mut collector = TopDocsCollector::new(3).expect("valid collector size");
    let mut expected = Vec::new();
    let mut valid_hits = 0;

    for (line_number, line) in fixture.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut columns = line.split('\t');
        let doc_id = columns
            .next()
            .expect("fixture doc_id")
            .parse::<u32>()
            .expect("fixture doc_id is u32");
        let score = columns
            .next()
            .expect("fixture score")
            .parse::<f64>()
            .expect("fixture score is f64");
        let expected_rank = columns
            .next()
            .expect("fixture expected rank")
            .parse::<usize>()
            .expect("fixture expected rank is usize");
        assert_eq!(
            columns.next(),
            None,
            "unexpected extra fixture column on line {}",
            line_number + 1
        );

        collector
            .collect(doc_id, score)
            .expect("fixture score is finite");
        valid_hits += 1;

        if expected_rank > 0 {
            expected.push((expected_rank, ScoreDoc { doc_id, score }));
        }
    }

    expected.sort_by_key(|(rank, _score_doc)| *rank);
    let expected = expected
        .into_iter()
        .map(|(_rank, score_doc)| score_doc)
        .collect();

    assert_eq!(
        collector.finish(),
        TopDocs {
            total_hits: valid_hits,
            score_docs: expected,
        }
    );
}
