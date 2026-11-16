//! Lot L7: `_score` parity with ES 8.6.1 (Lucene 9.4.2) on small fixtures.
//!
//! Every expected value is `lucene-source-derived`: it is re-derived here
//! from the Java expressions (ES `LegacyBM25Similarity.scorer`
//! l.66-67, Lucene `BM25Similarity` l.104-111/205-208/236/251,
//! `ConjunctionScorer`/`DisjunctionSumScorer` double sums,
//! `ReqOptSumScorer` float addition, `BoostQuery` boost threading,
//! `FuzzyTermsEnum` similarity, `BlendedTermQuery` max doc freq) with one
//! binary32 rounding per Java `float` operation, sharing no code with the
//! production kernel. Nothing here has been confirmed against a running
//! ES 8.6.1 yet: the differential gate is specified in
//! `docs/wp-d-matchid/l7-score-parity-gate.md`.

use serde_json::{json, Value};
use surch_index::mapping::IndexMapping;
use surch_search::small_float::{byte4_to_int, int_to_byte4};

use super::{parse_search_request, run_search};
use crate::state::AppState;

/// One binary32 rounding (binary64 op then `as f32` is the correctly rounded
/// binary32 result for `+ - * /`).
fn r(x: f64) -> f32 {
    x as f32
}

fn w(x: f32) -> f64 {
    f64::from(x)
}

/// ES 8.6.1 legacy BM25 of one term, re-derived from the Java source.
/// `len` is the raw field length of the document (SmallFloat-quantized
/// here), `None` for a field without norms (`LENGTH_TABLE[1]`).
fn oracle_bm25(
    doc_count: u64,
    doc_freq: u64,
    sum_ttf: u64,
    freq: u64,
    len: Option<u32>,
    boost: f32,
) -> f32 {
    let (k1, b) = (1.2_f32, 0.75_f32);
    let idf = r((1.0 + ((doc_count - doc_freq) as f64 + 0.5) / (doc_freq as f64 + 0.5)).ln());
    let avgdl = r(sum_ttf as f64 / doc_count as f64);
    let norm = len.map_or(1, int_to_byte4);
    let length = byte4_to_int(norm) as f32;
    let b_len_avg = r(w(r(w(b) * w(length))) / w(avgdl));
    let cache = r(1.0 / w(r(w(k1) * w(r(w(r(1.0 - w(b))) + w(b_len_avg))))));
    let legacy_boost = r(w(boost) * w(r(1.0 + w(k1))));
    let weight = r(w(legacy_boost) * w(idf));
    let denom = r(1.0 + w(r(freq as f64 * w(cache))));
    r(w(weight) - w(r(w(weight) / w(denom))))
}

/// Lucene `(float)` of a `double` sum.
fn oracle_double_sum(scores: &[f32]) -> f32 {
    r(scores.iter().map(|s| f64::from(*s)).sum::<f64>())
}

const INDEX: &str = "l7-scores";

/// Corpus (field lengths in tokens):
/// doc 0  NOM "dupont jean"          (2)  AGE 70  NOM.raw "dupont jean"
/// doc 1  NOM "dupont"               (1)  AGE 80  NOM.raw "dupont"
/// doc 2  NOM "martin dupont dupont" (3)  AGE 70  NOM.raw "martin dupont dupont"
/// doc 3  NOM "durand"               (1)  AGE 90  NOM.raw "durand"
/// NOM: docCount 4, sumTotalTermFreq 7, df(dupont) 3, df(martin) 1.
/// NOM.raw (keyword, no norms, DOCS): docCount 4, sumTotalTermFreq 4.
fn fixture() -> AppState {
    let state = AppState::default();
    let mapping = IndexMapping::from_properties_value(&json!({
        "NOM": {"type": "text", "fields": {"raw": {"type": "keyword"}}},
        "AGE": {"type": "short"},
        "MATCHED": {"type": "text"},
    }))
    .expect("valid mapping");
    state.create_index(INDEX, Some(mapping), json!({}), Default::default());
    let docs = [
        json!({"NOM": "dupont jean", "AGE": 70}),
        json!({"NOM": "dupont", "AGE": 80}),
        json!({"NOM": "martin dupont dupont", "AGE": 70}),
        json!({"NOM": "durand", "AGE": 90}),
    ];
    for (id, doc) in docs.into_iter().enumerate() {
        state.index_document(INDEX, &id.to_string(), doc);
    }
    state.refresh_index(INDEX);
    state
}

