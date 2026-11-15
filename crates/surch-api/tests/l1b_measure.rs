//! MVP L1b measurement (run explicitly, release):
//! `cargo test --release -p surch-api --test l1b_measure -- --ignored --nocapture`
//!
//! Wall time (median of 5 after one warm-up, `request_cache=false`) of the
//! deces-backend shapes that took the `_source` full scan before L1b (root
//! `function_score` with aggregations, `range`, `geo_distance`,
//! `match_all`-equivalent wrappers) and of a few reference shapes, on a
//! synthetic 200 k-doc index with the real `deces_index.yml` field types.
//! Public HTTP API only, so the same file runs on the pre-L1b tree.

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use serde_json::{json, Value};
use surch_api::{app_router_with_state, state::AppState, AppRouterState};
use tower::ServiceExt;

const INDEX: &str = "deces";

fn mapping() -> Value {
    let norm_text_raw = json!({"type": "text", "analyzer": "norm",
        "fields": {"raw": {"type": "keyword", "normalizer": "norm"}}});
    let date_text = json!({"type": "text", "analyzer": "standard",
        "index_prefixes": {"min_chars": 4, "max_chars": 6},
        "fields": {"raw": {"type": "keyword"}}});
    let keyword_norm = json!({"type": "keyword", "normalizer": "norm"});
    json!({"mappings": {"properties": {
        "UID": {"type": "keyword"},
        "SOURCE": keyword_norm.clone(),
        "PRENOMS_NOM": {"type": "text", "analyzer": "norm"},
        "PRENOM_NOM": {"type": "text", "analyzer": "norm"},
        "NOM": norm_text_raw.clone(),
        "PRENOM": norm_text_raw.clone(),
        "PRENOMS": norm_text_raw.clone(),
        "SEXE": {"type": "keyword"},
        "DATE_NAISSANCE": date_text.clone(),
        "DATE_NAISSANCE_NORM": {"type": "date", "format": "yyyyMMdd"},
        "COMMUNE_NAISSANCE": norm_text_raw.clone(),
        "DEPARTEMENT_NAISSANCE": keyword_norm.clone(),
        "GEOPOINT_NAISSANCE": {"type": "geo_point"},
        "DATE_DECES": date_text,
        "DATE_DECES_NORM": {"type": "date", "format": "yyyyMMdd"},
        "AGE_DECES": {"type": "short"},
        "COMMUNE_DECES": norm_text_raw,
        "DEPARTEMENT_DECES": keyword_norm,
        "GEOPOINT_DECES": {"type": "geo_point"}
    }}})
}

const NOMS: &[&str] = &[
    "DUPONT", "DUPOND", "DURAND", "MARTIN", "MATRIN", "BERNARD", "PETIT", "LEROY", "MOREAU",
    "DUBOIS", "LAURENT", "SIMON", "MICHEL", "GARCIA", "DAVID", "BERTRAND", "ROUX", "VINCENT",
    "FOURNIER", "MOREL", "GIRARD", "ANDRE", "MERCIER", "BLANC", "GUERIN", "BOYER", "HENRY",
];
const PRENOMS: &[&str] = &[
    "JEAN PIERRE",
    "JEAN",
    "MARIE LOUISE",
    "MARIE",
    "PIERRE",
    "LOUIS",
    "ANNE",
    "PAUL",
    "JACQUES",
    "MICHEL",
    "ANDRE",
    "RENE",
    "HELENE",
    "FRANCOIS",
];
const COMMUNES: &[(&str, &str, f64, f64)] = &[
    ("Paris", "75", 48.8566, 2.3522),
    ("Lyon", "69", 45.7640, 4.8357),
    ("Marseille", "13", 43.2965, 5.3698),
    ("Toulouse", "31", 43.6047, 1.4442),
    ("Nice", "06", 43.7102, 7.2620),
    ("Nantes", "44", 47.2184, -1.5536),
    ("Strasbourg", "67", 48.5734, 7.7521),
    ("Lille", "59", 50.6292, 3.0573),
];

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

