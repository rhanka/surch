use surch_search::fuzzy::{Fuzziness, FuzzyQueryConfig};
use surch_search::query::{
    rewrite_fuzzy_query, BooleanQuery, FuzzyQuery, Query, QueryError, TermQuery,
};

fn default_config() -> FuzzyQueryConfig {
    FuzzyQueryConfig::new(Fuzziness::Edits(1), 1, 3, true).expect("valid fuzzy config")
}

#[test]
fn term_query_rejects_empty_field() {
    let query = TermQuery::new("", "surch");

    assert_eq!(query, Err(QueryError::EmptyField));
}

#[test]
fn term_query_rejects_empty_value() {
    let query = TermQuery::new("title", "");

    assert_eq!(query, Err(QueryError::EmptyValue));
}

#[test]
fn fuzzy_query_rejects_empty_field() {
    let query = FuzzyQuery::new("", "surch", default_config());

    assert_eq!(query, Err(QueryError::EmptyField));
}

#[test]
fn fuzzy_query_rejects_empty_value() {
    let query = FuzzyQuery::new("title", "", default_config());

    assert_eq!(query, Err(QueryError::EmptyValue));
}

#[test]
fn boolean_query_accepts_non_empty_must_clauses_and_preserves_order() {
    let term_query = Query::Term(TermQuery::new("title", "surch").expect("term query"));
    let fuzzy_query =
        Query::Fuzzy(FuzzyQuery::new("body", "search", default_config()).expect("fuzzy query"));

    let query =
        BooleanQuery::new(vec![term_query.clone(), fuzzy_query.clone()]).expect("bool query");

    assert_eq!(query.must, vec![term_query, fuzzy_query]);
}

#[test]
fn boolean_query_rejects_empty_must_clauses() {
    let query = BooleanQuery::new(Vec::new());

    assert_eq!(query, Err(QueryError::EmptyMustClauses));
}

#[test]
fn query_enum_wraps_boolean_query() {
    let bool_query = BooleanQuery::new(vec![Query::Term(
        TermQuery::new("title", "surch").expect("term query"),
    )])
    .expect("bool query");

    assert_eq!(
        Query::Boolean(bool_query.clone()),
        Query::Boolean(bool_query)
    );
}

#[test]
fn fuzzy_query_rewrite_returns_term_queries_for_expanded_terms() {
    let query = FuzzyQuery::new("title", "surch", default_config()).expect("valid fuzzy query");

    let rewritten = rewrite_fuzzy_query(
        &query,
        [
            "surch", "sruch", "such", "surgh", "lurch", "search", "Surch",
        ],
    )
    .expect("rewrite succeeds");

    assert_eq!(
        rewritten,
        vec![
            TermQuery::new("title", "surch").expect("term query"),
            TermQuery::new("title", "sruch").expect("term query"),
            TermQuery::new("title", "such").expect("term query"),
        ]
    );
}

#[test]
fn fuzzy_query_rewrite_preserves_field_for_every_expanded_term() {
    let query = FuzzyQuery::new("title", "surch", default_config()).expect("valid fuzzy query");

    let rewritten = rewrite_fuzzy_query(&query, ["surch", "sruch", "such"]).expect("rewrite");

    assert!(rewritten
        .iter()
        .all(|term_query| term_query.field == "title"));
}

#[test]
fn fuzzy_query_classic_terms_fixture_matches_rewrite_results() {
    let fixture = include_str!("../../../tests/lucene_parity/search/query_classic_terms.tsv");
    let mut terms = Vec::new();
    let mut expected = Vec::new();

    for (line_number, line) in fixture.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut columns = line.split('\t');
        let field = columns.next().expect("fixture field");
        let query_value = columns.next().expect("fixture query");
        let term = columns.next().expect("fixture term");
        let should_match = columns
            .next()
            .expect("fixture expected match")
            .parse::<bool>()
            .expect("fixture expected match is bool");
        assert_eq!(
            columns.next(),
            None,
            "unexpected extra fixture column on line {}",
            line_number + 1
        );

        assert_eq!(field, "title", "fixture line {}", line_number + 1);
        assert_eq!(query_value, "surch", "fixture line {}", line_number + 1);

        terms.push(term);
        if should_match {
            expected.push(TermQuery::new(field, term).expect("term query"));
        }
    }

    let query = FuzzyQuery::new("title", "surch", default_config()).expect("valid fuzzy query");
    let rewritten = rewrite_fuzzy_query(&query, terms).expect("rewrite succeeds");

    assert_eq!(rewritten, expected);
}
