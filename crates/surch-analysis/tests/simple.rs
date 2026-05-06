use surch_analysis::{Analyzer, SimpleAnalyzer, Token};

const SIMPLE_FIXTURE: &str =
    include_str!("../../../tests/lucene_parity/analysis/simple_analyzer.tsv");

#[test]
fn simple_analyzer_tokenizes_alphabetic_sequences_and_lowercases_terms() {
    let analyzer = SimpleAnalyzer;

    assert_eq!(
        analyzer.token_stream("Surch 2.0: CAFÉ déjà-vu! β42"),
        vec![
            Token {
                term: "surch".to_owned(),
                start_offset: 0,
                end_offset: 5,
                position_increment: 1,
            },
            Token {
                term: "café".to_owned(),
                start_offset: 11,
                end_offset: 16,
                position_increment: 1,
            },
            Token {
                term: "déjà".to_owned(),
                start_offset: 17,
                end_offset: 23,
                position_increment: 1,
            },
            Token {
                term: "vu".to_owned(),
                start_offset: 24,
                end_offset: 26,
                position_increment: 1,
            },
            Token {
                term: "β".to_owned(),
                start_offset: 28,
                end_offset: 30,
                position_increment: 1,
            },
        ]
    );
}

#[test]
fn simple_analyzer_emits_no_tokens_for_empty_or_letterless_text() {
    let analyzer = SimpleAnalyzer;

    assert!(analyzer.token_stream("").is_empty());
    assert!(analyzer.token_stream("123 -- !!!").is_empty());
}

#[test]
fn simple_analyzer_matches_lucene_parity_fixture() {
    let analyzer = SimpleAnalyzer;

    for line in SIMPLE_FIXTURE.lines().skip(1) {
        let columns: Vec<&str> = line.split('\t').collect();
        assert_eq!(columns.len(), 2, "fixture row must have 2 columns: {line}");

        assert_eq!(
            analyzer.token_stream(columns[0]),
            parse_expected_tokens(columns[1]),
            "fixture row failed: {line}"
        );
    }
}

fn parse_expected_tokens(raw: &str) -> Vec<Token> {
    if raw.is_empty() {
        return Vec::new();
    }

    raw.split('|')
        .map(|encoded| {
            let parts: Vec<&str> = encoded.split(':').collect();
            assert_eq!(parts.len(), 4, "token must have 4 fields: {encoded}");

            Token {
                term: parts[0].to_owned(),
                start_offset: parts[1].parse().expect("start offset must be usize"),
                end_offset: parts[2].parse().expect("end offset must be usize"),
                position_increment: parts[3].parse().expect("position increment must be u32"),
            }
        })
        .collect()
}
