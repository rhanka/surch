//! MVP L1b guard: no query shape deces-backend's `buildRequest.ts` can emit
//! reaches the `_source` full scan (`full_scan_documents`, counted per
//! thread), and every shape returns exactly the brute-force match set.
//!
//! The bodies are built by a Rust port of `buildRequest.ts` / `queries.ts`
//! (matchID `packages/deces-backend/src`, functions named alike) fed with
//! synthetic inputs: simple `q` search, `id` lookup (`_msearch`), advanced
//! search (fuzzy and exact), the adaptive link block and the advanced block
//! (link `_msearch` chunks, `track_total_hits: false`), and the `/agg`
//! shapes (terms, date_histogram, composite + cardinality, `after`). The
//! index is created through `PUT /deces` with the real `deces_index.yml`
//! definition (`tests/matchid_compat/deces/mapping_real.json`, lot L2):
//! custom `norm` analyzer and normalizer with the `alphanum`
//! `pattern_replace` char filter, `DATE_*` text fields on the default
//! `standard` analyzer with `index_prefixes{4,6}`.
//!
//! Each shape runs on three layouts (single RAM segment, multi-segment,
//! disk-backed postings), through `_search` and `_msearch`, and its match
//! set is compared with `query_matches` over every stored document.

use std::collections::BTreeSet;

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    parse_search_request, query_matches, run_search, set_term_scan_source_check_max_for_test,
    take_full_scan_fallbacks_for_test,
};
use crate::{app_router_with_state, state::AppState, AppRouterState};

// ---------------------------------------------------------------------------
// Fixture: real deces mapping + synthetic corpus.
// ---------------------------------------------------------------------------

/// matchID `deces_index.yml` as ES receives it (provenance in
/// `tests/matchid_compat/deces/README.md`).
const DECES_INDEX_REAL: &str =
    include_str!("../../../tests/matchid_compat/deces/mapping_real.json");

