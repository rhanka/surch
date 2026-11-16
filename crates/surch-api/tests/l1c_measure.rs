//! MVP L1c measurement (run explicitly, release):
//! `L1C_DOCS=200000 cargo test --release -p surch-api --test l1c_measure -- --ignored --nocapture`
//!
//! Wall time (median of 5 after one warm-up, `request_cache=false`) and RSS
//! of the deces-backend `/agg` shapes (terms, date_histogram, composite +
//! cardinality, composite `after` paging, with and without criteria) and of
//! the high-df fuzzy OR shapes (`match PRENOMS "jean pierre"`, simple `q`
//! with one and two names), on a synthetic deces-like index with the real
//! `deces_index.yml` field types. Public HTTP API only, so the same file
//! runs on the pre-L1c tree.
//!
//! The corpus is generated chunk by chunk (never held whole), so the
//! process RSS is the index plus one bulk chunk. Run it under a cgroup cap
//! to mimic the node budget, e.g.
//! `systemd-run --user --scope -p MemoryMax=4608M -- <test binary> --ignored`.
//! `L1C_SHAPES=agg_none_terms,q_two_names` restricts the shapes;
//! `L1C_MAPPING=builtin` swaps the analysis (see [`mapping`]).

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use serde_json::{json, Value};
use surch_api::{app_router_with_state, state::AppState, AppRouterState};
use tower::ServiceExt;

const INDEX: &str = "deces";

/// `L1C_MAPPING=real` (default): the real `deces_index.yml` analysis, whose
/// `norm` analyzer strips every non letter/digit BEFORE tokenizing (so
/// `JEAN PIERRE` is the single term `jeanpierre`). `L1C_MAPPING=builtin`:
/// the `l1b_measure` mapping (no `settings.analysis`, Surch's builtin
/// tokenizing `norm`), where `jean pierre` is two high-df tokens.
fn mapping() -> Value {
    let mut mapping = mapping_real();
    if std::env::var("L1C_MAPPING").as_deref() == Ok("builtin") {
        if let Some(object) = mapping.as_object_mut() {
            object.remove("settings");
        }
    }
    mapping
}

