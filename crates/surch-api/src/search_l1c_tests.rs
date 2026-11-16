//! MVP L1c guards.
//!
//! 1. Aggregations served from doc values
//!    ([`super::doc_values_aggs::run_doc_values_aggregations`]) return byte
//!    for byte what the general path returns (hydrate every matched
//!    document, read `_source`), for the deces-backend `/agg` shapes, on
//!    RAM / multi-segment / disk-backed layouts, with deleted documents,
//!    missing values, normalised keywords, integers, dates and composite
//!    `after` paging; fields whose values do not provably rebuild from the
//!    index keep the general path.
//! 2. The merged fuzzy pass ([`super::batch_fuzzy_scores`]) gives every
//!    candidate the same `f32` score bits as the per-document scorer.

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::doc_values_aggs::{
    set_doc_values_aggs_override_for_test, take_doc_values_aggs_served_for_test,
};
use super::{
    parse_search_request, posting_candidates, run_search, score_for_query,
    set_fuzzy_batch_disabled_for_test, CandidateCtx, ScoringArena, SearchScoringContext,
};
use crate::{app_router_with_state, state::AppState, AppRouterState};

const DECES_INDEX_REAL: &str =
    include_str!("../../../tests/matchid_compat/deces/mapping_real.json");
const INDEX: &str = "deces";

#[derive(Clone, Copy, Debug)]
enum Layout {
    /// One RAM segment, documents deleted after the bulk (full rebuild).
    SingleRamWithDeletes,
    /// One segment per bulk chunk (flush budget 1 byte), RAM postings.
    MultiSegment,
    /// Disk-backed postings and spilled sub-field columns, with deletes.
    DiskBackedWithDeletes,
    /// Disk-backed and one segment per bulk chunk.
    DiskMultiSegment,
}

const LAYOUTS: [Layout; 4] = [
    Layout::SingleRamWithDeletes,
    Layout::MultiSegment,
    Layout::DiskBackedWithDeletes,
    Layout::DiskMultiSegment,
];