const NOMS: &[&str] = &[
    "DUPONT", "DUPOND", "DURAND", "MARTIN", "MATRIN", "BERNARD", "PETIT", "LEROY", "MOREAU",
];
const PRENOMS: &[&str] = &[
    "JEAN PIERRE",
    "JEAN",
    "MARIE LOUISE",
    "MARIE",
    "PIERRE",
    "LOUIS",
    "ANNE",
];
const COMMUNES: &[(&str, &str, &str, f64, f64)] = &[
    ("Paris", "75056", "75", 48.8566, 2.3522),
    ("Lyon", "69123", "69", 45.7640, 4.8357),
    ("Marseille", "13055", "13", 43.2965, 5.3698),
    ("Saint-Etienne", "42218", "42", 45.4397, 4.3872),
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
            let birth_year = 1900 + rng.below(91);
            let birth = format!(
                "{birth_year}{:02}{:02}",
                1 + rng.below(12),
                1 + rng.below(28)
            );
            let age = 20 + rng.below(80);
            let death = format!(
                "{}{:02}{:02}",
                birth_year + age,
                1 + rng.below(12),
                1 + rng.below(28)
            );
            let (commune_n, insee_n, dep_n, lat_n, lon_n) = COMMUNES[rng.below(COMMUNES.len())];
            let (commune_d, insee_d, dep_d, lat_d, lon_d) = COMMUNES[rng.below(COMMUNES.len())];
            let jitter = |rng: &mut Rng| (rng.below(2000) as f64 - 1000.0) / 100_000.0;
            let uid = format!("uid{n:05}");
            let source = json!({
                "UID": uid,
                "SOURCE": format!("deces-{}-m{:02}", birth_year + age, 1 + n % 12),
                "SOURCE_LINE": n,
                "NOM": nom,
                "PRENOM": prenom,
                "PRENOMS": prenoms,
                "PRENOMS_NOM": format!("{prenoms} {nom}"),
                "PRENOM_NOM": format!("{prenom} {nom}"),
                "SEXE": if n % 2 == 0 { "M" } else { "F" },
                "DATE_NAISSANCE": birth,
                "DATE_NAISSANCE_NORM": birth,
                "COMMUNE_NAISSANCE": commune_n,
                "CODE_INSEE_NAISSANCE": insee_n,
                "CODE_POSTAL_NAISSANCE": format!("{dep_n}000"),
                "CODE_INSEE_NAISSANCE_HISTORIQUE": [insee_n, format!("{dep_n}999")],
                "DEPARTEMENT_NAISSANCE": dep_n,
                "PAYS_NAISSANCE": "France",
                // Both accepted source shapes of a geo_point.
                "GEOPOINT_NAISSANCE": {"lat": lat_n + jitter(&mut rng), "lon": lon_n + jitter(&mut rng)},
                "GEOPOINT_DECES": format!("{},{}", lat_d + jitter(&mut rng), lon_d + jitter(&mut rng)),
                "DATE_DECES": death,
                "DATE_DECES_NORM": death,
                "AGE_DECES": age,
                "COMMUNE_DECES": commune_d,
                "CODE_INSEE_DECES": insee_d,
                "CODE_POSTAL_DECES": format!("{dep_d}000"),
                "CODE_INSEE_DECES_HISTORIQUE": insee_d,
                "DEPARTEMENT_DECES": dep_d,
                "PAYS_DECES": "France",
            });
            (uid, source)
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
enum Layout {
    SingleRam,
    MultiSegment,
    DiskBacked,
}

const INDEX: &str = "deces";

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

async fn build(layout: Layout, docs: &[(String, Value)]) -> (axum::Router, AppState) {
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
    match layout {
        Layout::SingleRam => state.set_flush_budget_bytes_override(INDEX, None),
        Layout::MultiSegment => state.set_flush_budget_bytes_override(INDEX, Some(1)),
        Layout::DiskBacked => {
            state.set_flush_budget_bytes_override(INDEX, None);
            state.set_postings_disk_enabled(INDEX, true);
        }
    }
    for chunk in docs.chunks(docs.len().div_ceil(5).max(1)) {
        let mut ndjson = String::new();
        for (id, source) in chunk {
            ndjson.push_str(&format!("{{\"index\":{{\"_id\":\"{id}\"}}}}\n{source}\n"));
        }
        let body = send(&router, Method::POST, &format!("/{INDEX}/_bulk"), ndjson).await;
        assert_eq!(body["errors"], false, "bulk failed: {body:?}");
    }
    send(
        &router,
        Method::POST,
        &format!("/{INDEX}/_refresh"),
        String::new(),
    )
    .await;
    match layout {
        Layout::SingleRam => assert_eq!(state.index_segment_count(INDEX), 1),
        Layout::MultiSegment => assert!(state.index_segment_count(INDEX) > 1),
        Layout::DiskBacked => assert!(state.index_disk_postings_bytes(INDEX).unwrap_or(0) > 0),
    }
    (router, state)
}

// ---------------------------------------------------------------------------
// Port of `queries.ts` (same names, same shapes).
// ---------------------------------------------------------------------------

fn prefix_query(field: &str, value: &str) -> Value {
    json!({"prefix": {field: value}})
}

fn match_query(field: &str, value: Value) -> Value {
    json!({"match": {field: value}})
}

/// `fuzzy`: `Value::Bool(false)` for the exact form, otherwise the
/// `fuzziness` value (`"auto"` or an edit count).
fn fuzzy_term_query(field: &str, value: &str, fuzzy: Value) -> Value {
    if fuzzy == Value::Bool(false) {
        match_query(field, json!(value))
    } else {
        json!({"match": {field: {"query": value, "fuzziness": fuzzy}}})
    }
}

fn fuzzy_should_term_query(field: &str, value: &str, fuzzy: bool) -> Value {
    if fuzzy {
        json!({"bool": {
            "must": [{"match": {field: {"query": value, "fuzziness": "auto"}}}],
            "should": [{"match": {format!("{field}.raw"): value}}]
        }})
    } else {
        match_query(&format!("{field}.raw"), json!(value))
    }
}

fn first_name_query(fields: [&str; 2], value: &str, fuzzy: bool) -> Value {
    if fuzzy {
        json!({"bool": {
            "must": [{"match": {fields[1]: {"query": value, "fuzziness": "auto"}}}],
            "should": [
                {"match": {fields[0]: {"query": value, "boost": 4}}},
                {"match": {fields[0]: {"query": value, "fuzziness": "auto", "boost": 2}}},
                {"match": {fields[1]: value}}
            ]
        }})
    } else {
        match_query(fields[0], json!(value))
    }
}

#[derive(Clone, Default)]
struct Name {
    first: Option<&'static str>,
    last: Option<&'static str>,
    legal: Option<&'static str>,
}

fn name_query(value: &Name, fuzzy: bool, must: bool) -> Value {
    let both = value.first.is_some() && value.last.is_some();
    let min_should: i64 = (if both { 2 } else { 1 }) - i64::from(!must);
    if fuzzy {
        let mut first_should = Vec::new();
        if let Some(first) = value.first {
            first_should.push(first_name_query(["PRENOM", "PRENOMS"], first, true));
        }
        if let Some(last) = value.last {
            first_should.push(fuzzy_should_term_query("NOM", last, true));
        }
        if let Some(legal) = value.legal {
            first_should.push(fuzzy_should_term_query("NOM", legal, true));
        }
        let mut should = vec![json!({"bool": {
            "should": first_should, "minimum_should_match": min_should, "boost": 2
        }})];
        if let (Some(first), Some(last)) = (value.first, value.last) {
            should.push(json!({"bool": {
                "should": [
                    first_name_query(["PRENOM", "PRENOMS"], last, true),
                    fuzzy_should_term_query("NOM", first, true)
                ],
                "minimum_should_match": min_should,
                "boost": 0.5
            }}));
        }
        json!({"bool": {"minimum_should_match": 1, "should": should}})
    } else {
        let mut should = Vec::new();
        if let Some(first) = value.first {
            should.push(match_query("PRENOM", json!(first)));
        }
        if let Some(last) = value.last {
            should.push(match_query("NOM", json!(last)));
        }
        if let Some(legal) = value.legal {
            should.push(match_query("NOM", json!(legal)));
        }
        json!({"bool": {"minimum_should_match": min_should, "should": should}})
    }
}

/// Transformed date input (`dateRangeTransformMask`): one `YYYYMMDD` /
/// `YYYYMM` / `YYYY` string, or a `[min, max]` pair with an open side.
#[derive(Clone)]
enum DateInput {
    Single(&'static str),
    Range(Option<&'static str>, Option<&'static str>),
}

fn date_range_string_query(field: &str, value: &DateInput, fuzzy: bool) -> Value {
    match value {
        DateInput::Range(None, Some(max)) => {
            json!({"range": {field: {"lte": format!("{max:9<8}")}}})
        }
        DateInput::Range(Some(min), None) => {
            json!({"range": {field: {"gte": format!("{min:0<8}")}}})
        }
        DateInput::Range(Some(a), Some(b)) => {
            let (min, max) = if a <= b { (a, b) } else { (b, a) };
            json!({"range": {field: {"gte": format!("{min:0<8}"), "lte": format!("{max:9<8}")}}})
        }
        DateInput::Range(None, None) => unreachable!("not emitted by the masks"),
        DateInput::Single(text) if text.len() < 8 => prefix_query(field, text),
        DateInput::Single(text) => fuzzy_should_term_query(field, text, fuzzy),
    }
}

#[derive(Clone)]
enum AgeInput {
    Single(u32),
    Range(u32, u32),
}

fn age_range_string_query(field: &str, value: &AgeInput) -> Value {
    match value {
        AgeInput::Range(a, b) => {
            json!({"range": {field: {"gte": (*a).min(*b), "lte": (*a).max(*b)}}})
        }
        AgeInput::Single(age) => match_query(field, json!(age)),
    }
}

fn geo_point_query(field: &str, lat: f64, lon: f64, distance: &str) -> Value {
    json!({"bool": {
        "must": {"match_all": {}},
        "filter": {"geo_distance": {"distance": distance, field: {"lat": lat, "lon": lon}}}
    }})
}

// ---------------------------------------------------------------------------
// Port of `buildRequest.ts`.
// ---------------------------------------------------------------------------

/// The searchable inputs of `RequestInput`, in its key order (the order
/// `Object.keys(searchInput)` walks in `buildAdvancedMatch`).
#[derive(Clone, Default)]
struct Input {
    name: Option<Name>,
    sex: Option<&'static str>,
    birth_date: Option<DateInput>,
    birth_city: Option<&'static str>,
    birth_location_code: Option<&'static str>,
    birth_postal_code: Option<&'static str>,
    birth_department: Option<&'static str>,
    birth_country: Option<&'static str>,
    birth_geo_point: Option<(f64, f64, &'static str)>,
    death_date: Option<DateInput>,
    death_age: Option<AgeInput>,
    death_city: Option<&'static str>,
    death_location_code: Option<&'static str>,
    death_postal_code: Option<&'static str>,
    death_department: Option<&'static str>,
    death_country: Option<&'static str>,
    death_geo_point: Option<(f64, f64, &'static str)>,
    source: Option<&'static str>,
    fuzzy: bool,
}

/// `buildFieldRequest` for every set key, as `(key, query)` pairs.
fn field_requests(input: &Input, must: bool) -> Vec<(&'static str, Value)> {
    let fuzzy = input.fuzzy;
    let mut out = Vec::new();
    if let Some(name) = &input.name {
        out.push(("name", name_query(name, fuzzy, must)));
    }
    if let Some(sex) = input.sex {
        out.push(("sex", match_query("SEXE", json!(sex))));
    }
    if let Some(date) = &input.birth_date {
        out.push((
            "birthDate",
            date_range_string_query("DATE_NAISSANCE", date, fuzzy),
        ));
    }
    if let Some(city) = input.birth_city {
        out.push((
            "birthCity",
            fuzzy_should_term_query("COMMUNE_NAISSANCE", city, fuzzy),
        ));
    }
    if let Some(code) = input.birth_location_code {
        out.push((
            "birthLocationCode",
            match_query("CODE_INSEE_NAISSANCE_HISTORIQUE", json!(code)),
        ));
    }
    if let Some(code) = input.birth_postal_code {
        out.push((
            "birthPostalCode",
            match_query("CODE_POSTAL_NAISSANCE", json!(code)),
        ));
    }
    if let Some(dep) = input.birth_department {
        out.push((
            "birthDepartment",
            match_query("DEPARTEMENT_NAISSANCE", json!(dep)),
        ));
    }
    if let Some(country) = input.birth_country {
        out.push((
            "birthCountry",
            fuzzy_should_term_query("PAYS_NAISSANCE", country, fuzzy),
        ));
    }
    if let Some((lat, lon, distance)) = input.birth_geo_point {
        out.push((
            "birthGeoPoint",
            geo_point_query("GEOPOINT_NAISSANCE", lat, lon, distance),
        ));
    }
    if let Some(date) = &input.death_date {
        out.push((
            "deathDate",
            date_range_string_query("DATE_DECES", date, fuzzy),
        ));
    }
    if let Some(age) = &input.death_age {
        out.push(("deathAge", age_range_string_query("AGE_DECES", age)));
    }
    if let Some(city) = input.death_city {
        out.push((
            "deathCity",
            fuzzy_should_term_query("COMMUNE_DECES", city, fuzzy),
        ));
    }
    if let Some(code) = input.death_location_code {
        out.push((
            "deathLocationCode",
            match_query("CODE_INSEE_DECES_HISTORIQUE", json!(code)),
        ));
    }
    if let Some(code) = input.death_postal_code {
        out.push((
            "deathPostalCode",
            match_query("CODE_POSTAL_DECES", json!(code)),
        ));
    }
    if let Some(dep) = input.death_department {
        out.push((
            "deathDepartment",
            match_query("DEPARTEMENT_DECES", json!(dep)),
        ));
    }
    if let Some(country) = input.death_country {
        out.push((
            "deathCountry",
            fuzzy_should_term_query("PAYS_DECES", country, fuzzy),
        ));
    }
    if let Some((lat, lon, distance)) = input.death_geo_point {
        out.push((
            "deathGeoPoint",
            geo_point_query("GEOPOINT_DECES", lat, lon, distance),
        ));
    }
    if let Some(source) = input.source {
        out.push(("source", match_query("SOURCE", json!(source))));
    }
    out
}

struct Block {
    scope: &'static [&'static str],
    minimum_match: u32,
    should: bool,
}

fn build_advanced_match(input: &Input, block: Option<&Block>) -> Value {
    let must: Vec<Value> = match block {
        None => field_requests(input, true)
            .into_iter()
            .map(|(_, query)| query)
            .collect(),
        Some(block) => {
            let all = field_requests(input, false);
            let scope: Vec<Value> = all
                .iter()
                .filter(|(key, _)| block.scope.contains(key))
                .map(|(_, query)| query.clone())
                .collect();
            let others: Vec<Value> = all
                .iter()
                .filter(|(key, _)| !block.scope.contains(key))
                .map(|(_, query)| query.clone())
                .collect();
            let mut must = vec![json!({"bool": {
                "should": scope, "minimum_should_match": block.minimum_match
            }})];
            if block.should {
                must.push(json!({"bool": {"should": others}}));
            }
            must
        }
    };
    json!({"function_score": {"query": {"bool": {"must": must}}}})
}

fn build_adaptative_block_match(input: &Input, block: &Block) -> Value {
    let Some(name) = input
        .name
        .as_ref()
        .filter(|n| n.last.is_some() && n.first.is_some())
    else {
        return build_advanced_match(input, Some(block));
    };
    let (last, first) = (name.last.unwrap_or(""), name.first.unwrap_or(""));
    let mut must = vec![fuzzy_term_query(
        "PRENOMS_NOM",
        &format!("{last} {first}"),
        json!("auto"),
    )];
    let mut should = vec![match_query("NOM", json!(last))];
    if let Some(legal) = name.legal {
        should.push(match_query("NOM", json!(legal)));
    }
    if let Some(date) = &input.birth_date {
        match date {
            DateInput::Range(..) => {
                must.push(date_range_string_query("DATE_NAISSANCE", date, true));
            }
            DateInput::Single(text) => {
                must.push(json!({"bool": {
                    "should": [
                        fuzzy_term_query("DATE_NAISSANCE", text, json!(1)),
                        prefix_query("DATE_NAISSANCE", &text[..4])
                    ],
                    "minimum_should_match": 1
                }}));
                should.push(match_query("DATE_NAISSANCE", json!(text)));
            }
        }
    }
    if let Some(date) = &input.death_date {
        match date {
            DateInput::Range(..) => must.push(date_range_string_query("DATE_DECES", date, true)),
            DateInput::Single(text) => should.push(match_query("DATE_DECES", json!(text))),
        }
    }
    json!({"function_score": {"query": {"bool": {
        "must": must, "should": should, "minimum_should_match": 1
    }}}})
}

fn build_index_match(id: &str) -> Value {
    json!({"bool": {"must": [{"match": {"UID": id}}]}})
}

fn build_simple_match(q: &str) -> Value {
    let terms: Vec<&str> = q.split_whitespace().collect();
    let date: Option<String> = terms
        .iter()
        .find(|t| {
            t.len() == 10
                && t.as_bytes()[2] == b'/'
                && t.as_bytes()[5] == b'/'
                && t.chars().filter(char::is_ascii_digit).count() == 8
        })
        .map(|t| format!("{}{}{}", &t[6..10], &t[3..5], &t[0..2]));
    let stop = ["el", "d", "le", "de", "la", "los"];
    let names: Vec<&str> = terms
        .iter()
        .copied()
        .filter(|t| t.chars().any(|c| c.is_ascii_alphabetic()))
        .filter(|t| !stop.contains(&t.to_lowercase().as_str()))
        .collect();
    let names_query = (!names.is_empty()).then(|| {
        let joined = names.join(" ");
        let mut must =
            vec![json!({"match": {"PRENOMS_NOM": {"query": joined, "fuzziness": "auto"}}})];
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
    });
    let date_query = date.map(|date| {
        json!({"bool": {"minimum_should_match": 1, "should": [
            {"match": {"DATE_NAISSANCE": {"query": date, "fuzziness": "auto"}}},
            {"match": {"DATE_DECES": {"query": date, "fuzziness": "auto"}}}
        ]}})
    });
    match (names_query, date_query) {
        (Some(names), Some(date)) => {
            json!({"function_score": {"query": {"bool": {"must": [names], "should": [date]}}}})
        }
        (None, Some(date)) => date,
        (Some(names), None) => names,
        (None, None) => json!({"match_all": {}}),
    }
}

const REFERENCE_SORT: &[(&str, &str)] = &[
    ("firstName", "PRENOM.raw"),
    ("lastName", "NOM.raw"),
    ("sex", "SEXE"),
    ("birthDate", "DATE_NAISSANCE_NORM"),
    ("birthCity", "COMMUNE_NAISSANCE.raw"),
    ("deathDate", "DATE_DECES_NORM"),
    ("deathAge", "AGE_DECES"),
    ("deathCity", "COMMUNE_DECES.raw"),
    ("deathDepartment", "DEPARTEMENT_DECES"),
];

fn reference_field(key: &str) -> &'static str {
    REFERENCE_SORT
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, field)| *field)
        .expect("known aggregation key")
}