fn mapping_real() -> Value {
    let norm_text_raw = json!({"type": "text", "analyzer": "norm",
        "fields": {"raw": {"type": "keyword", "normalizer": "norm"}}});
    let date_text = json!({"type": "text", "analyzer": "standard",
        "index_prefixes": {"min_chars": 4, "max_chars": 6},
        "fields": {"raw": {"type": "keyword"}}});
    let keyword_norm = json!({"type": "keyword", "normalizer": "norm"});
    json!({
    "settings": {"analysis": {
        "analyzer": {"norm": {"tokenizer": "standard", "char_filter": ["alphanum"],
                              "filter": ["asciifolding", "lowercase"]}},
        "normalizer": {"norm": {"type": "custom", "char_filter": ["alphanum"],
                                "filter": ["asciifolding", "lowercase"]}},
        "char_filter": {"alphanum": {"type": "pattern_replace",
                                     "pattern": "[^\\p{L}\\d]", "replacement": ""}}}},
    "mappings": {"properties": {
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
    ("Ajaccio", "2A", 41.9192, 8.7386),
    ("Bastia", "2B", 42.6973, 9.4509),
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

/// One synthetic deces record (same generator as `l1b_measure`, plus the
/// Corsican `2A`/`2B` departments so the keyword normalizer matters).
fn record(rng: &mut Rng, n: usize) -> (String, Value) {
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
    let mut point = |lat: f64, lon: f64| {
        let p = rng.below(400);
        (
            lat + ((p % 20) as f64 - 10.0) / 100.0,
            lon + ((p / 20) as f64 - 10.0) / 100.0,
        )
    };
    let (pn_lat, pn_lon) = point(latn, lonn);
    let (pd_lat, pd_lon) = point(latd, lond);
    let uid = format!("uid{n:08}");
    let source = json!({
        "UID": uid, "SOURCE": format!("deces-{}-m{:02}", (year + age).min(2025), 1 + n % 12),
        "NOM": nom, "PRENOM": prenom, "PRENOMS": prenoms,
        "PRENOMS_NOM": format!("{prenoms} {nom}"), "PRENOM_NOM": format!("{prenom} {nom}"),
        "SEXE": if n.is_multiple_of(2) { "M" } else { "F" },
        "DATE_NAISSANCE": birth, "DATE_NAISSANCE_NORM": birth,
        "COMMUNE_NAISSANCE": cn, "DEPARTEMENT_NAISSANCE": dn,
        "GEOPOINT_NAISSANCE": {"lat": pn_lat, "lon": pn_lon},
        "DATE_DECES": death, "DATE_DECES_NORM": death, "AGE_DECES": age,
        "COMMUNE_DECES": cd, "DEPARTEMENT_DECES": dd,
        "GEOPOINT_DECES": format!("{pd_lat},{pd_lon}"),
    });
    (uid, source)
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

/// deces-backend `buildSimpleMatch` (`q`) for names only (no date token).
fn simple_q(q: &str) -> Value {
    let names: Vec<&str> = q.split_whitespace().collect();
    let joined = names.join(" ");
    let mut must = vec![json!({"match": {"PRENOMS_NOM": {"query": joined, "fuzziness": "auto"}}})];
    if names.len() == 2 {
        let pair = |nom: &str, prenom: &str| {
            json!({"bool": {"must": [
                {"bool": {
                    "must": [{"match": {"NOM": {"query": nom, "fuzziness": "auto"}}}],
                    "should": [{"match": {"NOM": nom}}]
                }},
                {"bool": {
                    "must": [{"match": {"PRENOMS": {"query": prenom, "fuzziness": "auto"}}}],
                    "should": [
                        {"match": {"PRENOM": {"query": prenom, "fuzziness": "auto"}}},
                        {"match": {"PRENOM": prenom}}
                    ]
                }}
            ]}})
        };
        must.push(json!({"bool": {"minimum_should_match": 1, "should": [
            pair(names[0], names[1]),
            pair(names[1], names[0])
        ]}}));
    }
    json!({"bool": {
        "must": must,
        "should": [
            {"match": {"PRENOM_NOM": joined}},
            {"match": {"PRENOM_NOM": {"query": joined, "fuzziness": "auto"}}}
        ]
    }})
}

fn advanced(fields: Vec<Value>) -> Value {
    json!({"function_score": {"query": {"bool": {"must": fields}}}})
}

/// `buildRequest` envelope.
fn body(matched: Value, min_score: u32, size: u32, aggs: Option<Value>) -> Value {
    let mut body = json!({
        "min_score": min_score, "track_total_hits": true,
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

/// deces-backend `buildAggregation` composite over `sources` (`size` 1000)
/// plus one `cardinality` per source on the first page.
fn composite(sources: &[(&str, &str)], after: Option<Value>) -> Value {
    let list: Vec<Value> = sources
        .iter()
        .map(|(name, field)| {
            if field.ends_with("_NORM") {
                json!({*name: {"date_histogram": {"field": field, "calendar_interval": "month",
                                                  "format": "yyyyMMdd"}}})
            } else {
                json!({*name: {"terms": {"field": field}}})
            }
        })
        .collect();
    let mut aggs = json!({"bucketResults": {"composite": {"size": 1000, "sources": list}}});
    match after {
        Some(after) => aggs["bucketResults"]["composite"]["after"] = after,
        None => {
            for (name, field) in sources {
                aggs[format!("{name}_count")] = json!({"cardinality": {"field": field}});
            }
        }
    }
    aggs
}

fn vm_kib(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with(key))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|kib| kib.parse().ok())
        })
        .unwrap_or(0)
}

/// Reset the peak RSS (`VmHWM`) to the current RSS (Linux `clear_refs` 5).
fn reset_peak() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

fn mib(kib: u64) -> f64 {
    kib as f64 / 1024.0
}

#[tokio::test]
#[ignore = "measurement: L1C_DOCS=200000 cargo test --release -p surch-api --test l1c_measure -- --ignored --nocapture"]
async fn measure_agg_and_fuzzy_shapes() {
    let total: usize = std::env::var("L1C_DOCS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200_000);
    let only: Option<Vec<String>> = std::env::var("L1C_SHAPES")
        .ok()
        .map(|list| list.split(',').map(str::to_owned).collect());
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
    let mut rng = Rng(0x5eed_1b0b_2026_1115);
    let chunk = 20_000;
    let mut n = 0;
    while n < total {
        let end = (n + chunk).min(total);
        let mut ndjson = String::new();
        for i in n..end {
            let (id, source) = record(&mut rng, i);
            ndjson.push_str(&format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{source}\n"));
        }
        let bulk = send(&router, Method::POST, &format!("/{INDEX}/_bulk"), ndjson).await;
        assert_eq!(bulk["errors"], false);
        n = end;
    }
    send(
        &router,
        Method::POST,
        &format!("/{INDEX}/_refresh"),
        String::new(),
    )
    .await;
    println!(
        "indexed {total} docs in {:?} | RSS {:.0} MiB | peak {:.0} MiB",
        started.elapsed(),
        mib(vm_kib("VmRSS:")),
        mib(vm_kib("VmHWM:"))
    );

    let uri = format!("/{INDEX}/_search?request_cache=false");
    let sex_dept = [("sex", "SEXE"), ("deathDepartment", "DEPARTEMENT_DECES")];
    let first_page = send(
        &router,
        Method::POST,
        &uri,
        body(advanced(vec![]), 0, 0, Some(composite(&sex_dept, None))).to_string(),
    )
    .await;
    let after = first_page["aggregations"]["bucketResults"]["after_key"].clone();
    let city_date = [
        ("birthCity", "COMMUNE_NAISSANCE.raw"),
        ("deathDate", "DATE_DECES_NORM"),
    ];
    let city_first = send(
        &router,
        Method::POST,
        &uri,
        body(advanced(vec![]), 0, 0, Some(composite(&city_date, None))).to_string(),
    )
    .await;
    let city_after = city_first["aggregations"]["bucketResults"]["after_key"].clone();

    let shapes: Vec<(&str, Value)> = vec![
        (
            "fuzzy_prenoms_jean_pierre",
            json!({"query": {"match": {"PRENOMS": {"query": "jean pierre", "fuzziness": "auto"}}},
                   "min_score": 5, "track_total_hits": true, "size": 20}),
        ),
        (
            "adv_prenoms_jean_pierre",
            body(
                advanced(vec![json!({"match": {"PRENOMS": {"query": "jean pierre",
                                                            "fuzziness": "auto"}}})]),
                0,
                20,
                None,
            ),
        ),
        ("q_jean_pierre", body(simple_q("jean pierre"), 5, 20, None)),
        ("q_two_names", body(simple_q("jean dupont"), 5, 20, None)),
        (
            "agg_none_terms",
            body(
                advanced(vec![]),
                0,
                0,
                Some(
                    json!({"deathDepartment": {"terms": {"field": "DEPARTEMENT_DECES", "size": 100}}}),
                ),
            ),
        ),
        (
            "agg_none_terms_raw",
            body(
                advanced(vec![]),
                0,
                0,
                Some(json!({"lastName": {"terms": {"field": "NOM.raw", "size": 100}}})),
            ),
        ),
        (
            "agg_none_histogram",
            body(
                advanced(vec![]),
                0,
                0,
                Some(json!({"birthDate": {"date_histogram": {
                    "field": "DATE_NAISSANCE_NORM", "calendar_interval": "month",
                    "format": "yyyyMMdd"}}})),
            ),
        ),
        (
            "agg_none_composite",
            body(advanced(vec![]), 0, 0, Some(composite(&sex_dept, None))),
        ),
        (
            "agg_none_composite_page2",
            body(
                advanced(vec![]),
                0,
                0,
                Some(composite(&sex_dept, Some(after))),
            ),
        ),
        (
            "agg_none_composite_city_date_page2",
            body(
                advanced(vec![]),
                0,
                0,
                Some(composite(&city_date, Some(city_after))),
            ),
        ),
        (
            "agg_name_composite",
            body(
                advanced(vec![name_query("Jean", "Dupont")]),
                0,
                0,
                Some(composite(&sex_dept, None)),
            ),
        ),
        (
            "agg_adv_prenoms_composite",
            body(
                advanced(vec![json!({"match": {"PRENOMS": {"query": "jean pierre",
                                                            "fuzziness": "auto"}}})]),
                0,
                0,
                Some(composite(&sex_dept, None)),
            ),
        ),
        (
            "agg_q_jean_pierre_terms",
            body(
                simple_q("jean pierre"),
                5,
                0,
                Some(json!({"sex": {"terms": {"field": "SEXE", "size": 10}}})),
            ),
        ),
    ];

    for (label, request) in shapes {
        if only
            .as_ref()
            .is_some_and(|list| !list.iter().any(|name| name == label))
        {
            continue;
        }
        reset_peak();
        let rss_before = vm_kib("VmRSS:");
        let warm = send(&router, Method::POST, &uri, request.to_string()).await;
        assert!(warm["hits"].is_object(), "{label}: {warm}");
        let mut timings = Vec::new();
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let response = send(&router, Method::POST, &uri, request.to_string()).await;
            timings.push(t.elapsed());
            assert_eq!(response["hits"]["total"], warm["hits"]["total"], "{label}");
        }
        timings.sort();
        let digest = {
            use std::hash::{Hash, Hasher};
            let mut masked = warm.clone();
            masked["took"] = json!(0);
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            masked.to_string().hash(&mut hasher);
            hasher.finish()
        };
        println!(
            "{label}: total {} | median {:?} | min {:?} | max {:?} | RSS {:.0} -> peak {:.0} MiB | digest {digest:016x}",
            warm["hits"]["total"]["value"],
            timings[2],
            timings[0],
            timings[4],
            mib(rss_before),
            mib(vm_kib("VmHWM:")),
        );
    }
}