async fn send(router: &axum::Router, method: Method, uri: &str, body: String) -> Value {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body readable");
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

const NOMS: &[&str] = &["DUPONT", "DUPOND", "Martin", "MATRIN", "Le Gall", "N'DIAYE"];
const PRENOMS: &[&str] = &["JEAN PIERRE", "Jean", "MARIE", "Pierre", "JÉRÔME", "JEANNE"];
const COMMUNES: &[&str] = &["Paris", "Saint-Étienne", "Ajaccio", "BASTIA", "lyon"];
const DEPARTEMENTS: &[&str] = &["75", "42", "2A", "2b", "69", "06"];

/// Deces-like documents: some fields missing, keywords needing the `norm`
/// normalizer (`2A` -> `2a`), integer ages, `yyyyMMdd` dates (one
/// unparsable), sub-field columns on `NOM` / `COMMUNE_DECES`.
fn corpus(total: usize) -> Vec<(String, Value)> {
    let mut seed: u64 = 0x11c0_2026_1115_0001;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    (0..total)
        .map(|n| {
            let nom = NOMS[next(NOMS.len() as u64) as usize];
            let prenoms = PRENOMS[next(PRENOMS.len() as u64) as usize];
            let source_value = ["2020-M01", "2020-m02"][next(2) as usize];
            let mut source = json!({
                "UID": format!("uid{n:05}"),
                "NOM": nom,
                "PRENOM": prenoms.split(' ').next().unwrap_or(prenoms),
                "PRENOMS": prenoms,
                "PRENOMS_NOM": format!("{prenoms} {nom}"),
                "PRENOM_NOM": format!("{prenoms} {nom}"),
                "SOURCE": source_value,
            });
            if n % 9 != 4 {
                source["SEXE"] = json!(if next(2) == 0 { "M" } else { "F" });
            }
            if n % 11 != 5 {
                source["DEPARTEMENT_DECES"] =
                    json!(DEPARTEMENTS[next(DEPARTEMENTS.len() as u64) as usize]);
            }
            if n % 13 != 6 {
                source["COMMUNE_DECES"] = json!(COMMUNES[next(COMMUNES.len() as u64) as usize]);
            }
            if n % 7 != 3 {
                source["AGE_DECES"] = json!(20 + next(80));
            }
            let year = 1950 + next(70);
            let date = if n == 17 {
                "00000000".to_owned()
            } else {
                format!("{year}{:02}{:02}", 1 + next(12), 1 + next(28))
            };
            if n % 5 != 2 {
                source["DATE_DECES_NORM"] = json!(date);
                source["DATE_DECES"] = json!(date);
            }
            if n % 6 != 1 {
                let birth = format!("{}{:02}{:02}", year - 30, 1 + next(12), 1 + next(28));
                source["DATE_NAISSANCE_NORM"] = json!(birth);
                source["DATE_NAISSANCE"] = json!(birth);
            }
            (format!("d{n:05}"), source)
        })
        .collect()
}

async fn build(layout: Layout, docs: &[(String, Value)], extra: &[(String, Value)]) -> AppState {
    let state = AppState::default();
    let router = app_router_with_state(AppRouterState {
        app: state.clone(),
        ..Default::default()
    });
    let created = send(
        &router,
        Method::PUT,
        &format!("/{INDEX}"),
        DECES_INDEX_REAL.to_owned(),
    )
    .await;
    assert_eq!(created["acknowledged"], true, "PUT /{INDEX}: {created}");
    state.set_merge_fanin_override(INDEX, 0);
    let (multi, disk, deletes) = match layout {
        Layout::SingleRamWithDeletes => (false, false, true),
        Layout::MultiSegment => (true, false, false),
        Layout::DiskBackedWithDeletes => (false, true, true),
        Layout::DiskMultiSegment => (true, true, false),
    };
    state.set_flush_budget_bytes_override(INDEX, multi.then_some(1));
    if disk {
        state.set_postings_disk_enabled(INDEX, true);
    }
    let all: Vec<&(String, Value)> = docs.iter().chain(extra).collect();
    for chunk in all.chunks(all.len().div_ceil(4).max(1)) {
        let mut ndjson = String::new();
        for (id, source) in chunk {
            ndjson.push_str(&format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{source}\n"));
        }
        let body = send(&router, Method::POST, &format!("/{INDEX}/_bulk"), ndjson).await;
        assert_eq!(body["errors"], false, "bulk failed: {body:?}");
    }
    if deletes {
        for id in ["d00000", "d00001", "d00017", "d00100", "d00333"] {
            send(
                &router,
                Method::DELETE,
                &format!("/{INDEX}/_doc/{id}"),
                String::new(),
            )
            .await;
        }
    }
    send(
        &router,
        Method::POST,
        &format!("/{INDEX}/_refresh"),
        String::new(),
    )
    .await;
    state
}

fn serialized(mut response: super::SearchResponse) -> String {
    response.took = 0;
    String::from_utf8(crate::es_json::to_es_json_vec(&response).expect("serializes")).expect("utf8")
}

/// The general (`_source`) path and the doc-values path on `body`;
/// returns both serialized responses and whether doc values served it.
fn both_paths(state: &AppState, body: &Value) -> (String, String, bool) {
    let request = parse_search_request(&body.to_string()).expect("valid request");
    let indices = vec![INDEX.to_owned()];
    set_doc_values_aggs_override_for_test(Some(false));
    let reference = serialized(run_search(state, &indices, &request));
    take_doc_values_aggs_served_for_test();
    set_doc_values_aggs_override_for_test(Some(true));
    let fast = serialized(run_search(state, &indices, &request));
    set_doc_values_aggs_override_for_test(None);
    let served = take_doc_values_aggs_served_for_test() > 0;
    (reference, fast, served)
}

fn fuzzy_should(field: &str, value: &str) -> Value {
    json!({"bool": {"must": [{"match": {field: {"query": value, "fuzziness": "auto"}}}],
                    "should": [{"match": {format!("{field}.raw"): value}}]}})
}

/// deces-backend `nameQuery` (fuzzy) for first + last name.
fn name_query(first: &str, last: &str) -> Value {
    let first_name = |value: &str| {
        json!({"bool": {
        "must": [{"match": {"PRENOMS": {"query": value, "fuzziness": "auto"}}}],
        "should": [
            {"match": {"PRENOM": {"query": value, "boost": 4}}},
            {"match": {"PRENOM": {"query": value, "fuzziness": "auto", "boost": 2}}},
            {"match": {"PRENOMS": value}}
        ]}})
    };
    json!({"bool": {"minimum_should_match": 1, "should": [
        {"bool": {"should": [first_name(first), fuzzy_should("NOM", last)],
                  "minimum_should_match": 2, "boost": 2}},
        {"bool": {"should": [first_name(last), fuzzy_should("NOM", first)],
                  "minimum_should_match": 2, "boost": 0.5}}
    ]}})
}

/// `buildRequest` envelope, `size: 0`.
fn agg_body(matched: Value, min_score: f64, aggs: Value) -> Value {
    json!({
        "min_score": min_score, "track_total_hits": true,
        "_source": ["UID", "NOM"],
        "query": {"bool": {"must": [matched]}},
        "sort": [{"_score": "desc"}], "size": 0,
        "aggs": aggs,
    })
}

fn no_criteria() -> Value {
    json!({"function_score": {"query": {"bool": {"must": []}}}})
}

fn histogram(field: &str) -> Value {
    json!({"date_histogram": {"field": field, "calendar_interval": "month", "format": "yyyyMMdd"}})
}

/// deces-backend `buildAggregation` composite (+ cardinalities on page 1).
fn composite(sources: &[(&str, &str)], after: Option<&Value>) -> Value {
    let list: Vec<Value> = sources
        .iter()
        .map(|(name, field)| {
            if field.ends_with("_NORM") {
                json!({*name: histogram(field)})
            } else {
                json!({*name: {"terms": {"field": field}}})
            }
        })
        .collect();
    let mut aggs = json!({"bucketResults": {"composite": {"size": 7, "sources": list}}});
    match after {
        Some(after) => aggs["bucketResults"]["composite"]["after"] = after.clone(),
        None => {
            for (name, field) in sources {
                aggs[format!("{name}_count")] = json!({"cardinality": {"field": field}});
            }
        }
    }
    aggs
}

fn criteria() -> Vec<(&'static str, Value, f64)> {
    vec![
        ("no criteria", no_criteria(), 0.0),
        ("no query", Value::Null, 0.0),
        ("name fuzzy", name_query("Jean", "Dupont"), 0.0),
        (
            "fuzzy prenoms min_score 1.5",
            json!({"match": {"PRENOMS": {"query": "jean pierre", "fuzziness": "auto"}}}),
            1.5,
        ),
        (
            "death range + sex",
            json!({"bool": {"must": [
                {"range": {"DATE_DECES": {"gte": "19900000"}}},
                {"match": {"SEXE": "F"}}
            ]}}),
            0.0,
        ),
        (
            "match_all min_score above the constant",
            json!({"match_all": {}}),
            2.0,
        ),
    ]
}

fn single_aggs() -> Vec<Value> {
    vec![
        json!({"deathDepartment": {"terms": {"field": "DEPARTEMENT_DECES", "size": 4}}}),
        json!({"sex": {"terms": {"field": "SEXE", "size": 10}}}),
        json!({"deathAge": {"terms": {"field": "AGE_DECES", "size": 15}}}),
        json!({"lastName": {"terms": {"field": "NOM.raw", "size": 3}}}),
        json!({"deathCity": {"terms": {"field": "COMMUNE_DECES.raw", "size": 100}}}),
        json!({"source": {"terms": {"field": "SOURCE", "size": 100}}}),
        json!({"deathDate": histogram("DATE_DECES_NORM")}),
        json!({"birthDate": histogram("DATE_NAISSANCE_NORM")}),
        json!({"rawDate": {"date_histogram": {"field": "DATE_DECES.raw", "calendar_interval": "year"}}}),
        json!({"c1": {"cardinality": {"field": "COMMUNE_DECES.raw"}},
               "c2": {"cardinality": {"field": "AGE_DECES"}},
               "c3": {"cardinality": {"field": "DEPARTEMENT_DECES"}}}),
    ]
}

fn composite_sets() -> Vec<Vec<(&'static str, &'static str)>> {
    vec![
        vec![("sex", "SEXE"), ("deathDepartment", "DEPARTEMENT_DECES")],
        vec![
            ("deathCity", "COMMUNE_DECES.raw"),
            ("deathDate", "DATE_DECES_NORM"),
        ],
        vec![
            ("deathAge", "AGE_DECES"),
            ("sex", "SEXE"),
            ("lastName", "NOM.raw"),
        ],
    ]
}

async fn assert_paths_agree(state: &AppState, layout: Layout, expect_served: bool) {
    for (label, matched, min_score) in criteria() {
        let make = |aggs: Value| {
            let mut body = agg_body(matched.clone(), min_score, aggs);
            if matched.is_null() {
                body.as_object_mut().expect("object").remove("query");
            }
            body
        };
        for aggs in single_aggs() {
            let body = make(aggs);
            let (reference, fast, served) = both_paths(state, &body);
            assert_eq!(fast, reference, "{layout:?} {label}: {body}");
            assert_eq!(served, expect_served, "{layout:?} {label}: {body}");
        }
        for sources in composite_sets() {
            // Page through the whole composite like deces-backend does.
            let mut after: Option<Value> = None;
            for page in 0..200 {
                let body = make(composite(&sources, after.as_ref()));
                let (reference, fast, served) = both_paths(state, &body);
                assert_eq!(fast, reference, "{layout:?} {label} page {page}: {body}");
                assert_eq!(served, expect_served, "{layout:?} {label}: {body}");
                let parsed: Value = serde_json::from_str(&fast).expect("json");
                match parsed["aggregations"]["bucketResults"].get("after_key") {
                    Some(key) => after = Some(key.clone()),
                    None => break,
                }
                assert!(page < 199, "composite paging never ended");
            }
        }
    }
}

#[tokio::test]
async fn doc_values_aggregations_equal_the_source_path() {
    let docs = corpus(700);
    for layout in LAYOUTS {
        let state = build(layout, &docs, &[]).await;
        assert_paths_agree(&state, layout, true).await;
    }
}

#[tokio::test]
async fn fields_that_do_not_rebuild_from_terms_keep_the_source_path() {
    let docs = corpus(120);
    // One value per field kind the index terms cannot rebuild: a number on
    // a keyword, an array, an empty string (no term), a float on a short.
    let odd = vec![
        (
            "odd1".to_owned(),
            json!({"UID": "o1", "SEXE": 1, "NOM": "X"}),
        ),
        (
            "odd2".to_owned(),
            json!({"UID": "o2", "DEPARTEMENT_DECES": ["75", "2A"], "NOM": "Y"}),
        ),
        (
            "odd3".to_owned(),
            json!({"UID": "o3", "SOURCE": "", "AGE_DECES": 8.5}),
        ),
    ];
    for layout in [Layout::SingleRamWithDeletes, Layout::DiskMultiSegment] {
        let state = build(layout, &docs, &odd).await;
        for aggs in [
            json!({"sex": {"terms": {"field": "SEXE"}}}),
            json!({"d": {"terms": {"field": "DEPARTEMENT_DECES"}}}),
            json!({"s": {"cardinality": {"field": "SOURCE"}}}),
            json!({"a": {"terms": {"field": "AGE_DECES"}}}),
            composite(&[("sex", "SEXE"), ("lastName", "NOM.raw")], None),
        ] {
            let body = agg_body(no_criteria(), 0.0, aggs);
            let (reference, fast, served) = both_paths(&state, &body);
            assert_eq!(fast, reference, "{layout:?}: {body}");
            assert!(!served, "{layout:?}: doc values served {body}");
        }
        // The clean fields of the same index are still served.
        let body = agg_body(
            no_criteria(),
            0.0,
            json!({"n": {"terms": {"field": "NOM.raw"}}, "d": histogram("DATE_DECES_NORM")}),
        );
        let (reference, fast, served) = both_paths(&state, &body);
        assert_eq!(fast, reference, "{layout:?}: {body}");
        assert!(served, "{layout:?}: {body}");
    }
}

#[tokio::test]
async fn small_matched_sets_and_other_shapes_keep_the_general_path() {
    let state = build(Layout::SingleRamWithDeletes, &corpus(200), &[]).await;
    let indices = vec![INDEX.to_owned()];
    for body in [
        // Below the doc-values threshold (no override): general path.
        agg_body(
            no_criteria(),
            0.0,
            json!({"s": {"terms": {"field": "SEXE"}}}),
        ),
        // Hits requested, a superset candidate set, a full-scan clause.
        json!({"size": 1, "aggs": {"s": {"terms": {"field": "SEXE"}}}}),
        json!({"size": 0, "query": {"bool": {"must_not": [{"match": {"SEXE": "M"}}]}},
               "aggs": {"s": {"terms": {"field": "SEXE"}}}}),
        json!({"size": 0, "query": {"wildcard": {"NOM": "dup*"}},
               "aggs": {"s": {"terms": {"field": "SEXE"}}}}),
        // A text field is not a doc-values field.
        json!({"size": 0, "aggs": {"s": {"terms": {"field": "NOM"}}}}),
    ] {
        let request = parse_search_request(&body.to_string()).expect("valid");
        take_doc_values_aggs_served_for_test();
        run_search(&state, &indices, &request);
        assert_eq!(take_doc_values_aggs_served_for_test(), 0, "{body}");
    }
}

#[tokio::test]
async fn value_kinds_survive_a_restart_from_the_data_directory() {
    let root = std::env::temp_dir().join(format!(
        "l1c-kinds-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let docs = corpus(150);
    let body = agg_body(
        no_criteria(),
        0.0,
        composite(&[("sex", "SEXE"), ("deathAge", "AGE_DECES")], None),
    );
    let before = {
        let state = AppState::open_with_data_dir(&root).expect("data dir");
        let router = app_router_with_state(AppRouterState {
            app: state.clone(),
            ..Default::default()
        });
        send(
            &router,
            Method::PUT,
            &format!("/{INDEX}"),
            DECES_INDEX_REAL.to_owned(),
        )
        .await;
        let mut ndjson = String::new();
        for (id, source) in &docs {
            ndjson.push_str(&format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{source}\n"));
        }
        send(&router, Method::POST, &format!("/{INDEX}/_bulk"), ndjson).await;
        send(
            &router,
            Method::POST,
            &format!("/{INDEX}/_refresh"),
            String::new(),
        )
        .await;
        let (reference, fast, served) = both_paths(&state, &body);
        assert_eq!(fast, reference);
        assert!(served);
        fast
    };
    let state = AppState::open_with_data_dir(&root).expect("data dir reopens");
    let (reference, fast, served) = both_paths(&state, &body);
    assert_eq!(fast, reference);
    assert_eq!(fast, before);
    assert!(served, "the reopened index lost its value kinds");
    drop(state);
    let _ = std::fs::remove_dir_all(&root);
}

/// Per candidate `(id, f32 score bits)` of `query`, scored like the
/// generic exact path, with or without the merged fuzzy pass.
fn candidate_scores(state: &AppState, query: &super::SearchQuery, batch: bool) -> Vec<(u32, u32)> {
    let ctx = CandidateCtx::new(state, INDEX);
    let candidates = posting_candidates(&ctx, query, None).expect("postings-backed");
    let ids: Vec<u32> = candidates.ids.into_iter().collect();
    set_fuzzy_batch_disabled_for_test(!batch);
    let scores = state.with_search_reader(INDEX, |reader| {
        let reader = reader.expect("index");
        let mut arena = ScoringArena::default();
        let mut scoring = SearchScoringContext::new(&reader, query, &mut arena);
        scoring.prepare_batch(query, &ids);
        assert_eq!(
            scoring.fuzzy_batch.is_empty(),
            !batch || ids.len() < super::FUZZY_BATCH_MIN_IDS,
            "batch preparation"
        );
        ids.iter()
            .enumerate()
            .map(|(slot, &id)| {
                scoring.set_batch_slot(slot);
                let score = score_for_query(query, id, &scoring, None) as f32;
                (id, score.to_bits())
            })
            .collect()
    });
    set_fuzzy_batch_disabled_for_test(false);
    scores
}

fn fuzzy_shapes() -> Vec<Value> {
    vec![
        json!({"match": {"PRENOMS": {"query": "jean pierre", "fuzziness": "auto"}}}),
        json!({"match": {"PRENOMS": {"query": "jean jean pierre", "fuzziness": "auto",
                                     "operator": "and"}}}),
        json!({"match": {"NOM": {"query": "dupont", "fuzziness": 2, "boost": 3}}}),
        json!({"fuzzy": {"NOM": {"value": "martin", "fuzziness": 1}}}),
        json!({"match": {"NOM.raw": {"query": "dupont", "fuzziness": "auto"}}}),
        json!({"match": {"SEXE": {"query": "m", "fuzziness": 1}}}),
        name_query("Jean", "Dupont"),
        json!({"function_score": {"boost": 1.5, "query": {"bool": {"boost": 0.7, "should": [
            {"match": {"PRENOMS_NOM": {"query": "jean dupont", "fuzziness": "auto"}}},
            {"match": {"PRENOM_NOM": {"query": "pierre", "fuzziness": "auto", "boost": 2}}}
        ], "minimum_should_match": 1}}}}),
    ]
}

#[tokio::test]
async fn merged_fuzzy_pass_scores_bit_identical_to_the_per_document_scorer() {
    let docs = corpus(700);
    for layout in LAYOUTS {
        let state = build(layout, &docs, &[]).await;
        for shape in fuzzy_shapes() {
            let query = super::parse_search_query(&shape).expect("valid query");
            let reference = candidate_scores(&state, &query, false);
            let merged = candidate_scores(&state, &query, true);
            assert_eq!(merged, reference, "{layout:?}: {shape}");
        }
    }
}

#[tokio::test]
async fn merged_fuzzy_pass_keeps_search_responses_identical() {
    let docs = corpus(700);
    let indices = vec![INDEX.to_owned()];
    for layout in [Layout::SingleRamWithDeletes, Layout::DiskMultiSegment] {
        let state = build(layout, &docs, &[]).await;
        for shape in fuzzy_shapes() {
            for (min_score, size) in [(0.0, 700), (1.0, 20)] {
                let body = json!({"query": shape, "min_score": min_score,
                                  "track_total_hits": true, "size": size,
                                  "_source": ["UID"]});
                let request = parse_search_request(&body.to_string()).expect("valid");
                set_fuzzy_batch_disabled_for_test(true);
                let reference = serialized(run_search(&state, &indices, &request));
                set_fuzzy_batch_disabled_for_test(false);
                let merged = serialized(run_search(&state, &indices, &request));
                assert_eq!(merged, reference, "{layout:?}: {body}");
            }
        }
    }
}

/// A document whose analysis yields a blank term (`"--"` through the deces
/// `norm` normalizer gives `"  "`) used to stop the batch merge
/// (`PostingsError::EmptyTerm`), leaving it and the following documents out
/// of the index. Whitespace-only terms are now indexed like ES does and
/// empty terms are skipped, so the index covers every live document and the
/// doc-values path serves the same answer as the `_source` path.
#[tokio::test]
async fn a_blank_term_document_is_indexed_with_the_rest_of_its_batch() {
    let docs = corpus(120);
    let blank = vec![("blank".to_owned(), json!({"UID": "b1", "SOURCE": "--"}))];
    let mut after = blank.clone();
    after.extend(
        corpus(130)
            .into_iter()
            .skip(120)
            .map(|(id, source)| (format!("{id}x"), source)),
    );
    let state = build(Layout::SingleRamWithDeletes, &docs, &after).await;
    let covers = state.with_search_reader(INDEX, |reader| {
        reader.expect("index").index_covers_live_documents()
    });
    assert!(
        covers,
        "a blank-term document must not stop the batch merge"
    );
    for aggs in [
        json!({"sex": {"terms": {"field": "SEXE"}}}),
        json!({"n": {"terms": {"field": "NOM.raw"}}}),
    ] {
        let body = agg_body(no_criteria(), 0.0, aggs);
        let (reference, fast, served) = both_paths(&state, &body);
        assert_eq!(fast, reference, "{body}");
        assert!(served, "doc values declined {body}");
    }
}