fn build_aggregation(aggs: &[&str], aggs_size: u32, after_key: Option<Value>) -> Value {
    let date_histogram = |key: &str| {
        json!({"date_histogram": {
            "field": reference_field(key), "calendar_interval": "month", "format": "yyyyMMdd"
        }})
    };
    let mut request = serde_json::Map::new();
    if aggs.len() == 1 && ["birthDate", "deathDate"].contains(&aggs[0]) {
        request.insert(aggs[0].to_owned(), date_histogram(aggs[0]));
    } else if aggs.len() == 1 {
        request.insert(
            aggs[0].to_owned(),
            json!({"terms": {"field": reference_field(aggs[0]), "size": aggs_size}}),
        );
    } else {
        let sources: Vec<Value> = aggs
            .iter()
            .map(|agg| {
                if ["birthDate", "deathDate"].contains(agg) {
                    json!({*agg: date_histogram(agg)})
                } else {
                    json!({*agg: {"terms": {"field": reference_field(agg)}}})
                }
            })
            .collect();
        let mut composite = json!({"size": 1000, "sources": sources});
        match after_key {
            Some(after) => {
                composite["after"] = after;
            }
            None => {
                for agg in aggs {
                    request.insert(
                        format!("{agg}_count"),
                        json!({"cardinality": {"field": reference_field(agg)}}),
                    );
                }
            }
        }
        request.insert("bucketResults".to_owned(), json!({"composite": composite}));
    }
    Value::Object(request)
}

