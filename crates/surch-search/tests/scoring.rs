use surch_search::scoring::{bm25_idf, bm25_score, Bm25Config, Bm25Error};

const EPSILON: f64 = 0.000_001;

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= EPSILON,
        "actual {actual} did not match expected {expected}"
    );
}

#[test]
fn scoring_bm25_config_defaults_match_lucene_similarity() {
    let config = Bm25Config::default();

    assert_eq!(config.k1, 1.2);
    assert_eq!(config.b, 0.75);
}

#[test]
fn scoring_bm25_idf_uses_lucene_similarity_formula() {
    let idf = bm25_idf(10, 2).expect("valid corpus stats");

    assert_close(idf, 1.481_604_540_924_215_6);
}

#[test]
fn scoring_bm25_score_uses_lucene_similarity_formula() {
    let score = bm25_score(Bm25Config::default(), 10, 2, 3, 120, 100.0).expect("valid BM25 inputs");

    assert_close(score, 2.232_554_787_694_023_7);
}

#[test]
fn scoring_bm25_rejects_invalid_corpus_statistics() {
    assert_eq!(bm25_idf(0, 1), Err(Bm25Error::DocCountZero));
    assert_eq!(bm25_idf(10, 0), Err(Bm25Error::DocFreqZero));
    assert_eq!(
        bm25_idf(10, 11),
        Err(Bm25Error::DocFreqExceedsDocCount {
            doc_freq: 11,
            doc_count: 10,
        })
    );
}

#[test]
fn scoring_bm25_rejects_invalid_score_inputs() {
    assert_eq!(
        bm25_score(Bm25Config::default(), 10, 2, 0, 120, 100.0),
        Err(Bm25Error::TermFreqZero)
    );
    assert_eq!(
        bm25_score(Bm25Config::default(), 10, 2, 1, 0, 100.0),
        Err(Bm25Error::DocLenZero)
    );
    assert_eq!(
        bm25_score(Bm25Config::default(), 10, 2, 1, 120, 0.0),
        Err(Bm25Error::AvgDocLenNotPositive)
    );
}

#[test]
fn scoring_bm25_config_validation_is_typed() {
    assert_eq!(
        Bm25Config::new(-0.1, 0.75),
        Err(Bm25Error::NegativeK1 { k1: -0.1 })
    );
    assert_eq!(
        Bm25Config::new(1.2, -0.1),
        Err(Bm25Error::BOutOfRange { b: -0.1 })
    );
    assert_eq!(
        Bm25Config::new(1.2, 1.1),
        Err(Bm25Error::BOutOfRange { b: 1.1 })
    );
}

#[test]
fn scoring_bm25_classic_fixture_matches_expected_scores() {
    let fixture = include_str!("../../../tests/lucene_parity/search/bm25_classic.tsv");

    for (line_number, line) in fixture.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut columns = line.split('\t');
        let doc_count = columns
            .next()
            .expect("fixture doc_count")
            .parse::<u64>()
            .expect("fixture doc_count is u64");
        let doc_freq = columns
            .next()
            .expect("fixture doc_freq")
            .parse::<u64>()
            .expect("fixture doc_freq is u64");
        let term_freq = columns
            .next()
            .expect("fixture term_freq")
            .parse::<u64>()
            .expect("fixture term_freq is u64");
        let doc_len = columns
            .next()
            .expect("fixture doc_len")
            .parse::<u64>()
            .expect("fixture doc_len is u64");
        let avg_doc_len = columns
            .next()
            .expect("fixture avg_doc_len")
            .parse::<f64>()
            .expect("fixture avg_doc_len is f64");
        let expected_idf = columns
            .next()
            .expect("fixture expected_idf")
            .parse::<f64>()
            .expect("fixture expected_idf is f64");
        let expected_score = columns
            .next()
            .expect("fixture expected_score")
            .parse::<f64>()
            .expect("fixture expected_score is f64");
        assert_eq!(
            columns.next(),
            None,
            "unexpected extra fixture column on line {}",
            line_number + 1
        );

        assert_close(
            bm25_idf(doc_count, doc_freq).expect("fixture idf inputs"),
            expected_idf,
        );
        assert_close(
            bm25_score(
                Bm25Config::default(),
                doc_count,
                doc_freq,
                term_freq,
                doc_len,
                avg_doc_len,
            )
            .expect("fixture score inputs"),
            expected_score,
        );
    }
}
