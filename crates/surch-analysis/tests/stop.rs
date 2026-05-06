use surch_analysis::{stop_filter, Analyzer, StopAnalyzer, Token, ENGLISH_STOP_WORDS};

const STOP_FILTER_FIXTURE: &str =
    include_str!("../../../tests/lucene_parity/analysis/stop_filter.tsv");
const STOP_ANALYZER_FIXTURE: &str =
    include_str!("../../../tests/lucene_parity/analysis/stop_analyzer.tsv");

#[test]
fn stop_filter_removes_terms_and_accumulates_position_increment() {
    let tokens = vec![
        token("the", 0, 3, 1),
        token("quick", 4, 9, 1),
        token("and", 10, 13, 1),
        token("brown", 14, 19, 1),
        token("fox", 20, 23, 1),
    ];

    assert_eq!(
        stop_filter(tokens, ["the", "and"]),
        vec![
            token("quick", 4, 9, 2),
            token("brown", 14, 19, 2),
            token("fox", 20, 23, 1)
        ]
    );
}

#[test]
fn stop_filter_preserves_offsets_and_handles_empty_results() {
    assert_eq!(
        stop_filter(vec![token("keep", 7, 11, 3)], ["stop"]),
        vec![token("keep", 7, 11, 3)]
    );

    assert!(stop_filter(
        vec![token("a", 0, 1, 1), token("the", 2, 5, 1)],
        ["a", "the"]
    )
    .is_empty());
    assert!(stop_filter(Vec::new(), ["a"]).is_empty());
}

#[test]
fn stop_filter_matches_lucene_parity_fixture() {
    for line in STOP_FILTER_FIXTURE.lines().skip(1) {
        let columns: Vec<&str> = line.split('\t').collect();
        assert_eq!(columns.len(), 3, "fixture row must have 3 columns: {line}");

        assert_eq!(
            stop_filter(
                parse_expected_tokens(columns[0]),
                parse_stop_words(columns[1])
            ),
            parse_expected_tokens(columns[2]),
            "fixture row failed: {line}"
        );
    }
}

#[test]
fn stop_analyzer_applies_simple_analyzer_then_english_stop_filter() {
    let analyzer = StopAnalyzer;

    assert_eq!(
        analyzer.token_stream("The quick brown fox is in the yard."),
        vec![
            token("quick", 4, 9, 2),
            token("brown", 10, 15, 1),
            token("fox", 16, 19, 1),
            token("yard", 30, 34, 4),
        ]
    );
}

#[test]
fn stop_analyzer_emits_no_tokens_for_empty_or_stop_only_text() {
    let analyzer = StopAnalyzer;

    assert!(analyzer.token_stream("").is_empty());
    assert!(analyzer.token_stream("the and in").is_empty());
}

#[test]
fn stop_analyzer_matches_lucene_parity_fixture() {
    let analyzer = StopAnalyzer;

    for line in STOP_ANALYZER_FIXTURE.lines().skip(1) {
        let columns: Vec<&str> = line.split('\t').collect();
        assert_eq!(columns.len(), 2, "fixture row must have 2 columns: {line}");

        assert_eq!(
            analyzer.token_stream(columns[0]),
            parse_expected_tokens(columns[1]),
            "fixture row failed: {line}"
        );
    }
}

#[test]
fn stop_analyzer_exposes_minimal_english_stop_words() {
    assert!(ENGLISH_STOP_WORDS.contains(&"the"));
    assert!(ENGLISH_STOP_WORDS.contains(&"with"));
    assert!(!ENGLISH_STOP_WORDS.contains(&"quick"));
}

fn token(term: &str, start_offset: usize, end_offset: usize, position_increment: u32) -> Token {
    Token {
        term: term.to_owned(),
        start_offset,
        end_offset,
        position_increment,
    }
}

fn parse_stop_words(raw: &str) -> Vec<&str> {
    if raw.is_empty() {
        return Vec::new();
    }

    raw.split(',').collect()
}

fn parse_expected_tokens(raw: &str) -> Vec<Token> {
    if raw.is_empty() {
        return Vec::new();
    }

    raw.split('|')
        .map(|encoded| {
            let parts: Vec<&str> = encoded.split(':').collect();
            assert_eq!(parts.len(), 4, "token must have 4 fields: {encoded}");

            token(
                parts[0],
                parts[1].parse().expect("start offset must be usize"),
                parts[2].parse().expect("end offset must be usize"),
                parts[3].parse().expect("position increment must be u32"),
            )
        })
        .collect()
}
