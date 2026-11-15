//! MVP L1: `fuzzy` and `match`+`fuzziness` are served from the term
//! dictionary (Damerau-Levenshtein FST walk + Lucene top-terms selection +
//! postings union) instead of a `_source` full scan.
//!
//! * hit sets equal a brute-force oracle built from the index's own
//!   analyzer and `bounded_damerau_levenshtein`, on a single RAM segment, a
//!   multi-segment index and a disk-backed index — and the three layouts
//!   return bit-identical `(id, score)` lists;
//! * ES semantics: `match`+`fuzziness` is OR across tokens by default and
//!   honours `operator: and`; `prefix_length`, `max_expansions`,
//!   `fuzzy_transpositions` and `boost` are honoured;
//! * BM25 over the expanded terms ranks the exact spelling above 1-edit
//!   variants.
//!
//! The 200 k-doc candidate-count measurement is `#[ignore]`d (run with
//! `--release -- --ignored --nocapture`).

use std::collections::BTreeSet;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use serde_json::{json, Value};
use surch_api::{app_router_with_state, state::AppState, AppRouterState};
use surch_search::fuzzy::{bounded_damerau_levenshtein, edits_for_term_len, Fuzziness};
use tower::ServiceExt;

const BASE_NAMES: &[&str] = &[
    "dupont", "durand", "martin", "bernard", "petit", "lefevre", "leroy", "moreau", "dubois",
    "laurent", "simon", "michel", "garcia", "david", "bertrand", "roux", "vincent", "fournier",
    "morel", "girard", "andre", "mercier", "blanc", "guerin", "boyer", "henry",
];
const FIRST_NAMES: &[&str] = &[
    "jean", "pierre", "marie", "anne", "louis", "paul", "jacques", "michel", "andre", "rene",
    "helene", "eloise", "francois", "noel", "lea", "zoe",
];
const ALPHABET: &[char] = &['a', 'e', 'i', 'n', 'o', 'r', 's', 't', 'u', 'd'];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(rng: &mut Rng, word: &str) -> String {
    let mut chars: Vec<char> = word.chars().collect();
    let edits = rng.below(3);
    for _ in 0..edits {
        match rng.below(4) {
            0 => {
                let at = rng.below(chars.len() + 1);
                chars.insert(at, ALPHABET[rng.below(ALPHABET.len())]);
            }
            1 if chars.len() > 2 => {
                let at = rng.below(chars.len());
                chars.remove(at);
            }
            2 => {
                let at = rng.below(chars.len());
                chars[at] = ALPHABET[rng.below(ALPHABET.len())];
            }
            _ if chars.len() >= 2 => {
                let at = rng.below(chars.len() - 1);
                chars.swap(at, at + 1);
            }
            _ => {}
        }
    }
    chars.into_iter().collect()
}