/// `(id, _score as f32 bits)` of every hit, and `max_score` bits.
fn search(state: &AppState, body: Value) -> (Vec<(String, u32)>, Option<u32>) {
    let request = parse_search_request(&body.to_string()).expect("valid request");
    let response = run_search(state, &[INDEX.to_owned()], &request);
    let hits = response
        .hits
        .hits
        .iter()
        .map(|hit| {
            let score = hit["_score"].as_f64().expect("scored hit");
            // The f64 carried by the response is an exact f32 widening.
            assert_eq!(f64::from(score as f32), score, "score is an f32 value");
            (
                hit["_id"].as_str().expect("id").to_owned(),
                (score as f32).to_bits(),
            )
        })
        .collect();
    let max = response.hits.max_score.map(|max| (max as f32).to_bits());
    (hits, max)
}

fn bits(entries: &[(&str, f32)]) -> Vec<(String, u32)> {
    entries
        .iter()
        .map(|(id, score)| ((*id).to_owned(), score.to_bits()))
        .collect()
}

fn dupont(doc_tf_len: (u64, u32), boost: f32) -> f32 {
    oracle_bm25(4, 3, 7, doc_tf_len.0, Some(doc_tf_len.1), boost)
}

#[test]
fn single_term_match_scores_and_ties_break_on_doc_id() {
    let state = fixture();
    let (hits, max) = search(&state, json!({"query": {"match": {"NOM": "dupont"}}}));
    let d0 = dupont((1, 2), 1.0);
    let d1 = dupont((1, 1), 1.0);
    let d2 = dupont((2, 3), 1.0);
    let mut expected = vec![("0", d0), ("1", d1), ("2", d2)];
    // HitQueue.lessThan: score descending, then doc id ascending.
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    assert_eq!(hits, bits(&expected));
    assert_eq!(max, Some(expected[0].1.to_bits()));
}

#[test]
fn must_plus_should_is_req_opt_float_addition() {
    let state = fixture();
    let (hits, _) = search(
        &state,
        json!({"query": {"bool": {
            "must": [{"match": {"NOM": "dupont"}}],
            "should": [{"match": {"NOM": "martin"}}]
        }}}),
    );
    let martin = oracle_bm25(4, 1, 7, 1, Some(3), 1.0);
    // ReqOptSumScorer: `float score = req; score += opt;`
    let d2 = dupont((2, 3), 1.0) + martin;
    let d0 = dupont((1, 2), 1.0);
    let d1 = dupont((1, 1), 1.0);
    let mut expected = vec![("0", d0), ("1", d1), ("2", d2)];
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    assert_eq!(hits, bits(&expected));
}

#[test]
fn bool_boost_threads_into_term_weights_and_keyword_subfield_scores() {
    let state = fixture();
    let (hits, _) = search(
        &state,
        json!({"query": {"bool": {
            "boost": 2,
            "should": [
                {"match": {"NOM": "dupont"}},
                {"match": {"NOM.raw": "dupont"}}
            ]
        }}}),
    );
    // NOM.raw: keyword, no norms, docCount 4, sumTotalTermFreq 4, df 1.
    let raw = oracle_bm25(4, 1, 4, 1, None, 2.0);
    let d1 = oracle_double_sum(&[dupont((1, 1), 2.0), raw]);
    let d0 = dupont((1, 2), 2.0);
    let d2 = dupont((2, 3), 2.0);
    let mut expected = vec![("0", d0), ("1", d1), ("2", d2)];
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    assert_eq!(hits, bits(&expected));
}

#[test]
fn match_on_numeric_field_is_constant_score() {
    let state = fixture();
    let (hits, max) = search(&state, json!({"query": {"match": {"AGE": 70}}}));
    assert_eq!(hits, bits(&[("0", 1.0), ("2", 1.0)]));
    assert_eq!(max, Some(1.0_f32.to_bits()));
}

#[test]
fn constant_score_should_clause_counts_only_when_it_matches() {
    let state = fixture();
    let (hits, _) = search(
        &state,
        json!({"query": {"bool": {
            "must": [{"match": {"NOM": "dupont"}}],
            "should": [{"prefix": {"NOM": "mart"}}]
        }}}),
    );
    let d2 = dupont((2, 3), 1.0) + 1.0;
    let d0 = dupont((1, 2), 1.0);
    let d1 = dupont((1, 1), 1.0);
    let mut expected = vec![("0", d0), ("1", d1), ("2", d2)];
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    assert_eq!(hits, bits(&expected));
}

#[test]
fn repeated_token_boosts_the_weight_not_the_score() {
    let state = fixture();
    let (hits, _) = search(
        &state,
        json!({"query": {"match": {"NOM": "dupont dupont dupont"}}}),
    );
    // BooleanQuery.rewrite: three identical SHOULD clauses => BoostQuery(3f).
    let d0 = dupont((1, 2), 3.0);
    let d1 = dupont((1, 1), 3.0);
    let d2 = dupont((2, 3), 3.0);
    let mut expected = vec![("0", d0), ("1", d1), ("2", d2)];
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    assert_eq!(hits, bits(&expected));
}

