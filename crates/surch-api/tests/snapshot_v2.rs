//! L6.2 gate — monthly reload without re-indexing (`plan/mvp-matchid.md`
//! L4 / L6, `docs/ops/snapshot-v2.md`).
//!
//! The source node builds `deces` under `SURCH_DATA_DIR`, snapshots its
//! sealed segment files into an `fs` repository (same service code as the
//! `s3` one, see `snapshot_v2_s3_mock.rs` for the S3 wire path), and a node
//! with a FRESH data directory replays the matchID production restore Job
//! (`deploy/k8s/overlays/prod/elasticsearch-restore.job.yaml`) request by
//! request: readonly repository, `_restore` with the exact Job body, `_count`,
//! `_settings`, `_cluster/health?wait_for_status=green`, alias swap, marker
//! write, `_cat` prune. Searches through the swapped alias must be
//! byte-identical to the source index (`took` and the concrete `_index`
//! name excepted), and no term dictionary may be built on the restoring node.
//!
//! Test data lives under `$TMPDIR/l62-data/` (override `SURCH_L62_TEST_DIR`)
//! and is removed when each test ends.

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
const SNAPSHOT: &str = "snap-2026-11";
const TARGET: &str = "deces-snap-2026-11";

struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let base = std::env::var_os("SURCH_L62_TEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("l62-data"));
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = base.join(format!("{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    #[allow(dead_code)]
    fn path(&self) -> &Path {
        &self.0
    }

    fn sub(&self, name: &str) -> PathBuf {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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

fn open_in_memory() -> (Router, AppState) {
    let app = AppState::default();
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
}

async fn call(router: &Router, call: &Call) -> (u16, Vec<u8>) {
    let content_type = if call.ndjson {
        "application/x-ndjson"
    } else {
        "application/json"
    };
    // Like `curl` in the Job: `Accept: */*` on every request (the `_cat`
    // API answers text to it) and no Content-Type on body-less requests.
    let mut builder = Request::builder()
        .method(call.method.clone())
        .uri(&call.path)
        .header("accept", "*/*");
    if !call.body.is_empty() {
        builder = builder.header("content-type", content_type);
    }
    let response = router
        .clone()
        .oneshot(
            builder
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

async fn req(router: &Router, method: Method, path: &str, body: Value) -> (u16, Value) {
    let (status, bytes) = call(router, &Call::new(method, path, body)).await;
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

/// `curl -fsS`: any non-2xx fails the Job.
async fn must(router: &Router, method: Method, path: &str, body: Value) -> Value {
    let (status, value) = req(router, method.clone(), path, body).await;
    assert!(
        (200..300).contains(&status),
        "{method} {path} -> {status}: {value}"
    );
    value
}

async fn bulk(router: &Router, lines: String) {
    let (status, bytes) = call(
        router,
        &Call {
            method: Method::POST,
            path: "/_bulk".to_owned(),
            body: lines,
            ndjson: true,
        },
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let parsed: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["errors"], json!(false), "bulk item errors");
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

/// Source node: `deces` with the v2 mapping (custom analyzers, `raw` /
/// `autocomplete` multi-fields), multi-segment, disk postings, refreshed.
async fn build_deces(router: &Router, app: &AppState) {
    let mapping: Value = serde_json::from_str(MAPPING_V2).unwrap();
    must(router, Method::PUT, "/deces", mapping).await;
    app.set_postings_disk_enabled("deces", true);
    app.set_flush_budget_bytes_override("deces", Some(1));
    app.set_merge_fanin_override("deces", 0);
    for chunk in slice_chunks(4) {
        bulk(router, chunk).await;
    }
    must(router, Method::POST, "/deces/_refresh", Value::Null).await;
    assert!(app.index_segment_count("deces") >= 4);
}

fn replay_calls() -> Vec<Call> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(REPLAY);
    let manifest = matchid_replay::load_replay(path.to_str().unwrap()).expect("replay parses");
    let mut calls = Vec::new();
    for entry in manifest.requests {
        if entry.skip.is_some() || entry.request.path.contains("scroll") {
            continue;
        }
        if !entry.request.path.starts_with("/deces/") {
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
    assert!(calls.len() >= 20, "replay set unexpectedly small");
    calls
}

/// Searches addressed to `deces` (the index on the source node, the
/// swapped alias on the restoring node).
fn battery() -> Vec<Call> {
    let mut calls = replay_calls();
    let search = |body: Value| Call::new(Method::POST, "/deces/_search", body);
    calls.extend([
        search(json!({"query": {"fuzzy": {"NOM": {"value": "DUPOMT", "fuzziness": "AUTO"}}}, "size": 7})),
        search(json!({"query": {"match": {"NOM": {"query": "martn", "fuzziness": 1}}}, "size": 7})),
        search(json!({"query": {"range": {"SOURCE_LINE": {"gte": 100, "lte": 260}}}, "sort": [{"SOURCE_LINE": "desc"}], "size": 9})),
        search(json!({"size": 0, "aggs": {"sexe": {"terms": {"field": "SEXE"}}, "communes": {"cardinality": {"field": "COMMUNE_DECES"}}}})),
        search(json!({"query": {"match": {"PRENOMS": "MARIE"}}, "sort": [{"NOM.raw": "asc"}, {"SOURCE_LINE": "asc"}], "size": 12})),
        search(json!({"query": {"prefix": {"NOM": "dup"}}, "size": 6})),
        search(json!({"query": {"match": {"NOM.autocomplete": "mar"}}, "size": 6})),
        search(json!({"query": {"bool": {"must": [{"match": {"NOM": "MARTIN"}}], "filter": [{"term": {"SEXE": "F"}}], "should": [{"match": {"PRENOMS": "MARIE"}}]}}, "size": 10})),
        search(json!({"query": {"multi_match": {"query": "JEAN DUPONT", "fields": ["NOM", "PRENOMS"]}}, "size": 8})),
        search(json!({"query": {"match_all": {}}, "size": 3, "from": 997})),
        Call::new(Method::POST, "/deces/_count", json!({"query": {"term": {"SEXE": "F"}}})),
        Call::new(Method::POST, "/deces/_mget", json!({"ids": ["deces_00042", "deces_00999", "missing"]})),
    ]);
    calls
}

/// `took` -> 0 and the concrete restored index name -> `deces`: the only
/// differences allowed between the source index and the restored one.
fn normalize(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes).replace(TARGET, "deces");
    let bytes = text.as_bytes();
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

async fn record(router: &Router, calls: &[Call]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::with_capacity(calls.len());
    for request in calls {
        let (status, body) = call(router, request).await;
        out.push((status, normalize(&body)));
    }
    out
}

fn assert_same(label: &str, calls: &[Call], left: &[(u16, Vec<u8>)], right: &[(u16, Vec<u8>)]) {
    assert_eq!(left.len(), right.len());
    for ((request, l), r) in calls.iter().zip(left).zip(right) {
        assert_eq!(
            l,
            r,
            "{label}: {} {} {} diverged\nsource:   {}\nrestored: {}",
            request.method,
            request.path,
            request.body,
            String::from_utf8_lossy(&l.1),
            String::from_utf8_lossy(&r.1)
        );
    }
}

fn fs_repo_body(location: &Path, readonly: bool) -> Value {
    let mut settings = json!({ "location": location.display().to_string() });
    if readonly {
        settings["readonly"] = json!(true);
    }
    json!({ "type": "fs", "settings": settings })
}

/// The exact `_restore` body of the matchID prod Job (`ES_INDEX=deces`).
fn job_restore_body(target: &str) -> Value {
    json!({
        "indices": "deces",
        "ignore_unavailable": true,
        "include_global_state": false,
        "rename_pattern": "^deces$",
        "rename_replacement": target,
        "index_settings": {"index.number_of_replicas": 0}
    })
}

fn creation_date(metadata: &Value, index: &str) -> Value {
    let settings = &metadata[index]["settings"]["index"];
    settings
        .get("creation_date")
        .or_else(|| settings.get("index").and_then(|i| i.get("creation_date")))
        .cloned()
        .unwrap_or(Value::Null)
}

fn count(value: &Value) -> u64 {
    value["count"].as_u64().unwrap_or(0)
}

/// Every file under `dir`, recursively (diagnostics of the staging sweep).
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn blob_count(repo: &Path) -> usize {
    files_under(&repo.join("blobs")).len()
}

/// Replays the prod Job against `router` (FORCE_RESTORE path: `deces`
/// already serves an older snapshot through the alias).
async fn run_restore_job(router: &Router, repo_settings: Value) {
    // waiting for elasticsearch
    must(
        router,
        Method::GET,
        "/_cluster/health?wait_for_status=yellow&timeout=1s",
        Value::Null,
    )
    .await;
    // existing_count > 0, marker names another snapshot, FORCE_RESTORE=true
    assert!(count(&must(router, Method::GET, "/deces/_count", Value::Null).await) > 0);
    let marker = must(
        router,
        Method::GET,
        "/.matchid-restore-state/_doc/deces",
        Value::Null,
    )
    .await;
    assert_eq!(marker["_source"]["snapshot_name"], json!("snap-2026-10"));
    // configuring snapshot repository
    must(router, Method::PUT, "/_snapshot/matchID", repo_settings).await;
    // target_count: the target does not exist yet (curl -f fails -> empty)
    let (status, _) = req(
        router,
        Method::GET,
        &format!("/{TARGET}/_count"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 404);
    // restoring snapshot
    let restored = must(
        router,
        Method::POST,
        &format!("/_snapshot/matchID/{SNAPSHOT}/_restore?wait_for_completion=true"),
        job_restore_body(TARGET),
    )
    .await;
    assert_eq!(restored["snapshot"]["indices"], json!([TARGET]));
    assert_eq!(restored["snapshot"]["shards"]["failed"], json!(0));
    let restored_count = count(
        &must(
            router,
            Method::GET,
            &format!("/{TARGET}/_count"),
            Value::Null,
        )
        .await,
    );
    assert_eq!(restored_count, 1000);
    must(
        router,
        Method::PUT,
        &format!("/{TARGET}/_settings"),
        json!({"index": {"number_of_replicas": 0}}),
    )
    .await;
    let health = must(
        router,
        Method::GET,
        &format!("/_cluster/health/{TARGET}?wait_for_status=green&timeout=600s"),
        Value::Null,
    )
    .await;
    assert_eq!(health["status"], json!("green"));
    assert_eq!(health["indices"][TARGET]["number_of_replicas"], json!(0));
    // switch_alias (alias_exists branch)
    must(router, Method::GET, "/_alias/deces", Value::Null).await;
    must(
        router,
        Method::POST,
        "/_aliases",
        json!({"actions": [
            {"remove": {"index": "*", "alias": "deces"}},
            {"add": {"index": TARGET, "alias": "deces"}}
        ]}),
    )
    .await;
    assert_eq!(
        count(&must(router, Method::GET, "/deces/_count", Value::Null).await),
        1000
    );
    // write_marker
    must(
        router,
        Method::PUT,
        "/.matchid-restore-state/_doc/deces",
        json!({"snapshot_name": SNAPSHOT, "updated_at": "2026-11-15T00:00:00Z"}),
    )
    .await;
    // prune_stale_indices
    let (status, aliased) = call(
        router,
        &Call::new(Method::GET, "/_cat/aliases/deces?h=index", Value::Null),
    )
    .await;
    assert_eq!(status, 200);
    let aliased = String::from_utf8_lossy(&aliased).trim().to_owned();
    assert_eq!(aliased, TARGET);
    let (status, candidates) = call(
        router,
        &Call::new(Method::GET, "/_cat/indices/deces-*?h=index", Value::Null),
    )
    .await;
    assert_eq!(status, 200);
    let candidates = String::from_utf8_lossy(&candidates).into_owned();
    for candidate in candidates.split_whitespace() {
        if candidate != TARGET && candidate != aliased {
            must(
                router,
                Method::DELETE,
                &format!("/{candidate}"),
                Value::Null,
            )
            .await;
        }
    }
}

/// Restoring node in prod state: `deces` alias -> `deces-snap-2026-10`,
/// marker written for that snapshot.
async fn seed_previous_month(router: &Router) {
    must(router, Method::PUT, "/deces-snap-2026-10", json!({})).await;
    must(
        router,
        Method::PUT,
        "/deces-snap-2026-10/_doc/old-1",
        json!({"NOM": "ANCIEN"}),
    )
    .await;
    must(
        router,
        Method::POST,
        "/deces-snap-2026-10/_refresh",
        Value::Null,
    )
    .await;
    must(
        router,
        Method::POST,
        "/_aliases",
        json!({"actions": [{"add": {"index": "deces-snap-2026-10", "alias": "deces"}}]}),
    )
    .await;
    must(
        router,
        Method::PUT,
        "/.matchid-restore-state/_doc/deces",
        json!({"snapshot_name": "snap-2026-10", "updated_at": "2026-10-15T00:00:00Z"}),
    )
    .await;
}

#[tokio::test]
async fn deces_job_restores_segment_snapshot_without_reindexing() {
    let root = TestDir::new("job");
    let repo = root.sub("repo");
    let calls = battery();

    // Offline build + snapshot (source node).
    let (source, source_app) = open(&root.sub("source"));
    build_deces(&source, &source_app).await;
    let expected = record(&source, &calls).await;
    let failures: Vec<_> = calls
        .iter()
        .zip(&expected)
        .filter(|(_, (status, _))| *status != 200)
        .map(|(c, (s, b))| format!("{s} {} -> {}", c.path, String::from_utf8_lossy(b)))
        .collect();
    assert!(
        failures.is_empty(),
        "source battery failures: {failures:#?}"
    );
    let source_meta = must(&source, Method::GET, "/deces", Value::Null).await;
    must(
        &source,
        Method::PUT,
        "/_snapshot/matchID",
        fs_repo_body(&repo, false),
    )
    .await;
    let taken = must(
        &source,
        Method::PUT,
        &format!("/_snapshot/matchID/{SNAPSHOT}?wait_for_completion=true"),
        json!({"indices": "deces", "include_global_state": false}),
    )
    .await;
    assert_eq!(taken["snapshot"]["state"], json!("SUCCESS"));
    assert_eq!(taken["snapshot"]["surch"]["format"], json!("segments-v2"));
    assert!(taken["snapshot"]["surch"].get("reason").is_none());
    assert!(
        taken["snapshot"]["surch"]["files_uploaded"]
            .as_u64()
            .unwrap()
            >= 8
    );
    assert_eq!(taken["snapshot"]["surch"]["files_reused"], json!(0));
    drop(source);
    drop(source_app);

    // Production node with a fresh data directory.
    let target_dir = root.sub("target");
    let (router, app) = open(&target_dir);
    seed_previous_month(&router).await;

    // A readonly repository refuses snapshot creation (ES message).
    must(
        &router,
        Method::PUT,
        "/_snapshot/matchID",
        fs_repo_body(&repo, true),
    )
    .await;
    let (status, refused) = req(
        &router,
        Method::PUT,
        "/_snapshot/matchID/should-fail",
        json!({"indices": "deces-snap-2026-10"}),
    )
    .await;
    assert_eq!(status, 500, "{refused}");
    assert_eq!(refused["error"]["type"], json!("repository_exception"));
    assert_eq!(
        refused["error"]["reason"],
        json!("[matchID] cannot create snapshot in a readonly repository")
    );
    let (status, _) = req(
        &router,
        Method::DELETE,
        &format!("/_snapshot/matchID/{SNAPSHOT}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 500, "readonly repository refuses deletion");

    run_restore_job(&router, fs_repo_body(&repo, true)).await;

    // No document was re-analysed on the restoring node.
    assert_eq!(app.index_terms_build_count(TARGET), 0);
    assert!(
        app.index_segment_count(TARGET) >= 4,
        "segment layout restored as is"
    );
    assert!(!app.index_exists("deces-snap-2026-10"), "pruned");

    // Byte-identical searches through the swapped alias.
    let restored = record(&router, &calls).await;
    assert_same("restored", &calls, &expected, &restored);
    assert_eq!(app.index_terms_build_count(TARGET), 0);

    // Settings: analysis kept, creation_date kept (ES restore copies the
    // snapshot settings), number_of_replicas from `index_settings`.
    let target_meta = must(&router, Method::GET, &format!("/{TARGET}"), Value::Null).await;
    assert_eq!(
        creation_date(&target_meta, TARGET),
        creation_date(&source_meta, "deces")
    );
    assert!(creation_date(&target_meta, TARGET).is_string());
    assert_eq!(
        target_meta[TARGET]["settings"]["index"]["analysis"],
        source_meta["deces"]["settings"]["index"]["analysis"]
    );
    assert!(target_meta[TARGET]["settings"]["index"]["analysis"].is_object());
    let settings = must(
        &router,
        Method::GET,
        &format!("/{TARGET}/_settings"),
        Value::Null,
    )
    .await;
    assert_eq!(
        settings[TARGET]["settings"]["index"]["number_of_replicas"],
        json!(0)
    );

    // The marker is written with `PUT _doc` and no refresh. Surch has no
    // translog: it only survives a restart once a refresh committed it
    // (documented gap, `docs/ops/snapshot-v2.md`, recommended Job change:
    // `POST /.matchid-restore-state/_refresh` after `write_marker`).
    must(
        &router,
        Method::POST,
        "/.matchid-restore-state/_refresh",
        Value::Null,
    )
    .await;
    assert!(staging_dirs(&target_dir).is_empty(), "staging removed");

    // The restore is durable: restart the node, same answers.
    drop(router);
    drop(app);
    let (router, app) = open(&target_dir);
    let reopened = record(&router, &calls).await;
    assert_same("restored after restart", &calls, &expected, &reopened);
    assert_eq!(app.index_terms_build_count(TARGET), 0);
    let marker = must(
        &router,
        Method::GET,
        "/.matchid-restore-state/_doc/deces",
        Value::Null,
    )
    .await;
    assert_eq!(marker["_source"]["snapshot_name"], json!(SNAPSHOT));
}

/// `restore-*` / `snapshot-*` staging directories left under `tmp/`.
fn staging_dirs(data_dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(data_dir.join("tmp"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let name = p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    name.starts_with("restore-") || name.starts_with("snapshot-")
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn blake3_dedup_and_blob_gc_across_snapshots() {
    let root = TestDir::new("dedup");
    let repo = root.sub("repo");
    let (router, app) = open(&root.sub("data"));
    build_deces(&router, &app).await;
    must(
        &router,
        Method::PUT,
        "/_snapshot/r",
        fs_repo_body(&repo, false),
    )
    .await;

    let first = must(
        &router,
        Method::PUT,
        "/_snapshot/r/s1",
        json!({"indices": "deces"}),
    )
    .await;
    let first = &first["snapshot"]["surch"];
    let blobs_after_first = blob_count(&repo);
    assert_eq!(
        first["files_uploaded"].as_u64().unwrap() as usize,
        blobs_after_first
    );

    // Unchanged index: only the new MANIFEST generation is new content.
    let second = must(
        &router,
        Method::PUT,
        "/_snapshot/r/s2",
        json!({"indices": "deces"}),
    )
    .await;
    let second = &second["snapshot"]["surch"];
    assert!(
        second["files_uploaded"].as_u64().unwrap() <= 2,
        "unchanged index re-uploaded files: {second}"
    );
    assert!(
        second["bytes_uploaded"].as_u64().unwrap() * 20 < first["bytes_uploaded"].as_u64().unwrap(),
        "dedup: {second} vs {first}"
    );
    assert!(
        second["files_reused"].as_u64().unwrap() >= first["files_uploaded"].as_u64().unwrap() - 2
    );

    // New documents + refresh: the `source.dat` prefix chunk is reused,
    // only the appended chunk and the new segment files are uploaded.
    let mut extra = String::new();
    for i in 0..50 {
        extra.push_str(&format!(
            "{{\"index\":{{\"_index\":\"deces\",\"_id\":\"extra-{i}\"}}}}\n{{\"NOM\":\"NOUVEAU\",\"PRENOMS\":\"JEAN\",\"SEXE\":\"M\",\"SOURCE_LINE\":{}}}\n",
            2000 + i
        ));
    }
    bulk(&router, extra).await;
    must(&router, Method::POST, "/deces/_refresh", Value::Null).await;
    let third = must(
        &router,
        Method::PUT,
        "/_snapshot/r/s3",
        json!({"indices": "deces"}),
    )
    .await;
    let third = &third["snapshot"]["surch"];
    assert!(
        third["files_reused"].as_u64().unwrap() >= 1,
        "source prefix chunk reused: {third}"
    );

    // Deleting s1 keeps every blob s2 / s3 need; deleting all removes all.
    let before_delete = blob_count(&repo);
    must(&router, Method::DELETE, "/_snapshot/r/s1", Value::Null).await;
    assert!(blob_count(&repo) <= before_delete);
    let target_dir = root.sub("restore-s2");
    {
        let (restore_router, restore_app) = open(&target_dir);
        must(
            &restore_router,
            Method::PUT,
            "/_snapshot/r",
            fs_repo_body(&repo, true),
        )
        .await;
        must(
            &restore_router,
            Method::POST,
            "/_snapshot/r/s2/_restore?wait_for_completion=true",
            json!({"indices": "deces"}),
        )
        .await;
        assert_eq!(
            count(&must(&restore_router, Method::GET, "/deces/_count", Value::Null).await),
            1000
        );
        must(
            &restore_router,
            Method::POST,
            "/_snapshot/r/s3/_restore?wait_for_completion=true",
            json!({"indices": "deces", "rename_pattern": "(.+)", "rename_replacement": "$1-s3"}),
        )
        .await;
        assert_eq!(
            count(
                &must(
                    &restore_router,
                    Method::GET,
                    "/deces-s3/_count",
                    Value::Null
                )
                .await
            ),
            1050
        );
        assert_eq!(restore_app.index_terms_build_count("deces-s3"), 0);
    }
    must(&router, Method::DELETE, "/_snapshot/r/s2", Value::Null).await;
    must(&router, Method::DELETE, "/_snapshot/r/s3", Value::Null).await;
    assert_eq!(
        blob_count(&repo),
        0,
        "every blob collected once no snapshot is left"
    );
}

#[tokio::test]
async fn corrupt_blob_mid_restore_leaves_live_index_untouched() {
    let root = TestDir::new("crash");
    let repo = root.sub("repo");
    {
        let (router, app) = open(&root.sub("source"));
        build_deces(&router, &app).await;
        must(
            &router,
            Method::PUT,
            "/_snapshot/matchID",
            fs_repo_body(&repo, false),
        )
        .await;
        must(
            &router,
            Method::PUT,
            &format!("/_snapshot/matchID/{SNAPSHOT}"),
            json!({"indices": "deces"}),
        )
        .await;
    }
    // Bit rot in the largest blob (a postings or source file).
    let mut blobs = files_under(&repo.join("blobs"));
    blobs.sort_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    let victim = blobs.last().expect("blobs").clone();
    let mut bytes = std::fs::read(&victim).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x5a;
    std::fs::write(&victim, &bytes).unwrap();

    let target_dir = root.sub("target");
    let (router, app) = open(&target_dir);
    seed_previous_month(&router).await;
    must(
        &router,
        Method::PUT,
        "/_snapshot/matchID",
        fs_repo_body(&repo, true),
    )
    .await;
    let (status, error) = req(
        &router,
        Method::POST,
        &format!("/_snapshot/matchID/{SNAPSHOT}/_restore?wait_for_completion=true"),
        job_restore_body(TARGET),
    )
    .await;
    assert_eq!(status, 500, "{error}");
    let reason = error["error"]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("blake3"),
        "integrity error names the check: {reason}"
    );

    // Nothing of the failed restore is visible; the live alias still serves.
    assert!(!app.index_exists(TARGET));
    let (status, _) = req(
        &router,
        Method::GET,
        &format!("/{TARGET}/_count"),
        Value::Null,
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(
        count(&must(&router, Method::GET, "/deces/_count", Value::Null).await),
        1
    );
    assert!(staging_dirs(&target_dir).is_empty(), "staging removed");
    let index_dirs: Vec<String> = std::fs::read_dir(target_dir.join("indices"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !index_dirs.iter().any(|d| d.contains("2026-11")),
        "no index directory for the failed target: {index_dirs:?}"
    );

    // A process crash mid-download leaves a staging directory behind: the
    // next start sweeps it and the target index does not exist.
    let staging = target_dir
        .join("tmp")
        .join("restore-1-2-3")
        .join(format!("i-{TARGET}"));
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("seg-00000001.pst"), b"partial download").unwrap();
    drop(router);
    drop(app);
    let (router, app) = open(&target_dir);
    assert!(
        staging_dirs(&target_dir).is_empty(),
        "staging swept at start"
    );
    assert!(!app.index_exists(TARGET));
    assert_eq!(
        count(&must(&router, Method::GET, "/deces/_count", Value::Null).await),
        1
    );
}

#[tokio::test]
async fn restore_validation_matches_es() {
    let root = TestDir::new("validate");
    let repo = root.sub("repo");
    let (router, app) = open(&root.sub("data"));
    build_deces(&router, &app).await;
    must(
        &router,
        Method::PUT,
        "/_snapshot/r",
        fs_repo_body(&repo, false),
    )
    .await;
    must(
        &router,
        Method::PUT,
        "/_snapshot/r/s",
        json!({"indices": "deces"}),
    )
    .await;
    let restore = "/_snapshot/r/s/_restore?wait_for_completion=true";

    // Existing index without rename.
    let (status, body) = req(&router, Method::POST, restore, json!({"indices": "deces"})).await;
    assert_eq!(status, 500);
    assert_eq!(body["error"]["type"], json!("snapshot_restore_exception"));
    assert!(body["error"]["reason"]
        .as_str()
        .unwrap()
        .contains("an open index with same name already exists"));
    // Unmodifiable setting.
    let (status, body) = req(
        &router,
        Method::POST,
        restore,
        json!({"indices": "deces", "rename_pattern": "deces", "rename_replacement": "x",
               "index_settings": {"index.number_of_shards": 2}}),
    )
    .await;
    assert_eq!(status, 500);
    assert!(body["error"]["reason"]
        .as_str()
        .unwrap()
        .ends_with("cannot modify setting [index.number_of_shards] on restore"));
    // Missing index, then the same with ignore_unavailable.
    let (status, body) = req(&router, Method::POST, restore, json!({"indices": "nope"})).await;
    assert_eq!(status, 404);
    assert_eq!(body["error"]["reason"], json!("no such index [nope]"));
    let (status, body) = req(
        &router,
        Method::POST,
        restore,
        json!({"indices": "nope", "ignore_unavailable": true}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["snapshot"]["indices"], json!([]));
    // Rename onto an alias name.
    must(
        &router,
        Method::POST,
        "/_aliases",
        json!({"actions": [{"add": {"index": "deces", "alias": "taken"}}]}),
    )
    .await;
    let (status, body) = req(
        &router,
        Method::POST,
        restore,
        json!({"rename_pattern": "deces", "rename_replacement": "taken"}),
    )
    .await;
    assert_eq!(status, 500);
    assert!(body["error"]["reason"]
        .as_str()
        .unwrap()
        .contains("conflict with an alias with the same name"));
    // Invalid Java group reference.
    let (status, body) = req(
        &router,
        Method::POST,
        restore,
        json!({"rename_pattern": "deces", "rename_replacement": "$1"}),
    )
    .await;
    assert_eq!(status, 500);
    assert!(body["error"]["reason"]
        .as_str()
        .unwrap()
        .contains("No group 1"));
    // include_aliases=false drops the snapshot aliases.
    must(
        &router,
        Method::PUT,
        "/_snapshot/r/s-alias",
        json!({"indices": "deces"}),
    )
    .await;
    must(
        &router,
        Method::POST,
        "/_snapshot/r/s-alias/_restore?wait_for_completion=true",
        json!({"rename_pattern": "^deces$", "rename_replacement": "copy", "include_aliases": false}),
    )
    .await;
    assert!(app.aliases_for_index("copy").is_empty());
    // wait_for_completion=false: accepted, then the index appears.
    let accepted = must(
        &router,
        Method::POST,
        "/_snapshot/r/s-alias/_restore?wait_for_completion=false",
        json!({"rename_pattern": "^deces$", "rename_replacement": "async-copy"}),
    )
    .await;
    assert_eq!(accepted, json!({"accepted": true}));
    let health = must(
        &router,
        Method::GET,
        "/_cluster/health/async-copy?wait_for_status=green&timeout=60s",
        Value::Null,
    )
    .await;
    assert_eq!(health["status"], json!("green"));
    assert_eq!(
        count(&must(&router, Method::GET, "/async-copy/_count", Value::Null).await),
        1000
    );
    assert_eq!(
        app.aliases_for_index("async-copy"),
        vec!["taken".to_owned()]
    );
}

#[tokio::test]
async fn logical_v1_still_restores_with_rename_analysis_and_creation_date() {
    let root = TestDir::new("v1");
    let repo = root.sub("repo");
    let (source, _source_app) = open_in_memory();
    let mapping: Value = serde_json::from_str(MAPPING_V2).unwrap();
    must(&source, Method::PUT, "/deces", mapping).await;
    for chunk in slice_chunks(2) {
        bulk(&source, chunk).await;
    }
    must(&source, Method::POST, "/deces/_refresh", Value::Null).await;
    let calls = battery();
    let expected = record(&source, &calls).await;
    let source_meta = must(&source, Method::GET, "/deces", Value::Null).await;
    must(
        &source,
        Method::PUT,
        "/_snapshot/r",
        fs_repo_body(&repo, false),
    )
    .await;
    let taken = must(
        &source,
        Method::PUT,
        "/_snapshot/r/old",
        json!({"indices": "deces"}),
    )
    .await;
    assert_eq!(taken["snapshot"]["surch"]["format"], json!("logical-v1"));
    assert!(taken["snapshot"]["surch"]["reason"]
        .as_str()
        .unwrap()
        .contains("SURCH_DATA_DIR"));

    let target_dir = root.sub("target");
    {
        let (router, _app) = open(&target_dir);
        must(
            &router,
            Method::PUT,
            "/_snapshot/r",
            fs_repo_body(&repo, true),
        )
        .await;
        let restored = must(
            &router,
            Method::POST,
            "/_snapshot/r/old/_restore?wait_for_completion=true",
            job_restore_body(TARGET),
        )
        .await;
        assert_eq!(restored["snapshot"]["indices"], json!([TARGET]));
        must(
            &router,
            Method::POST,
            "/_aliases",
            json!({"actions": [{"add": {"index": TARGET, "alias": "deces"}}]}),
        )
        .await;
    }
    // Re-ingested AND committed: survives a restart.
    let (router, _app) = open(&target_dir);
    let restored = record(&router, &calls).await;
    assert_same("v1 restored", &calls, &expected, &restored);
    let target_meta = must(&router, Method::GET, &format!("/{TARGET}"), Value::Null).await;
    assert_eq!(
        creation_date(&target_meta, TARGET),
        creation_date(&source_meta, "deces")
    );
    assert_eq!(
        target_meta[TARGET]["settings"]["index"]["number_of_replicas"],
        json!(0)
    );
}

#[tokio::test]
async fn segment_snapshot_restore_without_data_dir_fails_closed() {
    let root = TestDir::new("nodata");
    let repo = root.sub("repo");
    {
        let (router, app) = open(&root.sub("source"));
        build_deces(&router, &app).await;
        must(
            &router,
            Method::PUT,
            "/_snapshot/r",
            fs_repo_body(&repo, false),
        )
        .await;
        must(
            &router,
            Method::PUT,
            "/_snapshot/r/s",
            json!({"indices": "deces"}),
        )
        .await;
    }
    let (router, app) = open_in_memory();
    must(
        &router,
        Method::PUT,
        "/_snapshot/r",
        fs_repo_body(&repo, true),
    )
    .await;
    let (status, body) = req(
        &router,
        Method::POST,
        "/_snapshot/r/s/_restore?wait_for_completion=true",
        json!({"indices": "deces"}),
    )
    .await;
    assert_eq!(status, 500, "{body}");
    assert!(body["error"]["reason"]
        .as_str()
        .unwrap()
        .contains("requires SURCH_DATA_DIR"));
    assert!(!app.index_exists("deces"));
}
