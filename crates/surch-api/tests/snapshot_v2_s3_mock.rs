//! L6.2 — the S3 wire path of segment snapshots, against an in-process S3
//! mock (no Docker, no network beyond 127.0.0.1; the MinIO e2e stays in
//! `snapshot_s3.rs` behind `SURCH_MINIO_E2E=1`).
//!
//! The mock implements the subset `S3Repository` speaks — path-style
//! `PutObject` / `GetObject` / `HeadObject` / `DeleteObject`,
//! `ListObjectsV2`, multipart `Create` / `UploadPart` / `Complete` /
//! `Abort` — and records every request, so the test can check what went on
//! the wire: path-style addressing on the `protocol://endpoint` URL built
//! from the matchID Job settings, SigV4 with the access key taken from
//! `SURCH_S3_CLIENT_DEFAULT_ACCESS_KEY` (ES keystore stand-in), multipart
//! above `buffer_size`, no re-upload of a blob already present, readonly
//! refusal before any write.
//!
//! Single test function on purpose: it sets process environment variables.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::{
    body::{to_bytes, Body, Bytes},
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use serde_json::{json, Value};
use surch_api::{
    app_router_with_state,
    snapshot_es::{S3Repository, S3RepositoryConfig, SnapshotRepository},
    state::AppState,
    AppRouterState,
};
use tower::ServiceExt;

const BUCKET: &str = "fichier-des-personnes-decedees-elasticsearch";
const ACCESS_KEY: &str = "SCWMOCKACCESSKEY0001";
const MAPPING_V2: &str = include_str!("../../../tests/matchid_compat/deces/mapping_v2.json");
const SLICE_NDJSON: &str = include_str!("../../../tests/matchid_compat/deces/slice-1000.ndjson");

#[derive(Default)]
struct MockS3 {
    objects: BTreeMap<String, Vec<u8>>,
    uploads: HashMap<String, (String, BTreeMap<i32, Vec<u8>>)>,
    next_upload: u64,
    /// `METHOD path?query` of every request, in order.
    log: Vec<String>,
    /// Requests whose `Authorization` header did not carry ACCESS_KEY.
    unsigned: Vec<String>,
}

type Shared = Arc<Mutex<MockS3>>;

fn etag(bytes: &[u8]) -> String {
    format!("\"{}\"", &blake3::hash(bytes).to_hex()[..32])
}

fn xml(status: StatusCode, body: String) -> Response {
    (status, [("content-type", "application/xml")], body).into_response()
}

fn no_such_key(key: &str) -> Response {
    xml(
        StatusCode::NOT_FOUND,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>NoSuchKey</Code>\
             <Message>The specified key does not exist.</Message><Key>{key}</Key></Error>"
        ),
    )
}

fn query_map(uri: &Uri) -> HashMap<String, String> {
    uri.query()
        .map(|q| {
            url_decode_pairs(q)
                .into_iter()
                .collect::<HashMap<String, String>>()
        })
        .unwrap_or_default()
}

fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(hex, 16).unwrap());
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
}

fn url_decode_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(pair), String::new()),
        })
        .collect()
}

