use surch_analysis::{lowercase_tokens, Token};

const LOWERCASE_FIXTURE: &str =
    include_str!("../../../tests/lucene_parity/analysis/lowercase_filter.tsv");

#[test]
fn lowercase_filter_preserves_offsets_and_position_increment() {
    let tokens = vec![
        Token {
            term: "CAFÉ".to_owned(),
            start_offset: 4,
            end_offset: 9,
            position_increment: 1,
        },
        Token {
            term: "İSTANBUL".to_owned(),
            start_offset: 12,
            end_offset: 21,
            position_increment: 3,
        },
    ];

    assert_eq!(
        lowercase_tokens(tokens),
        vec![
            Token {
                term: "café".to_owned(),
                start_offset: 4,
                end_offset: 9,
                position_increment: 1,
            },
            Token {
                term: "i\u{307}stanbul".to_owned(),
                start_offset: 12,
                end_offset: 21,
                position_increment: 3,
            },
        ]
    );
}

#[test]
fn lowercase_filter_matches_lucene_parity_fixture() {
    for line in LOWERCASE_FIXTURE.lines().skip(1) {
        let columns: Vec<&str> = line.split('\t').collect();
        assert_eq!(columns.len(), 5, "fixture row must have 5 columns: {line}");

        let token = Token {
            term: columns[0].to_owned(),
            start_offset: columns[1].parse().expect("start offset must be usize"),
            end_offset: columns[2].parse().expect("end offset must be usize"),
            position_increment: columns[3].parse().expect("position increment must be u32"),
        };

        assert_eq!(
            lowercase_tokens(vec![token]),
            vec![Token {
                term: columns[4].to_owned(),
                start_offset: columns[1].parse().expect("start offset must be usize"),
                end_offset: columns[2].parse().expect("end offset must be usize"),
                position_increment: columns[3].parse().expect("position increment must be u32"),
            }],
            "fixture row failed: {line}"
        );
    }
}