const SOURCE_FIELDS: &[&str] = &[
    "CODE_INSEE_DECES",
    "CODE_INSEE_NAISSANCE",
    "COMMUNE_DECES",
    "COMMUNE_NAISSANCE",
    "DATE_DECES",
    "DATE_NAISSANCE",
    "NOM",
    "PRENOM",
    "PRENOMS",
    "SEXE",
    "UID",
    "GEOPOINT_NAISSANCE",
    "GEOPOINT_DECES",
];

struct Envelope {
    full_text: bool,
    block: bool,
    size: u32,
    sort: Vec<Value>,
    aggs: Option<Value>,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            full_text: false,
            block: false,
            size: 20,
            sort: vec![json!({"_score": "desc"})],
            aggs: None,
        }
    }
}

/// `buildRequest` body around a `buildMatch` result.
fn build_request(matched: Value, envelope: Envelope) -> Value {
    let mut body = json!({
        "min_score": if envelope.full_text { 5 } else { 0 },
        "track_total_hits": !envelope.block,
        "_source": SOURCE_FIELDS,
        "query": {"bool": {"must": [matched]}},
        "sort": envelope.sort,
        "size": envelope.size,
    });
    if envelope.size > 0 {
        body["from"] = json!(0);
    }
    if let Some(aggs) = envelope.aggs {
        body["aggs"] = aggs;
    }
    body
}