async fn handle(
    State(shared): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s3 = shared.lock().unwrap();
    let line = format!("{method} {}", uri);
    if std::env::var_os("S3_MOCK_TRACE").is_some() {
        eprintln!("mock-s3: {line} ({} body bytes)", body.len());
    }
    let signed = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains(&format!("Credential={ACCESS_KEY}/")));
    if !signed {
        s3.unsigned.push(line.clone());
    }
    s3.log.push(line);
    let path = url_decode(uri.path());
    let Some(rest) = path.strip_prefix(&format!("/{BUCKET}")) else {
        return xml(
            StatusCode::NOT_FOUND,
            "<Error><Code>NoSuchBucket</Code></Error>".to_owned(),
        );
    };
    let key = rest.trim_start_matches('/').to_owned();
    let query = query_map(&uri);

    if key.is_empty() {
        // ListObjectsV2
        let prefix = query.get("prefix").cloned().unwrap_or_default();
        let mut contents = String::new();
        let mut count = 0;
        for (name, bytes) in s3.objects.range(prefix.clone()..) {
            if !name.starts_with(&prefix) {
                break;
            }
            count += 1;
            contents.push_str(&format!(
                "<Contents><Key>{name}</Key><LastModified>2026-11-15T00:00:00.000Z</LastModified>\
                 <ETag>{}</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                etag(bytes).replace('"', "&quot;"),
                bytes.len()
            ));
        }
        return xml(
            StatusCode::OK,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                 <Name>{BUCKET}</Name><Prefix>{prefix}</Prefix><KeyCount>{count}</KeyCount>\
                 <MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
            ),
        );
    }

    match method {
        Method::POST if query.contains_key("uploads") => {
            s3.next_upload += 1;
            let id = format!("upload-{}", s3.next_upload);
            s3.uploads
                .insert(id.clone(), (key.clone(), BTreeMap::new()));
            xml(
                StatusCode::OK,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><InitiateMultipartUploadResult>\
                     <Bucket>{BUCKET}</Bucket><Key>{key}</Key><UploadId>{id}</UploadId>\
                     </InitiateMultipartUploadResult>"
                ),
            )
        }
        Method::POST if query.contains_key("uploadId") => {
            let id = &query["uploadId"];
            let Some((upload_key, parts)) = s3.uploads.remove(id) else {
                return xml(
                    StatusCode::NOT_FOUND,
                    "<Error><Code>NoSuchUpload</Code></Error>".to_owned(),
                );
            };
            assert_eq!(upload_key, key);
            let joined: Vec<u8> = parts.into_values().flatten().collect();
            let tag = etag(&joined);
            s3.objects.insert(key.clone(), joined);
            xml(
                StatusCode::OK,
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><CompleteMultipartUploadResult>\
                     <Location>http://mock/{BUCKET}/{key}</Location><Bucket>{BUCKET}</Bucket>\
                     <Key>{key}</Key><ETag>{}</ETag></CompleteMultipartUploadResult>",
                    tag.replace('"', "&quot;")
                ),
            )
        }
        Method::PUT if query.contains_key("uploadId") => {
            let id = query["uploadId"].clone();
            let number: i32 = query["partNumber"].parse().unwrap();
            let tag = etag(&body);
            match s3.uploads.get_mut(&id) {
                Some((_, parts)) => {
                    parts.insert(number, body.to_vec());
                    (StatusCode::OK, [("etag", tag)]).into_response()
                }
                None => xml(
                    StatusCode::NOT_FOUND,
                    "<Error><Code>NoSuchUpload</Code></Error>".to_owned(),
                ),
            }
        }
        Method::DELETE if query.contains_key("uploadId") => {
            s3.uploads.remove(&query["uploadId"]);
            StatusCode::NO_CONTENT.into_response()
        }
        Method::PUT => {
            let tag = etag(&body);
            s3.objects.insert(key, body.to_vec());
            (StatusCode::OK, [("etag", tag)]).into_response()
        }
        Method::GET => match s3.objects.get(&key) {
            Some(bytes) => (
                StatusCode::OK,
                [
                    ("etag", etag(bytes)),
                    ("content-type", "application/octet-stream".to_owned()),
                ],
                bytes.clone(),
            )
                .into_response(),
            None => no_such_key(&key),
        },
        Method::HEAD => match s3.objects.get(&key) {
            Some(bytes) => (
                StatusCode::OK,
                [
                    ("etag", etag(bytes)),
                    ("content-length", bytes.len().to_string()),
                ],
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        Method::DELETE => {
            s3.objects.remove(&key);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

async fn start_mock() -> (u16, Shared) {
    let shared: Shared = Arc::default();
    let app = Router::new()
        .fallback(handle)
        // Multipart parts are 5 MiB: lift the axum 2 MiB default limit.
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(Arc::clone(&shared));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, shared)
}

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
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &std::path::Path) -> Router {
    let app = AppState::open_with_data_dir(dir).expect("data directory opens");
    app_router_with_state(AppRouterState {
        app,
        ..Default::default()
    })
}

async fn req(
    router: &Router,
    method: Method,
    path: &str,
    body: Value,
    ndjson: bool,
) -> (u16, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("accept", "*/*");
    let body = if body.is_null() {
        String::new()
    } else if ndjson {
        body.as_str().unwrap().to_owned()
    } else {
        body.to_string()
    };
    if !body.is_empty() {
        builder = builder.header(
            "content-type",
            if ndjson {
                "application/x-ndjson"
            } else {
                "application/json"
            },
        );
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

async fn must(router: &Router, method: Method, path: &str, body: Value) -> Value {
    let (status, value) = req(router, method.clone(), path, body, false).await;
    assert!(
        (200..300).contains(&status),
        "{method} {path} -> {status}: {value}"
    );
    value
}

/// The repository body of the prod Job, pointed at the mock (`endpoint` /
/// `protocol` are the only differences).
fn job_repository(port: u16, readonly: bool) -> Value {
    json!({
        "type": "s3",
        "settings": {
            "bucket": BUCKET,
            "client": "default",
            "region": "fr-par",
            "endpoint": format!("127.0.0.1:{port}"),
            "path_style_access": true,
            "protocol": "http",
            "readonly": readonly
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn segment_snapshot_round_trip_through_s3_wire() {
    // ES keystore stand-in for `s3.client.default.*`.
    std::env::set_var("SURCH_S3_CLIENT_DEFAULT_ACCESS_KEY", ACCESS_KEY);
    std::env::set_var("SURCH_S3_CLIENT_DEFAULT_SECRET_KEY", "mock-secret");
    let (port, mock) = start_mock().await;
    let root = TestDir::new("s3mock");

    // 1. Offline build + snapshot to the (writable) bucket.
    let source = open(&root.0.join("source"));
    let mapping: Value = serde_json::from_str(MAPPING_V2).unwrap();
    must(&source, Method::PUT, "/deces", mapping).await;
    let (status, bulk) = req(
        &source,
        Method::POST,
        "/_bulk",
        Value::String(SLICE_NDJSON.to_owned()),
        true,
    )
    .await;
    assert_eq!(status, 200, "{bulk}");
    must(&source, Method::POST, "/deces/_refresh", Value::Null).await;
    let probe = json!({"query": {"match": {"NOM": "DUPONT"}}, "size": 5});
    let expected = must(&source, Method::POST, "/deces/_search", probe.clone()).await;
    must(
        &source,
        Method::PUT,
        "/_snapshot/matchID",
        job_repository(port, false),
    )
    .await;
    let repo = must(&source, Method::GET, "/_snapshot/matchID", Value::Null).await;
    assert_eq!(
        repo["matchID"]["settings"]["path_style_access"],
        json!("true")
    );
    assert_eq!(
        repo["matchID"]["settings"]["endpoint"],
        json!(format!("127.0.0.1:{port}"))
    );
    let taken = must(
        &source,
        Method::PUT,
        "/_snapshot/matchID/snap-2026-11?wait_for_completion=true",
        json!({"indices": "deces", "include_global_state": false}),
    )
    .await;
    assert_eq!(taken["snapshot"]["surch"]["format"], json!("segments-v2"));
    let uploaded = taken["snapshot"]["surch"]["files_uploaded"]
        .as_u64()
        .unwrap();
    assert!(uploaded >= 5, "{taken}");
    {
        let s3 = mock.lock().unwrap();
        assert!(
            s3.unsigned.is_empty(),
            "unsigned requests: {:?}",
            s3.unsigned
        );
        assert!(
            s3.log.iter().all(|l| l.contains(&format!(" /{BUCKET}"))),
            "path-style addressing on the mock endpoint: {:?}",
            &s3.log[..3]
        );
        let blob_puts = s3
            .log
            .iter()
            .filter(|l| l.starts_with("PUT ") && l.contains("/blobs/"))
            .count() as u64;
        assert_eq!(blob_puts, uploaded);
    }
    // Same content again: only HEADs for the existing blobs, 1-2 PUTs.
    let log_before = mock.lock().unwrap().log.len();
    let again = must(
        &source,
        Method::PUT,
        "/_snapshot/matchID/snap-2026-11-bis",
        json!({"indices": "deces"}),
    )
    .await;
    assert!(
        again["snapshot"]["surch"]["files_uploaded"]
            .as_u64()
            .unwrap()
            <= 2
    );
    {
        let s3 = mock.lock().unwrap();
        let blob_puts = s3.log[log_before..]
            .iter()
            .filter(|l| l.starts_with("PUT ") && l.contains("/blobs/"))
            .count();
        assert!(
            blob_puts <= 2,
            "dedup on the wire: {:?}",
            &s3.log[log_before..]
        );
    }
    drop(source);

    // 2. Production node: readonly repository, the Job restore body.
    let target = open(&root.0.join("target"));
    must(
        &target,
        Method::PUT,
        "/_snapshot/matchID",
        job_repository(port, true),
    )
    .await;
    let log_before = mock.lock().unwrap().log.len();
    let (status, refused) = req(
        &target,
        Method::PUT,
        "/_snapshot/matchID/nope",
        json!({}),
        false,
    )
    .await;
    assert_eq!(status, 500);
    assert_eq!(
        refused["error"]["reason"],
        json!("[matchID] cannot create snapshot in a readonly repository")
    );
    assert_eq!(
        mock.lock().unwrap().log.len(),
        log_before,
        "readonly refusal happens before any S3 request"
    );
    let restored = must(
        &target,
        Method::POST,
        "/_snapshot/matchID/snap-2026-11/_restore?wait_for_completion=true",
        json!({
            "indices": "deces",
            "ignore_unavailable": true,
            "include_global_state": false,
            "rename_pattern": "^deces$",
            "rename_replacement": "deces-snap-2026-11",
            "index_settings": {"index.number_of_replicas": 0}
        }),
    )
    .await;
    assert_eq!(
        restored["snapshot"]["indices"],
        json!(["deces-snap-2026-11"])
    );
    let count = must(
        &target,
        Method::GET,
        "/deces-snap-2026-11/_count",
        Value::Null,
    )
    .await;
    assert_eq!(count["count"], json!(1000));
    let got = must(&target, Method::POST, "/deces-snap-2026-11/_search", probe).await;
    let normalize = |mut v: Value| {
        v["took"] = json!(0);
        v.to_string().replace("deces-snap-2026-11", "deces")
    };
    assert_eq!(normalize(got), normalize(expected));
    assert!(mock.lock().unwrap().unsigned.is_empty());

    // 3. Multipart above `buffer_size` (5 MiB, the ES floor), streamed both
    //    ways through the same client code.
    let big = root.0.join("big.bin");
    let data: Vec<u8> = (0..12 * 1024 * 1024u32)
        .map(|i| (i * 7 % 253) as u8)
        .collect();
    std::fs::write(&big, &data).unwrap();
    let config = S3RepositoryConfig {
        bucket: BUCKET.to_owned(),
        region: Some("fr-par".to_owned()),
        endpoint: Some(format!("127.0.0.1:{port}")),
        protocol: Some("http".to_owned()),
        path_style_access: Some(true),
        client: Some("default".to_owned()),
        disable_request_checksum: true,
        buffer_size: 5 * 1024 * 1024,
        ..Default::default()
    };
    let log_before = mock.lock().unwrap().log.len();
    let (round_trip, len) = tokio::task::spawn_blocking(move || {
        let repo = S3Repository::new(config).unwrap();
        repo.put_file_range("blobs/big", &big, 0, data.len() as u64)
            .unwrap();
        let mut out = Vec::new();
        let n = repo.get_object_to_writer("blobs/big", &mut out).unwrap();
        (out == data, n)
    })
    .await
    .unwrap();
    assert!(round_trip);
    assert_eq!(len, 12 * 1024 * 1024);
    let s3 = mock.lock().unwrap();
    let parts = s3.log[log_before..]
        .iter()
        .filter(|l| l.starts_with("PUT ") && l.contains("partNumber="))
        .count();
    assert_eq!(
        parts,
        3,
        "12 MiB in 5 MiB parts: {:?}",
        &s3.log[log_before..]
    );
    assert!(s3.uploads.is_empty(), "multipart completed");
}
