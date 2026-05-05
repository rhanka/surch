use surch_search::fuzzy::{bounded_damerau_levenshtein, FuzzyError};

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