// ---------------------------------------------------------------------------
// The shape catalogue.
// ---------------------------------------------------------------------------

fn jean_dupont() -> Name {
    Name {
        first: Some("Jean"),
        last: Some("Dupont"),
        legal: None,
    }
}

fn advanced(input: Input) -> Value {
    build_request(build_advanced_match(&input, None), Envelope::default())
}

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

/// Field values of the first corpus document matching `keep`, so the
/// synthetic inputs below hit the corpus (non-vacuous checks).
fn sample(docs: &[(String, Value)], keep: impl Fn(&Value) -> bool) -> &Value {
    &docs
        .iter()
        .find(|(_, source)| keep(source))
        .expect("corpus holds a sample")
        .1
}

/// `(label, body, transport)`: `true` sends it through `_msearch`.
fn shapes(docs: &[(String, Value)]) -> Vec<(String, Value, bool)> {
    let fuzzy = |input: Input| Input {
        fuzzy: true,
        ..input
    };
    let dupont = sample(docs, |s| s["NOM"] == "DUPONT" && s["PRENOM"] == "JEAN");
    let field = |source: &Value, name: &str| leak(source[name].as_str().unwrap_or_default());
    let birth = field(dupont, "DATE_NAISSANCE");
    let birth_city = field(dupont, "COMMUNE_NAISSANCE");
    let marseille = sample(docs, |s| s["COMMUNE_NAISSANCE"] == "Marseille");
    let marseille_source = field(marseille, "SOURCE");
    let marseille_death_dep = field(marseille, "DEPARTEMENT_DECES");
    let mut out: Vec<(String, Value, bool)> = Vec::new();
    let mut push = |label: &str, body: Value, msearch: bool| {
        out.push((label.to_owned(), body, msearch));
    };

    // Simple search (`q`), min_score 5.
    for q in [
        "jean dupont",
        "dupont",
        "dupont jean 12/03/1950",
        "12/03/1950",
        "le de",
        "marie louise martin",
    ] {
        let full_text = Envelope {
            full_text: true,
            ..Envelope::default()
        };
        push(
            &format!("simple q={q:?}"),
            build_request(build_simple_match(q), full_text),
            false,
        );
    }

    // `id` lookup (updatedIds `_msearch`).
    push(
        "index match by UID (msearch)",
        build_request(build_index_match("uid00042"), Envelope::default()),
        true,
    );

    // Advanced search, fuzzy (deces-backend default) and exact.
    for fuzzy_on in [true, false] {
        let tag = if fuzzy_on { "fuzzy" } else { "exact" };
        let base = |input: Input| Input {
            fuzzy: fuzzy_on,
            ..input
        };
        push(
            &format!("advanced {tag} first+last"),
            advanced(base(Input {
                name: Some(jean_dupont()),
                ..Input::default()
            })),
            false,
        );
        push(
            &format!("advanced {tag} first+last+legal"),
            advanced(base(Input {
                name: Some(Name {
                    legal: Some("Martin"),
                    ..jean_dupont()
                }),
                ..Input::default()
            })),
            false,
        );
        push(
            &format!("advanced {tag} last only"),
            advanced(base(Input {
                name: Some(Name {
                    last: Some("Durand"),
                    ..Name::default()
                }),
                ..Input::default()
            })),
            false,
        );
        push(
            &format!("advanced {tag} name + full birth date + birth city"),
            advanced(base(Input {
                name: Some(jean_dupont()),
                birth_date: Some(DateInput::Single(birth)),
                birth_city: Some(birth_city),
                ..Input::default()
            })),
            false,
        );
        push(
            &format!("advanced {tag} every field"),
            advanced(base(Input {
                name: Some(jean_dupont()),
                sex: Some("M"),
                birth_date: Some(DateInput::Range(Some("1920"), Some("1960"))),
                birth_city: Some("Paris"),
                birth_location_code: Some("75056"),
                birth_postal_code: Some("75000"),
                birth_department: Some("75"),
                birth_country: Some("France"),
                birth_geo_point: Some((48.8566, 2.3522, "5km")),
                death_date: Some(DateInput::Range(Some("1990"), None)),
                death_age: Some(AgeInput::Range(30, 95)),
                death_city: Some("Lyon"),
                death_location_code: Some("69123"),
                death_postal_code: Some("69000"),
                death_department: Some("69"),
                death_country: Some("France"),
                death_geo_point: Some((45.7640, 4.8357, "20km")),
                source: Some("deces-2001-m01"),
                ..Input::default()
            })),
            false,
        );
    }
    let date_shapes: Vec<(&str, DateInput)> = vec![
        ("birth year prefix", DateInput::Single(&birth[..4])),
        ("birth year-month prefix", DateInput::Single(&birth[..6])),
        ("birth range", DateInput::Range(Some("1940"), Some("1945"))),
        (
            "birth range full dates",
            DateInput::Range(Some("19400101"), Some("19451231")),
        ),
        (
            "birth lower limit",
            DateInput::Range(Some("19800101"), None),
        ),
        ("birth upper limit", DateInput::Range(None, Some("1910"))),
    ];
    for (label, date) in date_shapes {
        push(
            &format!("advanced name + {label}"),
            advanced(fuzzy(Input {
                name: Some(jean_dupont()),
                birth_date: Some(date.clone()),
                ..Input::default()
            })),
            false,
        );
        push(
            &format!("advanced {label} alone"),
            advanced(fuzzy(Input {
                birth_date: Some(date),
                ..Input::default()
            })),
            false,
        );
    }
    push(
        "advanced lastSeenAliveDate (death > date) + name",
        advanced(fuzzy(Input {
            name: Some(jean_dupont()),
            death_date: Some(DateInput::Range(Some("20000101"), None)),
            ..Input::default()
        })),
        false,
    );
    push(
        "advanced death age exact + sex",
        advanced(fuzzy(Input {
            sex: Some("F"),
            death_age: Some(AgeInput::Single(85)),
            ..Input::default()
        })),
        false,
    );
    push(
        "advanced death age range alone",
        advanced(fuzzy(Input {
            death_age: Some(AgeInput::Range(60, 70)),
            ..Input::default()
        })),
        false,
    );
    push(
        "advanced birth geo point alone",
        advanced(fuzzy(Input {
            birth_geo_point: Some((48.8566, 2.3522, "2km")),
            ..Input::default()
        })),
        false,
    );
    push(
        "advanced death geo point (lat,lon string) + name",
        advanced(fuzzy(Input {
            name: Some(jean_dupont()),
            death_geo_point: Some((45.4397, 4.3872, "10km")),
            ..Input::default()
        })),
        false,
    );
    push(
        "advanced city/country/codes + source",
        advanced(fuzzy(Input {
            birth_city: Some("Marseile"),
            birth_country: Some("France"),
            death_department: Some(marseille_death_dep),
            source: Some(marseille_source),
            ..Input::default()
        })),
        false,
    );
    let sorted = Envelope {
        sort: vec![
            json!({"NOM.raw": "asc"}),
            json!({"DATE_NAISSANCE_NORM": "desc"}),
        ],
        ..Envelope::default()
    };
    push(
        "advanced name sorted by lastName, birthDate",
        build_request(
            build_advanced_match(
                &fuzzy(Input {
                    name: Some(jean_dupont()),
                    ..Input::default()
                }),
                None,
            ),
            sorted,
        ),
        false,
    );
    push(
        "advanced without criteria (function_score over empty bool)",
        advanced(fuzzy(Input::default())),
        false,
    );

    // Link: adaptive block (`_msearch` chunks, track_total_hits false).
    let link_block = Block {
        scope: &["name", "birthDate"],
        minimum_match: 1,
        should: true,
    };
    let link = |input: Input| {
        build_request(
            build_adaptative_block_match(&input, &link_block),
            Envelope {
                block: true,
                ..Envelope::default()
            },
        )
    };
    push(
        "link block name + full birth date",
        link(fuzzy(Input {
            name: Some(jean_dupont()),
            birth_date: Some(DateInput::Single(birth)),
            ..Input::default()
        })),
        true,
    );
    push(
        "link block name + legal + birth range + death range",
        link(fuzzy(Input {
            name: Some(Name {
                legal: Some("Martin"),
                ..jean_dupont()
            }),
            birth_date: Some(DateInput::Range(Some("1930"), Some("1960"))),
            death_date: Some(DateInput::Range(Some("1990"), Some("2010"))),
            ..Input::default()
        })),
        true,
    );
    push(
        "link block name + exact death date + city",
        link(fuzzy(Input {
            name: Some(jean_dupont()),
            death_date: Some(DateInput::Single("20010412")),
            birth_city: Some("Paris"),
            ..Input::default()
        })),
        true,
    );
    push(
        "link block without first name (advanced block)",
        link(fuzzy(Input {
            name: Some(Name {
                last: Some("Dupont"),
                ..Name::default()
            }),
            birth_date: Some(DateInput::Range(Some("1930"), Some("1960"))),
            birth_city: Some("Lyon"),
            sex: Some("M"),
            ..Input::default()
        })),
        true,
    );
    push(
        "link block without first name, no other field",
        link(fuzzy(Input {
            name: Some(Name {
                last: Some("Durand"),
                ..Name::default()
            }),
            birth_date: Some(DateInput::Single("1950")),
            ..Input::default()
        })),
        true,
    );

    // `/agg`: size 0, aggregations, `/agg` without criteria.
    let agg = |input: Input, aggs: &[&str], after: Option<Value>| {
        build_request(
            build_advanced_match(&input, None),
            Envelope {
                size: 0,
                aggs: Some(build_aggregation(aggs, 250, after)),
                ..Envelope::default()
            },
        )
    };
    push(
        "agg terms sex, name criteria",
        agg(
            fuzzy(Input {
                name: Some(jean_dupont()),
                ..Input::default()
            }),
            &["sex"],
            None,
        ),
        false,
    );
    push(
        "agg date_histogram birthDate, birth range criteria",
        agg(
            fuzzy(Input {
                birth_date: Some(DateInput::Range(Some("1930"), Some("1960"))),
                ..Input::default()
            }),
            &["birthDate"],
            None,
        ),
        false,
    );
    push(
        "agg composite + cardinality, city criteria",
        agg(
            fuzzy(Input {
                death_city: Some("Lyon"),
                ..Input::default()
            }),
            &["lastName", "deathDepartment", "deathDate"],
            None,
        ),
        false,
    );
    push(
        "agg composite after key, age range criteria",
        agg(
            fuzzy(Input {
                death_age: Some(AgeInput::Range(50, 90)),
                ..Input::default()
            }),
            &["lastName", "sex"],
            Some(json!({"lastName": "dupont", "sex": "F"})),
        ),
        false,
    );
    push(
        "agg terms without criteria",
        agg(fuzzy(Input::default()), &["deathCity"], None),
        false,
    );
    out
}