/// `FuzzyTermsEnum.next` (Lucene 9.4.2 l.248-255) similarity boost:
/// `1.0f - (float) ed / (float) Math.min(codePointCount(term), termLength)`.
fn fuzzy_similarity(ed: u32, term_len: u32, query_len: u32) -> f32 {
    r(1.0
        - w(r(
            f64::from(ed as f32) / f64::from(term_len.min(query_len) as f32)
        )))
}

#[test]
fn fuzzy_match_uses_similarity_boost_and_blended_max_doc_freq() {
    let state = fixture();
    let (hits, _) = search(
        &state,
        json!({"query": {"match": {"NOM": {"query": "dupond", "fuzziness": "auto"}}}}),
    );
    // AUTO on 6 chars => 2 edits. Kept terms: dupont (ed 1), durand (ed 2).
    // BlendedTermQuery.rewrite: df = max(df(dupont)=3, df(durand)=1) = 3.
    let dupont_boost = fuzzy_similarity(1, 6, 6);
    let durand_boost = fuzzy_similarity(2, 6, 6);
    // BoostQuery(TermQuery, similarity).createWeight(1f) => similarity * 1f.
    let d0 = oracle_bm25(4, 3, 7, 1, Some(2), dupont_boost);
    let d1 = oracle_bm25(4, 3, 7, 1, Some(1), dupont_boost);
    let d2 = oracle_bm25(4, 3, 7, 2, Some(3), dupont_boost);
    let d3 = oracle_bm25(4, 3, 7, 1, Some(1), durand_boost);
    let mut expected = vec![("0", d0), ("1", d1), ("2", d2), ("3", d3)];
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    assert_eq!(hits, bits(&expected));
}

#[test]
fn boosted_fuzzy_clause_multiplies_similarity_by_the_clause_boost() {
    let state = fixture();
    let (hits, _) = search(
        &state,
        json!({"query": {"match": {"NOM": {"query": "durant", "fuzziness": 1, "boost": 2}}}}),
    );
    // durant -> durand (ed 1); dupont is 2 edits away (excluded).
    let boost = r(w(fuzzy_similarity(1, 6, 6)) * 2.0);
    let d3 = oracle_bm25(4, 1, 7, 1, Some(1), boost);
    assert_eq!(hits, bits(&[("3", d3)]));
}

#[test]
fn function_score_without_functions_threads_its_boost_into_the_sub_query() {
    let state = fixture();
    let (hits, _) = search(
        &state,
        json!({"query": {"function_score": {"query": {"match": {"NOM": "dupont"}}, "boost": 2}}}),
    );
    // BoostQuery(FunctionScoreQuery, 2).createWeight => sub-query weight
    // boost 2 (FunctionScoreQuery.createWeight l.245); no function => the
    // sub-query score (CustomBoostFactorScorer.score l.408-410).
    let d0 = dupont((1, 2), 2.0);
    let d1 = dupont((1, 1), 2.0);
    let d2 = dupont((2, 3), 2.0);
    let mut expected = vec![("0", d0), ("1", d1), ("2", d2)];
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    assert_eq!(hits, bits(&expected));
}

#[test]
fn function_score_combines_in_double_and_casts_once() {
    let state = fixture();
    let (hits, _) = search(
        &state,
        json!({"query": {"function_score": {
            "query": {"match": {"NOM": "dupont"}},
            "functions": [
                {"field_value_factor": {"field": "AGE", "modifier": "log1p", "factor": 1}}
            ],
            "boost_mode": "multiply"
        }}}),
    );
    // FieldValueFactorFunction: LOG1P = Math.log10(value * factor + 1);
    // CombineFunction.MULTIPLY: (float) (queryScore * min(funcScore, maxBoost)).
    let combine = |score: f32, age: f64| r(f64::from(score) * (age + 1.0).log10());
    let d0 = combine(dupont((1, 2), 1.0), 70.0);
    let d1 = combine(dupont((1, 1), 1.0), 80.0);
    let d2 = combine(dupont((2, 3), 1.0), 70.0);
    let mut expected = vec![("0", d0), ("1", d1), ("2", d2)];
    expected.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    assert_eq!(hits, bits(&expected));
}

#[test]
fn min_score_compares_float_values() {
    let state = fixture();
    let d1 = dupont((1, 1), 1.0);
    // `min_score` equal to a hit's float score keeps it (`>=`).
    let (hits, _) = search(
        &state,
        json!({"min_score": f64::from(d1), "query": {"match": {"NOM": "dupont"}}}),
    );
    assert!(hits.iter().any(|(id, _)| id == "1"));
}