fn corpus(total: usize) -> Vec<(String, Value)> {
    let mut rng = Rng(0x5eed_1b0b_2026_1115);
    (0..total)
        .map(|n| {
            let nom = NOMS[rng.below(NOMS.len())];
            let prenoms = PRENOMS[rng.below(PRENOMS.len())];
            let prenom = prenoms.split(' ').next().unwrap_or(prenoms);
            let year = 1900 + rng.below(91);
            let birth = format!("{year}{:02}{:02}", 1 + rng.below(12), 1 + rng.below(28));
            let age = 20 + rng.below(80);
            let death = format!(
                "{}{:02}{:02}",
                (year + age).min(2025),
                1 + rng.below(12),
                1 + rng.below(28)
            );
            let (cn, dn, latn, lonn) = COMMUNES[rng.below(COMMUNES.len())];
            let (cd, dd, latd, lond) = COMMUNES[rng.below(COMMUNES.len())];
            // 400 distinct points (a 20 x 20 grid, 0.01 deg apart) around
            // each city: 3 200 distinct points in all, like the ~36 k INSEE
            // commune centroids for 29 M records (few points, many docs each).
            let mut point = |lat: f64, lon: f64| {
                let p = rng.below(400);
                (
                    lat + ((p % 20) as f64 - 10.0) / 100.0,
                    lon + ((p / 20) as f64 - 10.0) / 100.0,
                )
            };
            let (pn_lat, pn_lon) = point(latn, lonn);
            let (pd_lat, pd_lon) = point(latd, lond);
            let geo_n = json!({"lat": pn_lat, "lon": pn_lon});
            let geo_d = format!("{pd_lat},{pd_lon}");
            let uid = format!("uid{n:07}");
            let source = json!({
                "UID": uid, "SOURCE": format!("deces-{}-m{:02}", (year + age).min(2025), 1 + n % 12),
                "NOM": nom, "PRENOM": prenom, "PRENOMS": prenoms,
                "PRENOMS_NOM": format!("{prenoms} {nom}"), "PRENOM_NOM": format!("{prenom} {nom}"),
                "SEXE": if n % 2 == 0 { "M" } else { "F" },
                "DATE_NAISSANCE": birth, "DATE_NAISSANCE_NORM": birth,
                "COMMUNE_NAISSANCE": cn, "DEPARTEMENT_NAISSANCE": dn, "GEOPOINT_NAISSANCE": geo_n,
                "DATE_DECES": death, "DATE_DECES_NORM": death, "AGE_DECES": age,
                "COMMUNE_DECES": cd, "DEPARTEMENT_DECES": dd, "GEOPOINT_DECES": geo_d,
            });
            (uid, source)
        })
        .collect()
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
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body readable");
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn fuzzy_should(field: &str, value: &str) -> Value {
    json!({"bool": {"must": [{"match": {field: {"query": value, "fuzziness": "auto"}}}],
                    "should": [{"match": {format!("{field}.raw"): value}}]}})
}

/// deces-backend `nameQuery` (fuzzy) for first + last name, `must = true`.
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

fn geo(field: &str, lat: f64, lon: f64, distance: &str) -> Value {
    json!({"bool": {"must": {"match_all": {}},
        "filter": {"geo_distance": {"distance": distance, field: {"lat": lat, "lon": lon}}}}})
}

fn advanced(fields: Vec<Value>) -> Value {
    json!({"function_score": {"query": {"bool": {"must": fields}}}})
}

/// `buildRequest` envelope.
fn body(
    matched: Value,
    min_score: u32,
    track_total_hits: bool,
    size: u32,
    aggs: Option<Value>,
) -> Value {
    let mut body = json!({
        "min_score": min_score, "track_total_hits": track_total_hits,
        "_source": ["UID", "NOM", "PRENOM", "DATE_NAISSANCE", "DATE_DECES"],
        "query": {"bool": {"must": [matched]}},
        "sort": [{"_score": "desc"}], "size": size,
    });
    if size > 0 {
        body["from"] = json!(0);
    }
    if let Some(aggs) = aggs {
        body["aggs"] = aggs;
    }
    body
}

fn shapes() -> Vec<(&'static str, Value)> {
    let birth_range = json!({"range": {"DATE_NAISSANCE": {"gte": "19400000", "lte": "19459999"}}});
    let death_after = json!({"range": {"DATE_DECES": {"gte": "20000000"}}});
    let age_range = json!({"range": {"AGE_DECES": {"gte": 60, "lte": 70}}});
    let paris = geo("GEOPOINT_NAISSANCE", 48.8566, 2.3522, "5km");
    let terms_sex = json!({"sex": {"terms": {"field": "SEXE", "size": 250}}});
    let histogram = json!({"birthDate": {"date_histogram": {
        "field": "DATE_NAISSANCE_NORM", "calendar_interval": "month", "format": "yyyyMMdd"}}});
    vec![
        (
            "advanced name (fuzzy), size 20",
            body(
                advanced(vec![name_query("Jean", "Dupont")]),
                0,
                true,
                20,
                None,
            ),
        ),
        (
            "/agg terms sex, advanced name (root function_score + aggs)",
            body(
                advanced(vec![name_query("Jean", "Dupont")]),
                0,
                true,
                0,
                Some(terms_sex),
            ),
        ),
        (
            "advanced name + birth range",
            body(
                advanced(vec![name_query("Jean", "Dupont"), birth_range.clone()]),
                0,
                true,
                20,
                None,
            ),
        ),
        (
            "advanced birth range alone",
            body(advanced(vec![birth_range.clone()]), 0, true, 20, None),
        ),
        (
            "advanced death >2000 (lastSeenAliveDate) + sex",
            body(
                advanced(vec![death_after, json!({"match": {"SEXE": "F"}})]),
                0,
                true,
                20,
                None,
            ),
        ),
        (
            "advanced age range alone",
            body(advanced(vec![age_range]), 0, true, 20, None),
        ),
        (
            "advanced birth geo 5km alone",
            body(advanced(vec![paris.clone()]), 0, true, 20, None),
        ),
        (
            "advanced name + birth geo 5km",
            body(
                advanced(vec![name_query("Jean", "Dupont"), paris]),
                0,
                true,
                20,
                None,
            ),
        ),
        (
            "link block name + birth range (track_total_hits false)",
            body(
                json!({"function_score": {"query": {"bool": {
                    "must": [
                        {"match": {"PRENOMS_NOM": {"query": "Dupont Jean", "fuzziness": "auto"}}},
                        birth_range
                    ],
                    "should": [{"match": {"NOM": "Dupont"}}],
                    "minimum_should_match": 1}}}}),
                0,
                false,
                20,
                None,
            ),
        ),
        (
            "link block name + full birth date (fuzzy 1 / prefix 4)",
            body(
                json!({"function_score": {"query": {"bool": {
                    "must": [
                        {"match": {"PRENOMS_NOM": {"query": "Dupont Jean", "fuzziness": "auto"}}},
                        {"bool": {"should": [
                            {"match": {"DATE_NAISSANCE": {"query": "19420315", "fuzziness": 1}}},
                            {"prefix": {"DATE_NAISSANCE": "1942"}}
                        ], "minimum_should_match": 1}}
                    ],
                    "should": [{"match": {"NOM": "Dupont"}}, {"match": {"DATE_NAISSANCE": "19420315"}}],
                    "minimum_should_match": 1}}}}),
                0,
                false,
                20,
                None,
            ),
        ),
        (
            "/agg date_histogram, no criteria (function_score over empty bool)",
            body(advanced(vec![]), 0, true, 0, Some(histogram)),
        ),
        (
            "simple q stop words only (match_all, min_score 5)",
            body(json!({"match_all": {}}), 5, true, 20, None),
        ),
        (
            "simple q 'jean pierre' high-df fuzzy OR (reference)",
            body(
                json!({"bool": {"must": [
                    {"match": {"PRENOMS_NOM": {"query": "jean pierre", "fuzziness": "auto"}}}],
                    "should": [{"match": {"PRENOM_NOM": "jean pierre"}},
                               {"match": {"PRENOM_NOM": {"query": "jean pierre", "fuzziness": "auto"}}}]}}),
                5,
                true,
                20,
                None,
            ),
        ),
        (
            "reference full scan: wildcard",
            json!({"query": {"wildcard": {"NOM": "dup*"}}, "size": 20, "track_total_hits": true}),
        ),
    ]
}

#[tokio::test]
#[ignore = "measurement: cargo test --release -p surch-api --test l1b_measure -- --ignored --nocapture"]
async fn measure_build_request_shapes_on_200k_docs() {
    let total = 200_000;
    let docs = corpus(total);
    let state = AppState::default();
    let router = app_router_with_state(AppRouterState {
        app: state.clone(),
        ..Default::default()
    });
    let created = send(
        &router,
        Method::PUT,
        &format!("/{INDEX}"),
        mapping().to_string(),
    )
    .await;
    assert_eq!(created["acknowledged"], true, "{created}");
    let started = std::time::Instant::now();
    for chunk in docs.chunks(20_000) {
        let mut ndjson = String::new();
        for (id, source) in chunk {
            ndjson.push_str(&format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{source}\n"));
        }
        let bulk = send(&router, Method::POST, &format!("/{INDEX}/_bulk"), ndjson).await;
        assert_eq!(bulk["errors"], false);
    }
    send(
        &router,
        Method::POST,
        &format!("/{INDEX}/_refresh"),
        String::new(),
    )
    .await;
    println!("indexed {total} docs in {:?}", started.elapsed());
    for (label, request) in shapes() {
        let uri = format!("/{INDEX}/_search?request_cache=false");
        let warm = send(&router, Method::POST, &uri, request.to_string()).await;
        assert!(warm["hits"].is_object(), "{label}: {warm}");
        let mut timings = Vec::new();
        let mut hits = Value::Null;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let response = send(&router, Method::POST, &uri, request.to_string()).await;
            timings.push(t.elapsed());
            hits = response["hits"]["total"].clone();
        }
        timings.sort();
        println!(
            "{label}: total {} | median {:?} | min {:?} | max {:?}",
            hits, timings[2], timings[0], timings[4]
        );
    }
}
