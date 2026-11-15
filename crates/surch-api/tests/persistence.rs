//! MVP L6 gate — persistent index: restart without re-indexing
//! (`plan/mvp-matchid.md` L6, `docs/ops/persistence.md`).
//!
//! Every test builds state through the public HTTP router on top of
//! `AppState::open_with_data_dir`, records a battery of responses, drops
//! EVERY handle on the state (router included, which also releases the data
//! directory lock), reopens the directory and requires the very same bytes
//! back — `took` excepted, it is wall-clock time. The battery replays the
//! frozen matchID `deces` slice with its v2 mapping (custom analyzers,
//! `raw`/`autocomplete` multi-fields) and the `deces_v1` replay set, plus
//! fuzzy / range / prefix / sort / aggregation / alias / geo / date requests
//! and documents updated and deleted between two refreshes.
//!
//! Test data directories live under `$TMPDIR/l6-data/` (override with
//! `SURCH_L6_TEST_DIR`) and are removed when each test ends.

use std::path::{Path, PathBuf};

use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
    Router,
};
use serde_json::{json, Value};
use surch_api::{app_router_with_state, state::AppState, AppRouterState};
use tower::ServiceExt;

const MAPPING_V2: &str = include_str!("../../../tests/matchid_compat/deces/mapping_v2.json");
const SLICE_NDJSON: &str = include_str!("../../../tests/matchid_compat/deces/slice-1000.ndjson");
const REPLAY: &str = "../../tests/matchid_compat/replays/deces_v1.json";

struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let base = std::env::var_os("SURCH_L6_TEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("l6-data"));
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = base.join(format!("{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &Path) -> (Router, AppState) {
    let app = AppState::open_with_data_dir(dir).expect("data directory opens");
    let router = app_router_with_state(AppRouterState {
        app: app.clone(),
        ..Default::default()
    });
    (router, app)
}

#[derive(Clone)]
struct Call {
    method: Method,
    path: String,
    body: String,
    ndjson: bool,
}

impl Call {
    fn new(method: Method, path: &str, body: Value) -> Self {
        Self {
            method,
            path: path.to_owned(),
            body: if body.is_null() {
                String::new()
            } else {
                body.to_string()
            },
            ndjson: false,
        }
    }

    fn ndjson(path: &str, lines: String) -> Self {
        Self {
            method: Method::POST,
            path: path.to_owned(),
            body: lines,
            ndjson: true,
        }
    }
}

async fn call(router: &Router, call: &Call) -> (u16, Vec<u8>) {
    let content_type = if call.ndjson {
        "application/x-ndjson"
    } else {
        "application/json"
    };
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(call.method.clone())
                .uri(&call.path)
                .header("content-type", content_type)
                .body(Body::from(call.body.clone()))
                .expect("request builds"),
        )
        .await
        .expect("router responds");
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body readable")
        .to_vec();
    (status, bytes)
}

async fn must(router: &Router, request: Call) -> Vec<u8> {
    let (status, body) = call(router, &request).await;
    assert!(
        (200..300).contains(&status),
        "{} {} -> {status}: {}",
        request.method,
        request.path,
        String::from_utf8_lossy(&body)
    );
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_ne!(
        parsed.get("errors"),
        Some(&Value::Bool(true)),
        "{} reported item errors: {}",
        request.path,
        String::from_utf8_lossy(&body)
    );
    body
}

/// Replace every `"took":<digits>` value by `0` — the only field allowed to
/// differ between two executions of the same request. Everything else is
/// compared byte for byte (scores, sort values, aggregation buckets, order).
fn normalize(bytes: &[u8]) -> Vec<u8> {
    const KEY: &[u8] = b"\"took\":";
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(KEY) {
            out.extend_from_slice(KEY);
            i += KEY.len();
            out.push(b'0');
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn slice_chunks(chunks: usize) -> Vec<String> {
    let lines: Vec<&str> = SLICE_NDJSON.lines().filter(|l| !l.is_empty()).collect();
    let docs = lines.len() / 2;
    let per = docs.div_ceil(chunks);
    lines
        .chunks(per * 2)
        .map(|chunk| {
            let mut out = chunk.join("\n");
            out.push('\n');
            out
        })
        .collect()
}

fn places_ndjson(range: std::ops::Range<usize>, variant: &str) -> String {
    const KINDS: [&str; 3] = ["museum", "park", "station"];
    let mut out = String::new();
    for i in range {
        let lat = 43.0 + (i % 17) as f64 * 0.37;
        let lon = -1.0 + (i % 13) as f64 * 0.41;
        let year = 1990 + (i % 30);
        let month = 1 + (i % 12);
        out.push_str(&format!(
            "{{\"index\":{{\"_index\":\"places\",\"_id\":\"p{i}\"}}}}\n"
        ));
        out.push_str(
            &json!({
                "name": format!("{variant} {} place {}", KINDS[i % 3], i % 7),
                "kind": KINDS[i % 3],
                "location": {"lat": lat, "lon": lon},
                "opened": format!("{year}{month:02}15"),
                "rating": (i % 5) as f64 + 0.5,
            })
            .to_string(),
        );
        out.push('\n');
    }
    out
}

/// Build the two indices + aliases, with a refresh after each write wave.
async fn load(router: &Router, app: &AppState) {
    let mapping: Value = serde_json::from_str(MAPPING_V2).unwrap();
    must(router, Call::new(Method::PUT, "/deces", mapping)).await;
    // Disk-backed postings + one sealed segment per bulk chunk + no merge:
    // the reopened index must serve a genuinely multi-segment layout.
    app.set_postings_disk_enabled("deces", true);
    app.set_flush_budget_bytes_override("deces", Some(1));
    app.set_merge_fanin_override("deces", 0);
    for chunk in slice_chunks(4) {
        must(router, Call::ndjson("/_bulk", chunk)).await;
    }
    must(
        router,
        Call::new(Method::POST, "/deces/_refresh", Value::Null),
    )
    .await;
    assert!(
        app.index_segment_count("deces") >= 4,
        "deces must be multi-segment"
    );

    must(
        router,
        Call::new(
            Method::PUT,
            "/places",
            json!({
                "settings": {"number_of_replicas": 0},
                "mappings": {"properties": {
                    "name": {"type": "text", "analyzer": "standard"},
                    "kind": {"type": "keyword"},
                    "location": {"type": "geo_point"},
                    "opened": {"type": "date", "format": "yyyyMMdd"},
                    "rating": {"type": "float"}
                }},
                "aliases": {"landmarks": {}}
            }),
        ),
    )
    .await;
    must(router, Call::ndjson("/_bulk", places_ndjson(0..60, "old"))).await;
    must(
        router,
        Call::new(Method::POST, "/places/_refresh", Value::Null),
    )
    .await;
    // Second wave: updates of existing ids, deletes, fresh inserts.
    let mut wave = places_ndjson(10..20, "new");
    for i in [3, 4, 25, 59] {
        wave.push_str(&format!(
            "{{\"delete\":{{\"_index\":\"places\",\"_id\":\"p{i}\"}}}}\n"
        ));
    }
    wave.push_str(&places_ndjson(60..75, "fresh"));
    must(router, Call::ndjson("/_bulk", wave)).await;
    must(
        router,
        Call::new(Method::POST, "/places/_refresh", Value::Null),
    )
    .await;

    must(
        router,
        Call::new(
            Method::POST,
            "/_aliases",
            json!({"actions": [
                {"add": {"index": "deces", "alias": "everything"}},
                {"add": {"index": "places", "alias": "everything"}},
                {"add": {"index": "deces", "alias": "deces-current", "is_write_index": true}}
            ]}),
        ),
    )
    .await;
}

fn replay_calls() -> Vec<Call> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(REPLAY);
    let manifest = matchid_replay::load_replay(path.to_str().unwrap()).expect("replay parses");
    let mut calls = Vec::new();
    for entry in manifest.requests {
        if entry.skip.is_some() || entry.request.path.contains("scroll") {
            continue;
        }
        let method = Method::from_bytes(entry.request.method.as_bytes()).unwrap();
        match (&entry.request.body, &entry.request.body_ndjson) {
            (_, Some(lines)) => {
                let mut body = String::new();
                for line in lines {
                    body.push_str(&line.to_string());
                    body.push('\n');
                }
                calls.push(Call {
                    method,
                    path: entry.request.path.clone(),
                    body,
                    ndjson: true,
                });
            }
            (Some(body), None) => calls.push(Call::new(method, &entry.request.path, body.clone())),
            (None, None) => calls.push(Call::new(method, &entry.request.path, Value::Null)),
        }
    }
    assert!(calls.len() >= 25, "replay set unexpectedly small");
    calls
}

fn battery() -> Vec<Call> {
    let mut calls = replay_calls();
    let search = |path: &str, body: Value| Call::new(Method::POST, path, body);
    calls.extend([
        search("/deces/_search", json!({"query": {"fuzzy": {"NOM": {"value": "DUPOMT", "fuzziness": "AUTO"}}}, "size": 7})),
        search("/deces/_search", json!({"query": {"match": {"NOM": {"query": "martn", "fuzziness": 1}}}, "size": 7})),
        search("/deces/_search", json!({"query": {"range": {"SOURCE_LINE": {"gte": 100, "lte": 260}}}, "sort": [{"SOURCE_LINE": "desc"}], "size": 9})),
        search("/deces/_search", json!({"query": {"range": {"DATE_DECES": {"gte": "19900101", "lt": "20000101"}}}, "size": 5})),
        search("/deces/_search", json!({"size": 0, "aggs": {"sexe": {"terms": {"field": "SEXE"}}, "communes": {"cardinality": {"field": "COMMUNE_DECES"}}, "sources": {"terms": {"field": "SOURCE", "size": 3}}}})),
        search("/deces/_search", json!({"query": {"match": {"PRENOMS": "MARIE"}}, "sort": [{"NOM.raw": "asc"}, {"SOURCE_LINE": "asc"}], "size": 12})),
        search("/deces/_search", json!({"query": {"prefix": {"NOM": "dup"}}, "size": 6})),
        search("/deces/_search", json!({"query": {"match": {"NOM.autocomplete": "mar"}}, "size": 6})),
        search("/deces/_search", json!({"query": {"bool": {"must": [{"match": {"NOM": "MARTIN"}}], "filter": [{"term": {"SEXE": "F"}}], "should": [{"match": {"PRENOMS": "MARIE"}}]}}, "size": 10})),
        search("/deces/_search", json!({"query": {"terms": {"COMMUNE_DECES": ["BREST", "LEHAVRE", "ANGERS"]}}, "size": 4})),
        search("/deces/_search", json!({"query": {"multi_match": {"query": "JEAN DUPONT", "fields": ["NOM", "PRENOMS"]}}, "size": 8})),
        search("/deces/_search", json!({"query": {"match_all": {}}, "size": 3, "from": 997})),
        search("/deces/_count", json!({"query": {"term": {"SEXE": "F"}}})),
        search("/deces-current/_search", json!({"query": {"match": {"NOM": "BERNARD"}}, "size": 4})),
        search("/everything/_search", json!({"query": {"match_all": {}}, "size": 5})),
        search("/places/_search", json!({"query": {"geo_distance": {"distance": "120km", "location": {"lat": 44.5, "lon": 0.6}}}, "size": 50})),
        search("/places/_search", json!({"query": {"geo_bounding_box": {"location": {"top_left": {"lat": 47.0, "lon": -1.0}, "bottom_right": {"lat": 44.0, "lon": 2.0}}}}, "size": 50})),
        search("/places/_search", json!({"query": {"range": {"opened": {"gte": "20000101"}}}, "size": 0, "aggs": {"per_year": {"date_histogram": {"field": "opened", "calendar_interval": "year"}}, "kinds": {"terms": {"field": "kind"}}}})),
        search("/places/_search", json!({"query": {"match": {"name": "new museum place"}}, "size": 20})),
        search("/landmarks/_search", json!({"query": {"term": {"kind": "park"}}, "sort": [{"rating": "desc"}], "size": 20})),
        search("/places/_count", Value::Null),
        search("/deces/_mget", json!({"ids": ["deces_00042", "deces_00999", "missing"]})),
        search("/places/_mget", json!({"ids": ["p12", "p4", "p61"]})),
        Call::new(Method::GET, "/deces/_mapping", Value::Null),
        Call::new(Method::GET, "/deces", Value::Null),
        Call::new(Method::GET, "/places", Value::Null),
        Call::new(Method::GET, "/_alias", Value::Null),
    ]);
    calls
}

async fn record(router: &Router, calls: &[Call]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::with_capacity(calls.len());
    for request in calls {
        let (status, body) = call(router, request).await;
        out.push((status, normalize(&body)));
    }
    out
}

fn assert_same(label: &str, calls: &[Call], before: &[(u16, Vec<u8>)], after: &[(u16, Vec<u8>)]) {
    assert_eq!(before.len(), after.len());
    for ((request, left), right) in calls.iter().zip(before).zip(after) {
        assert_eq!(
            left,
            right,
            "{label}: {} {} {} diverged after reopen\nbefore: {}\nafter:  {}",
            request.method,
            request.path,
            request.body,
            String::from_utf8_lossy(&left.1),
            String::from_utf8_lossy(&right.1)
        );
    }
}

#[tokio::test]
async fn reopen_serves_byte_identical_responses() {
    let dir = TestDir::new("roundtrip");
    let calls = battery();

    let (router, app) = open(dir.path());
    load(&router, &app).await;
    let before = record(&router, &calls).await;
    let failed: Vec<String> = calls
        .iter()
        .zip(&before)
        .filter(|(_, (status, _))| *status != 200)
        .map(|(request, (status, body))| {
            format!(
                "{status} {} {} -> {}",
                request.path,
                request.body,
                String::from_utf8_lossy(body)
            )
        })
        .collect();
    assert!(failed.is_empty(), "unexpected failures: {failed:#?}");
    let segments = app.index_segment_count("deces");
    drop(router);
    drop(app);

    let (router, app) = open(dir.path());
    assert_eq!(
        app.index_segment_count("deces"),
        segments,
        "segment layout survives"
    );
    let after = record(&router, &calls).await;
    assert_same("first reopen", &calls, &before, &after);
    // Proof that nothing was re-analysed: no term dictionary was built by
    // the reopen nor by the 50+ requests served from it.
    assert_eq!(app.index_terms_build_count("deces"), 0);
    assert_eq!(app.index_terms_build_count("places"), 0);

    // Writes after a reopen follow the post-refresh rebuild path, and the
    // next refresh commits a new generation that also reopens identically.
    let mut late = places_ndjson(75..90, "late");
    late.push_str("{\"delete\":{\"_index\":\"places\",\"_id\":\"p70\"}}\n");
    must(&router, Call::ndjson("/_bulk", late)).await;
    must(
        &router,
        Call::new(Method::POST, "/places/_refresh", Value::Null),
    )
    .await;
    let second = record(&router, &calls).await;
    assert_ne!(second, after, "the extra writes must be visible");
    drop(router);
    drop(app);

    let (router, _app) = open(dir.path());
    let third = record(&router, &calls).await;
    assert_same("second reopen", &calls, &second, &third);
}

#[tokio::test]
async fn unset_data_dir_keeps_in_memory_engine_identical() {
    // Same workload on the historical in-memory engine: persistence must not
    // change a single response byte.
    let dir = TestDir::new("parity");
    let calls = battery();
    let (router, app) = open(dir.path());
    load(&router, &app).await;
    let persisted = record(&router, &calls).await;
    drop(router);
    drop(app);

    let app = AppState::default();
    let router = app_router_with_state(AppRouterState {
        app: app.clone(),
        ..Default::default()
    });
    load(&router, &app).await;
    let memory = record(&router, &calls).await;
    assert_same("in-memory vs persisted", &calls, &memory, &persisted);
    assert!(app.data_dir().is_none());
}

fn index_dir(root: &Path, name: &str) -> PathBuf {
    root.join("indices").join(format!("i-{name}"))
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn crash_before_manifest_rename_reopens_previous_generation() {
    let dir = TestDir::new("crash");
    let calls = battery();
    let (router, app) = open(dir.path());
    load(&router, &app).await;
    let committed = record(&router, &calls).await;

    // Disk state of generation G for `places`, saved before G+1 is written.
    let places = index_dir(dir.path(), "places");
    let backup = dir.path().join("backup-places");
    copy_dir(&places, &backup);
    let committed_files = files(&places);

    must(
        &router,
        Call::ndjson("/_bulk", places_ndjson(100..140, "crashed")),
    )
    .await;
    must(
        &router,
        Call::new(Method::POST, "/places/_refresh", Value::Null),
    )
    .await;
    drop(router);
    drop(app);

    // Simulate a crash between "new generation files written" and "manifest
    // renamed": restore every file of G (manifest included, nothing garbage
    // collected yet) but keep the G+1 files and the longer `source.dat`, and
    // leave a half-written manifest behind.
    for name in &committed_files {
        if name != "source.dat" {
            std::fs::copy(backup.join(name), places.join(name)).unwrap();
        }
    }
    std::fs::remove_dir_all(&backup).unwrap();
    std::fs::write(places.join("MANIFEST.tmp.4242.1.0"), b"half written").unwrap();
    // An index directory whose first manifest never landed.
    let ghost = index_dir(dir.path(), "ghost");
    std::fs::create_dir_all(&ghost).unwrap();
    std::fs::write(ghost.join("seg-00000001.seg"), b"orphan").unwrap();
    // A spill tempfile of the dead process.
    std::fs::write(
        dir.path().join("tmp").join("surch-postings-1-2-3.dat"),
        b"x",
    )
    .unwrap();

    let (router, app) = open(dir.path());
    let reopened = record(&router, &calls).await;
    assert_same("crash recovery", &calls, &committed, &reopened);
    assert!(!ghost.exists(), "uncommitted index directory is removed");
    assert!(!app.index_names().contains(&"ghost".to_owned()));
    assert_eq!(
        files(&places),
        committed_files,
        "orphans of G+1 are collected"
    );
    assert_eq!(
        std::fs::read_dir(dir.path().join("tmp")).unwrap().count(),
        0,
        "spill leftovers are swept"
    );
}

#[tokio::test]
async fn corrupt_truncated_or_foreign_version_files_fail_closed() {
    let dir = TestDir::new("failclosed");
    let (router, app) = open(dir.path());
    load(&router, &app).await;
    drop(router);
    drop(app);
    let deces = index_dir(dir.path(), "deces");
    let pick = |suffix: &str| -> PathBuf {
        let name = files(&deces)
            .into_iter()
            .find(|name| name.ends_with(suffix))
            .unwrap_or_else(|| panic!("no *{suffix} file"));
        deces.join(name)
    };

    let expect_error = |what: &str, needle: &str| {
        let error = AppState::open_with_data_dir(dir.path())
            .err()
            .unwrap_or_else(|| panic!("{what}: open must fail closed"))
            .to_string();
        assert!(error.contains(needle), "{what}: unexpected error {error}");
    };

    // Truncated postings segment.
    let pst = pick(".pst");
    let original = std::fs::read(&pst).unwrap();
    std::fs::write(&pst, &original[..original.len() / 2]).unwrap();
    expect_error("truncated .pst", "truncated or resized");
    std::fs::write(&pst, &original).unwrap();

    // Bit flip inside committed `_source` bytes.
    let source = deces.join("source.dat");
    let original = std::fs::read(&source).unwrap();
    let mut flipped = original.clone();
    flipped[original.len() / 3] ^= 0x20;
    std::fs::write(&source, &flipped).unwrap();
    expect_error("flipped source.dat", "checksum mismatch");
    std::fs::write(&source, &original).unwrap();

    // Truncated segment metadata.
    let seg = pick(".seg");
    let original = std::fs::read(&seg).unwrap();
    std::fs::write(&seg, &original[..original.len() - 5]).unwrap();
    expect_error("truncated .seg", "truncated or resized");
    std::fs::write(&seg, &original).unwrap();

    // Manifest written by a future format version.
    let manifest = deces.join("MANIFEST");
    let original = std::fs::read(&manifest).unwrap();
    let mut future = original.clone();
    future[8..12].copy_from_slice(&2u32.to_le_bytes());
    std::fs::write(&manifest, &future).unwrap();
    expect_error("manifest v2", "format version 2");
    std::fs::write(&manifest, &original).unwrap();

    // Segment file from another format version.
    let original = std::fs::read(&seg).unwrap();
    let mut future = original.clone();
    future[8..12].copy_from_slice(&9u32.to_le_bytes());
    std::fs::write(&seg, &future).unwrap();
    expect_error("segment v9", "format version 9");
    std::fs::write(&seg, &original).unwrap();

    // Untouched again: opens.
    let (_router, app) = open(dir.path());
    assert_eq!(app.count("deces"), 1000);
}

#[tokio::test]
async fn data_dir_is_exclusive_and_deletes_are_durable() {
    let dir = TestDir::new("lock");
    let (router, app) = open(dir.path());
    load(&router, &app).await;
    assert!(
        AppState::open_with_data_dir(dir.path()).is_err(),
        "a second process state on the same directory is refused"
    );
    must(&router, Call::new(Method::DELETE, "/places", Value::Null)).await;
    assert!(!index_dir(dir.path(), "places").exists());
    drop(router);
    drop(app);

    let (router, app) = open(dir.path());
    assert_eq!(app.index_names(), vec!["deces".to_owned()]);
    let aliases = must(&router, Call::new(Method::GET, "/_alias", Value::Null)).await;
    let aliases: Value = serde_json::from_slice(&aliases).unwrap();
    assert!(aliases.get("places").is_none(), "{aliases}");
    assert!(
        aliases["deces"]["aliases"].get("everything").is_some(),
        "{aliases}"
    );
}

#[tokio::test]
async fn mapping_change_and_out_of_refresh_commits_stay_consistent() {
    let dir = TestDir::new("mapping");
    let (router, app) = open(dir.path());
    must(
        &router,
        Call::new(
            Method::PUT,
            "/places",
            json!({"mappings": {"properties": {"name": {"type": "text"}, "kind": {"type": "keyword"}}}}),
        ),
    )
    .await;
    // Documents NOT refreshed, then a mapping change: it seals and commits
    // postings, id maps and `_source` together.
    must(&router, Call::ndjson("/_bulk", places_ndjson(0..30, "m"))).await;
    must(
        &router,
        Call::new(
            Method::PUT,
            "/places/_mapping",
            json!({"properties": {"note": {"type": "keyword"}}}),
        ),
    )
    .await;
    // More un-refreshed documents, then an explicit re-create through the
    // state API: the commit must seal them too, never persist postings that
    // reference doc ids the id maps do not know.
    must(&router, Call::ndjson("/_bulk", places_ndjson(30..45, "n"))).await;
    app.create_index("places", None, json!({}), Default::default());
    let calls = vec![
        Call::new(Method::GET, "/places/_mapping", Value::Null),
        Call::new(
            Method::POST,
            "/places/_search",
            json!({"query": {"match": {"name": "place"}}, "size": 50}),
        ),
        Call::new(
            Method::POST,
            "/places/_search",
            json!({"query": {"range": {"rating": {"gte": 2}}}, "sort": [{"rating": "asc"}], "size": 50}),
        ),
        Call::new(Method::POST, "/places/_count", Value::Null),
    ];
    let before = record(&router, &calls).await;
    assert!(before.iter().all(|(status, _)| *status == 200));
    assert!(String::from_utf8_lossy(&before[0].1).contains("\"note\""));
    assert!(String::from_utf8_lossy(&before[3].1).contains("\"count\":45"));
    drop(router);
    drop(app);

    let (router, _app) = open(dir.path());
    let after = record(&router, &calls).await;
    assert_same("mapping change", &calls, &before, &after);
}
