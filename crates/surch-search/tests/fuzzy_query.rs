use surch_search::fuzzy::{
    bounded_damerau_levenshtein, edits_for_term_len, parse_fuzziness, Fuzziness, FuzzyError,
    FuzzyQueryConfig,
};

#[test]
fn fuzzy_query_returns_zero_for_identical_terms() {
    let distance = bounded_damerau_levenshtein("surch", "surch", 2, true);

    assert_eq!(distance, Ok(Some(0)));
}

#[test]
fn fuzzy_query_returns_one_for_single_insertion_deletion_or_substitution() {
    assert_eq!(
        bounded_damerau_levenshtein("surch", "surche", 2, true),
        Ok(Some(1))
    );
    assert_eq!(
        bounded_damerau_levenshtein("surch", "such", 2, true),
        Ok(Some(1))
    );
    assert_eq!(
        bounded_damerau_levenshtein("surch", "surgh", 2, true),
        Ok(Some(1))
    );
}

#[test]
fn fuzzy_query_returns_two_for_two_edits() {
    assert_eq!(
        bounded_damerau_levenshtein("kitten", "sittin", 2, true),
        Ok(Some(2))
    );
}

#[test]
fn fuzzy_query_returns_none_when_distance_exceeds_bound() {
    assert_eq!(
        bounded_damerau_levenshtein("surch", "elastic", 2, true),
        Ok(None)
    );
}

#[test]
fn fuzzy_query_rejects_max_edits_above_lucene_limit() {
    let distance = bounded_damerau_levenshtein("surch", "surche", 3, true);

    assert_eq!(distance, Err(FuzzyError::MaxEditsTooLarge { max_edits: 3 }));
}

#[test]
fn fuzzy_query_counts_adjacent_transposition_as_one_when_enabled() {
    assert_eq!(
        bounded_damerau_levenshtein("surch", "sruch", 1, true),
        Ok(Some(1))
    );
}

#[test]
fn fuzzy_query_counts_adjacent_transposition_as_two_when_disabled() {
    assert_eq!(
        bounded_damerau_levenshtein("surch", "sruch", 1, false),
        Ok(None)
    );
    assert_eq!(
        bounded_damerau_levenshtein("surch", "sruch", 2, false),
        Ok(Some(2))
    );
}

#[test]
fn fuzzy_query_compares_unicode_scalar_values_not_bytes() {
    assert_eq!(
        bounded_damerau_levenshtein("cafe", "cafe\u{301}", 2, true),
        Ok(Some(1))
    );
    assert_eq!(
        bounded_damerau_levenshtein("éclair", "écliar", 1, true),
        Ok(Some(1))
    );
}

#[test]
fn fuzzy_query_parse_fuzziness_accepts_auto_default_thresholds() {
    assert_eq!(
        parse_fuzziness("AUTO"),
        Ok(Fuzziness::Auto { low: 3, high: 6 })
    );
}

#[test]
fn fuzzy_query_parse_fuzziness_accepts_auto_custom_thresholds() {
    assert_eq!(
        parse_fuzziness("AUTO:4,7"),
        Ok(Fuzziness::Auto { low: 4, high: 7 })
    );
}

#[test]
fn fuzzy_query_parse_fuzziness_rejects_edits_above_lucene_limit() {
    assert_eq!(
        parse_fuzziness("3"),
        Err(FuzzyError::MaxEditsTooLarge { max_edits: 3 })
    );
}

#[test]
fn fuzzy_query_auto_fuzziness_matches_opensearch_lucene_thresholds() {
    let fuzziness = Fuzziness::Auto { low: 3, high: 6 };

    assert_eq!(edits_for_term_len(fuzziness, 2), Ok(0));
    assert_eq!(edits_for_term_len(fuzziness, 3), Ok(1));
    assert_eq!(edits_for_term_len(fuzziness, 5), Ok(1));
    assert_eq!(edits_for_term_len(fuzziness, 6), Ok(2));
}

#[test]
fn fuzzy_query_auto_fuzziness_uses_custom_thresholds() {
    let fuzziness = Fuzziness::Auto { low: 4, high: 7 };

    assert_eq!(edits_for_term_len(fuzziness, 3), Ok(0));
    assert_eq!(edits_for_term_len(fuzziness, 4), Ok(1));
    assert_eq!(edits_for_term_len(fuzziness, 6), Ok(1));
    assert_eq!(edits_for_term_len(fuzziness, 7), Ok(2));
}

#[test]
fn fuzzy_query_config_rejects_zero_max_expansions() {
    let config = FuzzyQueryConfig::new(Fuzziness::Edits(1), 0, 0, true);

    assert_eq!(config, Err(FuzzyError::MaxExpansionsZero));
}

#[test]
fn fuzzy_query_config_rejects_edits_above_lucene_limit() {
    let config = FuzzyQueryConfig::new(Fuzziness::Edits(3), 0, 50, true);

    assert_eq!(config, Err(FuzzyError::MaxEditsTooLarge { max_edits: 3 }));
}

#[test]
fn fuzzy_query_classic_fixture_matches_expected_edit_distances() {
    let fixture = include_str!("../../../tests/lucene_parity/search/fuzzy_classic_cases.tsv");

    for (line_number, line) in fixture.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut columns = line.split('\t');
        let left = columns.next().expect("fixture left term");
        let right = columns.next().expect("fixture right term");
        let expected_edits = columns
            .next()
            .expect("fixture expected edits")
            .parse::<u8>()
            .expect("fixture expected edits is u8");
        assert_eq!(
            columns.next(),
            None,
            "unexpected extra fixture column on line {}",
            line_number + 1
        );

        assert_eq!(
            bounded_damerau_levenshtein(left, right, 2, true),
            Ok(Some(expected_edits)),
            "fixture line {}",
            line_number + 1
        );
    }
}