// ---------------------------------------------------------------------------
// Execution.
// ---------------------------------------------------------------------------

fn hit_ids(response: &Value) -> BTreeSet<String> {
    response["hits"]["hits"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .filter_map(|hit| hit["_id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Every stored document `query_matches` accepts — the brute-force oracle.
fn brute_force(state: &AppState, query: &Value) -> BTreeSet<String> {
    let parsed = parse_search_request(&json!({"query": query}).to_string())
        .expect("oracle query parses")
        .query
        .expect("query present");
    let mapping = state.index_mapping(INDEX).expect("mapping");
    state
        .documents(INDEX)
        .into_iter()
        .filter(|document| query_matches(&parsed, &document.source, &mapping))
        .map(|document| document.id)
        .collect()
}

async fn run_shape(
    router: &axum::Router,
    state: &AppState,
    label: &str,
    body: &Value,
    msearch: bool,
    layout: Layout,
) -> usize {
    // 1. The real request, through the transport deces-backend uses.
    take_full_scan_fallbacks_for_test();
    let response = if msearch {
        let ndjson = format!("{}\n{}\n", json!({"index": INDEX}), body);
        let multi = send(router, Method::POST, "/_msearch", ndjson).await;
        multi["responses"][0].clone()
    } else {
        send(
            router,
            Method::POST,
            &format!("/{INDEX}/_search?request_cache=false"),
            body.to_string(),
        )
        .await
    };
    assert!(
        response["hits"].is_object(),
        "{label} ({layout:?}) failed: {response}"
    );
    assert_eq!(
        take_full_scan_fallbacks_for_test(),
        0,
        "{label} ({layout:?}) took the _source full scan"
    );

    // 2. Its match set (no min_score, no window) against brute force.
    let query = body["query"].clone();
    let request = parse_search_request(
        &json!({"query": query, "size": 10_000, "track_total_hits": true}).to_string(),
    )
    .expect("match-set request parses");
    let matched = run_search(state, &[INDEX.to_owned()], &request);
    assert_eq!(
        take_full_scan_fallbacks_for_test(),
        0,
        "{label} ({layout:?}) match-set run took the _source full scan"
    );
    let matched = serde_json::to_value(matched).expect("response serialises");
    let expected = brute_force(state, &query);
    assert_eq!(
        hit_ids(&matched),
        expected,
        "{label} ({layout:?}): postings match set differs from brute force"
    );
    expected.len()
}

#[test]
fn every_build_request_shape_is_served_without_full_scan() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let docs = corpus(600);
    let shapes = shapes(&docs);
    assert_eq!(shapes.len(), 47, "buildRequest.ts shapes under guard");
    for layout in [Layout::SingleRam, Layout::MultiSegment, Layout::DiskBacked] {
        runtime.block_on(async {
            let (router, state) = build(layout, &docs).await;
            let mut empty = Vec::new();
            // `Some(0)`: range / geo clauses always walk their term
            // dictionary, even restricted to a small sibling set; `None`:
            // production threshold (small sets checked on `_source`).
            for source_check_max in [Some(0), None] {
                set_term_scan_source_check_max_for_test(source_check_max);
                for (label, body, msearch) in &shapes {
                    let matched = run_shape(&router, &state, label, body, *msearch, layout).await;
                    if matched == 0 && source_check_max.is_none() {
                        empty.push(label.clone());
                    }
                }
            }
            set_term_scan_source_check_max_for_test(None);
            // The synthetic inputs are chosen to hit the corpus: an empty
            // match set would make a shape's check vacuous. Only the
            // "every field" conjunctions may legitimately match nothing.
            // Lot L2 closed the former `DATE_NAISSANCE.raw` /
            // `COMMUNE_NAISSANCE.raw` exception: a `match` on a keyword
            // sub-field now analyses the query with its normalizer.
            empty.retain(|label| !label.ends_with("every field"));
            assert!(
                empty.is_empty(),
                "{layout:?}: shapes with an empty match set: {empty:?}"
            );
        });
    }
}

#[test]
fn a_geo_point_indexed_as_lon_lat_array_declines_to_the_counted_scan() {
    // The `[lon, lat]` array form is flattened into two bare numbers by the
    // indexer: the term walk cannot pair them back, so the geo filter
    // declines and the full scan (still correct) is counted.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let mut docs = corpus(40);
        docs[0].1["GEOPOINT_NAISSANCE"] = json!([2.3522, 48.8566]);
        let (router, state) = build(Layout::SingleRam, &docs).await;
        let query = geo_point_query("GEOPOINT_NAISSANCE", 48.8566, 2.3522, "50km");
        let body = json!({"query": query, "size": 100, "track_total_hits": true});
        take_full_scan_fallbacks_for_test();
        let response = send(
            &router,
            Method::POST,
            &format!("/{INDEX}/_search?request_cache=false"),
            body.to_string(),
        )
        .await;
        assert_eq!(take_full_scan_fallbacks_for_test(), 1);
        assert_eq!(hit_ids(&response), brute_force(&state, &query));
        assert!(hit_ids(&response).contains("uid00000"));
    });
}

#[test]
fn range_follows_the_es_semantics_of_each_field_type() {
    // Independent oracle: the expected sets are computed from the corpus
    // values in Rust, not with `query_matches`.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let docs = corpus(400);
    let expect = |keep: &dyn Fn(&Value) -> bool| -> BTreeSet<String> {
        docs.iter()
            .filter(|(_, source)| keep(source))
            .map(|(id, _)| id.clone())
            .collect()
    };
    let text = |source: &Value, field: &str| source[field].as_str().unwrap_or("").to_owned();
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        (
            // `text` field: TermRangeQuery over the analysed term, raw bounds.
            "text DATE_NAISSANCE gte/lte",
            json!({"range": {"DATE_NAISSANCE": {"gte": "19400000", "lte": "19459999"}}}),
            expect(&|s| {
                let d = text(s, "DATE_NAISSANCE");
                d.as_str() >= "19400000" && d.as_str() <= "19459999"
            }),
        ),
        (
            "text DATE_DECES gt/lt with numeric bounds",
            json!({"range": {"DATE_DECES": {"gt": 19900101, "lt": 20000101}}}),
            expect(&|s| {
                let d = text(s, "DATE_DECES");
                d.as_str() > "19900101" && d.as_str() < "20000101"
            }),
        ),
        (
            // `keyword` sub-field: same term walk on the `.raw` postings.
            "keyword DATE_NAISSANCE.raw lte",
            json!({"range": {"DATE_NAISSANCE.raw": {"lte": "19109999"}}}),
            expect(&|s| text(s, "DATE_NAISSANCE").as_str() <= "19109999"),
        ),
        (
            // keyword + normalizer: the bound is normalised like the terms.
            "keyword+normalizer COMMUNE_NAISSANCE.raw gte LYON lt PARIS",
            json!({"range": {"COMMUNE_NAISSANCE.raw": {"gte": "LYON", "lt": "PARIS"}}}),
            expect(&|s| {
                let c = text(s, "COMMUNE_NAISSANCE").to_lowercase();
                c.as_str() >= "lyon" && c.as_str() < "paris"
            }),
        ),
        (
            // numeric `short`: numeric order, not the terms' byte order
            // ("100" < "60" as bytes).
            "short AGE_DECES gte 60 lte 99",
            json!({"range": {"AGE_DECES": {"gte": 60, "lte": 99}}}),
            expect(&|s| (60..=99).contains(&s["AGE_DECES"].as_u64().unwrap_or(0))),
        ),
        (
            "short AGE_DECES string bound gt \"95\"",
            json!({"range": {"AGE_DECES": {"gt": "95"}}}),
            expect(&|s| s["AGE_DECES"].as_u64().unwrap_or(0) > 95),
        ),
        (
            "integer SOURCE_LINE gte 10 lt 20 (whole dictionary walk)",
            json!({"range": {"SOURCE_LINE": {"gte": 10, "lt": 20}}}),
            expect(&|s| (10..20).contains(&s["SOURCE_LINE"].as_u64().unwrap_or(0))),
        ),
    ];
    let males = expect(&|s| s["SEXE"] == "M");
    for layout in [Layout::SingleRam, Layout::MultiSegment, Layout::DiskBacked] {
        runtime.block_on(async {
            let (_router, state) = build(layout, &docs).await;
            for (label, query, expected) in &cases {
                assert!(!expected.is_empty(), "{label}: vacuous case");
                let filtered: BTreeSet<String> = expected.intersection(&males).cloned().collect();
                for (wrapped, want) in [
                    (query.clone(), expected.clone()),
                    // as a filter of a bool driven by a postings clause
                    (
                        json!({"bool": {"must": [{"match": {"SEXE": "M"}}], "filter": [query]}}),
                        filtered,
                    ),
                ] {
                    let request = parse_search_request(
                        &json!({"query": wrapped, "size": 10_000, "track_total_hits": true})
                            .to_string(),
                    )
                    .expect("request parses");
                    for source_check_max in [Some(0), None] {
                        set_term_scan_source_check_max_for_test(source_check_max);
                        take_full_scan_fallbacks_for_test();
                        let response = run_search(&state, &[INDEX.to_owned()], &request);
                        assert_eq!(
                            take_full_scan_fallbacks_for_test(),
                            0,
                            "{label} ({layout:?}) took the full scan"
                        );
                        let got = hit_ids(&serde_json::to_value(response).expect("serialises"));
                        assert_eq!(
                            got, want,
                            "{label} ({layout:?}, {source_check_max:?}): {wrapped}"
                        );
                    }
                    set_term_scan_source_check_max_for_test(None);
                }
            }
        });
    }
}

