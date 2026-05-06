use surch_analysis::{Analyzer, KeywordAnalyzer, Token, WhitespaceAnalyzer};

const ANALYSIS_FIXTURE: &str =
    include_str!("../../../tests/lucene_parity/analysis/token_stream_classic.tsv");

#[test]
fn keyword_analyzer_emits_entire_input_as_one_token() {
    let analyzer = KeywordAnalyzer;

    assert_eq!(
        analyzer.token_stream("Hello, Surch"),
        vec![Token {
            term: "Hello, Surch".to_owned(),
            start_offset: 0,
            end_offset: 12,
            position_increment: 1,
        }]
    );
}

#[test]
fn keyword_analyzer_emits_no_tokens_for_empty_input() {
    let analyzer = KeywordAnalyzer;

    assert!(analyzer.token_stream("").is_empty());
}

#[test]
fn whitespace_analyzer_splits_on_unicode_whitespace_with_byte_offsets() {
    let analyzer = WhitespaceAnalyzer;

    assert_eq!(
        analyzer.token_stream("a café  β"),
        vec![
            Token {
                term: "a".to_owned(),
                start_offset: 0,
                end_offset: 1,
                position_increment: 1,
            },
            Token {
                term: "café".to_owned(),
                start_offset: 2,
                end_offset: 7,
                position_increment: 1,
            },
            Token {
                term: "β".to_owned(),
                start_offset: 9,
                end_offset: 11,
                position_increment: 1,
            },
        ]
    );
}

#[test]
fn analyzers_match_lucene_parity_fixture() {
    for line in ANALYSIS_FIXTURE.lines().skip(1) {
        let columns: Vec<&str> = line.split('\t').collect();
        assert_eq!(columns.len(), 3, "fixture row must have 3 columns: {line}");

        let analyzer_name = columns[0];
        let input = columns[1];
        let expected = parse_expected_tokens(columns[2]);
        let actual = match analyzer_name {
            "keyword" => KeywordAnalyzer.token_stream(input),
            "whitespace" => WhitespaceAnalyzer.token_stream(input),
            other => panic!("unknown analyzer in fixture: {other}"),
        };

        assert_eq!(actual, expected, "fixture row failed: {line}");
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