/// Deterministic corpus: `nom` is one (possibly misspelt) surname,
/// `prenoms` one or two first names, `sexe` a keyword.
fn corpus(docs: usize, seed: u64) -> Vec<(String, Value)> {
    let mut rng = Rng(seed);
    (0..docs)
        .map(|doc| {
            let base = BASE_NAMES[rng.below(BASE_NAMES.len())];
            let nom = mutate(&mut rng, base);
            let mut prenoms = FIRST_NAMES[rng.below(FIRST_NAMES.len())].to_owned();
            if rng.below(2) == 0 {
                prenoms.push(' ');
                let second = FIRST_NAMES[rng.below(FIRST_NAMES.len())];
                prenoms.push_str(&mutate(&mut rng, second));
            }
            let sexe = if doc % 2 == 0 { "M" } else { "F" };
            (
                format!("d{doc:06}"),
                json!({"nom": nom, "prenoms": prenoms, "sexe": sexe}),
            )
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum Layout {
    SingleRam,
    MultiSegment,
    DiskBacked,
}

async fn response_json(response: axum::response::Response<Body>) -> Value {
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response body should be json")
}

async fn send(router: &axum::Router, method: Method, uri: &str, body: String) -> Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK, "{uri} failed");
    response_json(response).await
}

async fn build(index: &str, layout: Layout, docs: &[(String, Value)]) -> (axum::Router, AppState) {
    let state = AppState::default();
    state.create_index(index, None, json!({}), Default::default());
    state.set_merge_fanin_override(index, 0);
    match layout {
        Layout::SingleRam => state.set_flush_budget_bytes_override(index, None),
        Layout::MultiSegment => state.set_flush_budget_bytes_override(index, Some(1)),
        Layout::DiskBacked => {
            state.set_flush_budget_bytes_override(index, None);
            state.set_postings_disk_enabled(index, true);
        }
    }
    let router = app_router_with_state(AppRouterState {
        app: state.clone(),
        ..Default::default()
    });
    for chunk in docs.chunks(docs.len().div_ceil(6).max(1)) {
        let mut ndjson = String::new();
        for (id, source) in chunk {
            ndjson.push_str(&format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{source}\n"));
        }
        let body = send(&router, Method::POST, &format!("/{index}/_bulk"), ndjson).await;
        assert_eq!(body["errors"], false, "bulk failed: {body:?}");
    }
    send(
        &router,
        Method::POST,
        &format!("/{index}/_refresh"),
        String::new(),
    )
    .await;
    match layout {
        Layout::SingleRam => assert_eq!(state.index_segment_count(index), 1),
        Layout::MultiSegment => assert!(state.index_segment_count(index) > 1),
        Layout::DiskBacked => assert!(
            state.index_disk_postings_bytes(index).unwrap_or(0) > 0,
            "disk-backed layout must have persisted postings"
        ),
    }
    (router, state)
}

async fn search(router: &axum::Router, index: &str, body: Value) -> Value {
    send(
        router,
        Method::POST,
        &format!("/{index}/_search"),
        body.to_string(),
    )
    .await
}

fn hit_ids(body: &Value) -> BTreeSet<String> {
    body["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["_id"].as_str().expect("id").to_owned())
        .collect()
}

fn scored_hits(body: &Value) -> Vec<(String, u64)> {
    body["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| {
            (
                hit["_id"].as_str().expect("id").to_owned(),
                hit["_score"].as_f64().expect("score").to_bits(),
            )
        })
        .collect()
}

/// Brute-force oracle: ES `match`+`fuzziness` semantics without the
/// `max_expansions` cap (callers pass a large cap).
#[allow(clippy::too_many_arguments)] // test oracle: one argument per ES knob
fn oracle(
    state: &AppState,
    index: &str,
    docs: &[(String, Value)],
    field: &str,
    query: &str,
    fuzziness: Fuzziness,
    prefix_length: usize,
    require_all: bool,
) -> BTreeSet<String> {
    let mapping = state.index_mapping(index).expect("mapping");
    let analyzer = mapping.analyzer(field);
    let query_tokens = analyzer.terms(query);
    docs.iter()
        .filter(|(_, source)| {
            let field_tokens = analyzer.terms(source[field].as_str().unwrap_or_default());
            let token_ok = |token: &String| {
                let len = token.chars().count();
                let k = edits_for_term_len(fuzziness, len).expect("edits");
                field_tokens.iter().any(|candidate| {
                    if k == 0 || prefix_length >= len {
                        return candidate == token;
                    }
                    let prefix: String = token.chars().take(prefix_length).collect();
                    candidate.starts_with(&prefix)
                        && candidate.chars().count() >= prefix_length
                        && bounded_damerau_levenshtein(token, candidate, k, true)
                            .expect("k <= 2")
                            .is_some()
                })
            };
            if require_all {
                query_tokens.iter().all(token_ok)
            } else {
                query_tokens.iter().any(token_ok)
            }
        })
        .map(|(id, _)| id.clone())
        .collect()
}

#[tokio::test]
async fn fuzzy_hits_equal_oracle_and_are_identical_across_layouts() {
    let docs = corpus(1_200, 0x1234_5678_9abc_def1);
    let index = "fuzzy-parity";
    let mut layouts = Vec::new();
    for layout in [Layout::SingleRam, Layout::MultiSegment, Layout::DiskBacked] {
        layouts.push((layout, build(index, layout, &docs).await));
    }
    let cases: Vec<(&str, &str, Value, Fuzziness, usize, bool)> = vec![
        (
            "nom",
            "dupont",
            json!("auto"),
            Fuzziness::Auto { low: 3, high: 6 },
            0,
            false,
        ),
        (
            "nom",
            "durnad",
            json!("AUTO"),
            Fuzziness::Auto { low: 3, high: 6 },
            0,
            false,
        ),
        ("nom", "martin", json!(1), Fuzziness::Edits(1), 0, false),
        ("nom", "bernard", json!(2), Fuzziness::Edits(2), 2, false),
        (
            "nom",
            "roux",
            json!("auto"),
            Fuzziness::Auto { low: 3, high: 6 },
            0,
            false,
        ),
        (
            "prenoms",
            "jean pierre",
            json!("auto"),
            Fuzziness::Auto { low: 3, high: 6 },
            0,
            false,
        ),
        (
            "prenoms",
            "jean pierre",
            json!("auto"),
            Fuzziness::Auto { low: 3, high: 6 },
            0,
            true,
        ),
        (
            "prenoms",
            "marie helene",
            json!(1),
            Fuzziness::Edits(1),
            1,
            false,
        ),
    ];
    for (field, text, fuzziness_json, fuzziness, prefix_length, and) in cases {
        let mut inner = json!({
            "query": text,
            "fuzziness": fuzziness_json,
            "prefix_length": prefix_length,
            "max_expansions": 10_000,
        });
        if and {
            inner["operator"] = json!("and");
        }
        let body = json!({
            "query": {"match": {field: inner}},
            "size": 10_000,
            "track_total_hits": true,
        });
        let mut reference: Option<Vec<(String, u64)>> = None;
        for (layout, (router, state)) in &layouts {
            let response = search(router, index, body.clone()).await;
            let expected = oracle(
                state,
                index,
                &docs,
                field,
                text,
                fuzziness,
                prefix_length,
                and,
            );
            assert!(!expected.is_empty(), "{field}:{text} must match something");
            assert_eq!(
                hit_ids(&response),
                expected,
                "layout={layout:?} field={field} text={text} and={and}"
            );
            assert_eq!(
                response["hits"]["total"]["value"].as_u64(),
                Some(expected.len() as u64)
            );
            let scored = scored_hits(&response);
            match &reference {
                None => reference = Some(scored),
                Some(reference) => assert_eq!(
                    &scored, reference,
                    "layout={layout:?} must score bit-identically to single RAM ({field}:{text})"
                ),
            }
        }
    }
}

#[tokio::test]
async fn bool_of_fuzzy_clauses_matches_oracle_intersection() {
    let docs = corpus(800, 0xfeed_beef_0bad_cafe);
    let index = "fuzzy-bool";
    let (router, state) = build(index, Layout::MultiSegment, &docs).await;
    let auto = Fuzziness::Auto { low: 3, high: 6 };
    let noms = oracle(&state, index, &docs, "nom", "dupont", auto, 0, false);
    let prenoms = oracle(&state, index, &docs, "prenoms", "jean", auto, 0, false);
    let males: BTreeSet<String> = docs
        .iter()
        .filter(|(_, source)| source["sexe"] == "M")
        .map(|(id, _)| id.clone())
        .collect();
    let body = json!({
        "query": {"bool": {
            "must": [
                {"match": {"nom": {"query": "dupont", "fuzziness": "auto"}}},
                {"match": {"prenoms": {"query": "jean", "fuzziness": "auto"}}}
            ],
            "filter": [{"term": {"sexe": "M"}}]
        }},
        "size": 10_000,
    });
    let response = search(&router, index, body).await;
    let expected: BTreeSet<String> = noms
        .intersection(&prenoms)
        .filter(|id| males.contains(*id))
        .cloned()
        .collect();
    assert!(!expected.is_empty());
    assert_eq!(hit_ids(&response), expected);

    // `should` disjunction with minimum_should_match 1 (deces name query
    // shape): union of the two fuzzy clauses.
    let body = json!({
        "query": {"bool": {
            "should": [
                {"match": {"nom": {"query": "dupont", "fuzziness": "auto"}}},
                {"match": {"nom": {"query": "martin", "fuzziness": "auto"}}}
            ],
            "minimum_should_match": 1
        }},
        "size": 10_000,
    });
    let response = search(&router, index, body).await;
    let martins = oracle(&state, index, &docs, "nom", "martin", auto, 0, false);
    let expected: BTreeSet<String> = noms.union(&martins).cloned().collect();
    assert_eq!(hit_ids(&response), expected);
}

async fn small_index(index: &str, docs: &[(&str, Value)]) -> axum::Router {
    let docs: Vec<(String, Value)> = docs
        .iter()
        .map(|(id, source)| ((*id).to_owned(), source.clone()))
        .collect();
    build(index, Layout::SingleRam, &docs).await.0
}

#[tokio::test]
async fn match_fuzziness_is_or_across_tokens_by_default_and_honours_and() {
    let router = small_index(
        "fuzzy-op",
        &[
            ("both", json!({"prenoms": "jean pierre"})),
            ("jean", json!({"prenoms": "jean"})),
            ("pierre", json!({"prenoms": "pierre"})),
            ("other", json!({"prenoms": "louis"})),
        ],
    )
    .await;
    let or = search(
        &router,
        "fuzzy-op",
        json!({"query": {"match": {"prenoms": {"query": "jaen piere", "fuzziness": "auto"}}}}),
    )
    .await;
    assert_eq!(
        hit_ids(&or),
        ["both", "jean", "pierre"].map(str::to_owned).into()
    );
    assert_eq!(or["hits"]["hits"][0]["_id"], "both");
    let and = search(
        &router,
        "fuzzy-op",
        json!({"query": {"match": {"prenoms": {
            "query": "jaen piere", "fuzziness": "auto", "operator": "and"
        }}}}),
    )
    .await;
    assert_eq!(hit_ids(&and), ["both"].map(str::to_owned).into());
}

#[tokio::test]
async fn exact_spelling_outranks_variants_and_max_expansions_keeps_best_terms() {
    let router = small_index(
        "fuzzy-rank",
        &[
            ("exact", json!({"nom": "dupont"})),
            ("one-edit", json!({"nom": "dupond"})),
            ("transposed", json!({"nom": "dpuont"})),
            ("two-edits", json!({"nom": "dupintz"})),
        ],
    )
    .await;
    let all = search(
        &router,
        "fuzzy-rank",
        json!({"query": {"match": {"nom": {"query": "dupont", "fuzziness": 2}}}}),
    )
    .await;
    assert_eq!(all["hits"]["hits"][0]["_id"], "exact");
    let scores: Vec<f64> = all["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["_score"].as_f64().expect("score"))
        .collect();
    assert!(
        scores[0] > scores[1],
        "exact must outrank variants: {scores:?}"
    );

    // max_expansions = 2: Lucene keeps the best similarity terms —
    // `dupont` (1.0), then `dpuont` / `dupond` (1 edit, 0.833) by term
    // order: `dpuont` < `dupond`.
    let capped = search(
        &router,
        "fuzzy-rank",
        json!({"query": {"match": {"nom": {
            "query": "dupont", "fuzziness": 2, "max_expansions": 2
        }}}}),
    )
    .await;
    assert_eq!(
        hit_ids(&capped),
        ["exact", "transposed"].map(str::to_owned).into()
    );

    // Transpositions off: `dpuont` costs 2 edits, still within 2.
    let no_transpositions = search(
        &router,
        "fuzzy-rank",
        json!({"query": {"match": {"nom": {
            "query": "dupont", "fuzziness": 1, "fuzzy_transpositions": false
        }}}}),
    )
    .await;
    assert_eq!(
        hit_ids(&no_transpositions),
        ["exact", "one-edit"].map(str::to_owned).into()
    );

    // prefix_length = 2 excludes the variant whose first two chars differ.
    let prefixed = search(
        &router,
        "fuzzy-rank",
        json!({"query": {"match": {"nom": {
            "query": "dupont", "fuzziness": 2, "prefix_length": 2
        }}}}),
    )
    .await;
    assert_eq!(
        hit_ids(&prefixed),
        ["exact", "one-edit", "two-edits"].map(str::to_owned).into()
    );
}

#[tokio::test]
async fn boost_inside_match_objects_scales_the_score() {
    let router = small_index(
        "fuzzy-boost",
        &[
            ("a", json!({"nom": "dupont"})),
            ("b", json!({"nom": "martin"})),
        ],
    )
    .await;
    for (plain, boosted) in [
        (
            json!({"match": {"nom": {"query": "dupont", "fuzziness": "auto"}}}),
            json!({"match": {"nom": {"query": "dupont", "fuzziness": "auto", "boost": 3}}}),
        ),
        (
            json!({"match": {"nom": {"query": "dupont"}}}),
            json!({"match": {"nom": {"query": "dupont", "boost": 3}}}),
        ),
        (
            json!({"fuzzy": {"nom": {"value": "dupont"}}}),
            json!({"fuzzy": {"nom": {"value": "dupont", "boost": 3}}}),
        ),
    ] {
        let base = search(&router, "fuzzy-boost", json!({"query": plain})).await;
        let scaled = search(&router, "fuzzy-boost", json!({"query": boosted})).await;
        let base_score = base["hits"]["hits"][0]["_score"].as_f64().expect("score");
        let scaled_score = scaled["hits"]["hits"][0]["_score"].as_f64().expect("score");
        assert!(
            (scaled_score / base_score - 3.0).abs() < 1e-6,
            "{plain}: {base_score} -> {scaled_score}"
        );
    }
}

/// Measurement (run explicitly): candidate count of fuzzy shapes vs the
/// whole corpus on a synthetic 200 k-doc index, and wall time of the
/// postings path vs a shape that still full-scans (`wildcard`).
#[tokio::test]
#[ignore = "measurement: cargo test --release -p surch-api --test fuzzy_postings -- --ignored --nocapture"]
async fn measure_candidates_vs_full_scan_on_200k_docs() {
    let total = 200_000;
    let docs = corpus(total, 0xabad_1dea_2026_1115);
    let index = "fuzzy-200k";
    let started = std::time::Instant::now();
    let (router, _state) = build(index, Layout::SingleRam, &docs).await;
    println!("indexed {total} docs in {:?}", started.elapsed());
    let shapes = [
        (
            "match nom dupont fuzziness auto",
            json!({"match": {"nom": {"query": "dupont", "fuzziness": "auto"}}}),
        ),
        (
            "match prenoms 'jean pierre' fuzziness auto (OR)",
            json!({"match": {"prenoms": {"query": "jean pierre", "fuzziness": "auto"}}}),
        ),
        (
            "bool must[fuzzy nom, fuzzy prenoms] filter sexe",
            json!({"bool": {
                "must": [
                    {"match": {"nom": {"query": "dupont", "fuzziness": "auto"}}},
                    {"match": {"prenoms": {"query": "jean", "fuzziness": "auto"}}}
                ],
                "filter": [{"term": {"sexe": "M"}}]
            }}),
        ),
        (
            "full-scan baseline: wildcard nom dup*",
            json!({"wildcard": {"nom": "dup*"}}),
        ),
    ];
    for (label, query) in shapes {
        let body = json!({"query": query, "size": 10, "track_total_hits": true});
        // Warm-up then 5 timed runs (`request_cache` off: no cache hit).
        let uri_body = body.clone();
        search(&router, index, uri_body.clone()).await;
        let mut timings = Vec::new();
        let mut matched = 0;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let response = send(
                &router,
                Method::POST,
                &format!("/{index}/_search?request_cache=false"),
                uri_body.to_string(),
            )
            .await;
            timings.push(t.elapsed());
            matched = response["hits"]["total"]["value"].as_u64().unwrap_or(0);
        }
        timings.sort();
        println!(
            "{label}: candidates/matched {matched} of {total} docs ({:.3} %), median {:?}, max {:?}",
            matched as f64 * 100.0 / total as f64,
            timings[timings.len() / 2],
            timings[timings.len() - 1],
        );
    }
}