#[test]
fn keyword_prefix_is_served_on_every_layout() {
    // The keyword-prefix walk used to read the RAM postings channel, empty
    // on a disk-backed layout (silently no hit).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let docs = corpus(200);
    let expected: BTreeSet<String> = docs
        .iter()
        .filter(|(_, s)| {
            s["CODE_INSEE_NAISSANCE"]
                .as_str()
                .unwrap_or("")
                .starts_with("69")
        })
        .map(|(id, _)| id.clone())
        .collect();
    assert!(!expected.is_empty());
    for layout in [Layout::SingleRam, Layout::MultiSegment, Layout::DiskBacked] {
        runtime.block_on(async {
            let (_router, state) = build(layout, &docs).await;
            let request = parse_search_request(
                &json!({"query": {"prefix": {"CODE_INSEE_NAISSANCE": "69"}},
                        "size": 10_000, "track_total_hits": true})
                .to_string(),
            )
            .expect("request parses");
            take_full_scan_fallbacks_for_test();
            let response = run_search(&state, &[INDEX.to_owned()], &request);
            assert_eq!(take_full_scan_fallbacks_for_test(), 0, "{layout:?}");
            let got = hit_ids(&serde_json::to_value(response).expect("serialises"));
            assert_eq!(got, expected, "{layout:?}");
        });
    }
}
