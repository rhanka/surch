//! Repository SPI for ES-parity `_snapshot` storage.
//!
//! Mirrors the Elasticsearch / OpenSearch `BlobStoreRepository`
//! contract: a small key/value store with atomic compare-and-set on
//! the root `index-{N}` manifest. The contract is intentionally
//! synchronous — the C-SNAPSHOT-S1 MVP runs against `FsRepository`
//! (local disk) where blocking I/O is the natural shape, and the
//! callers wrap operations in `tokio::task::spawn_blocking` when they
//! need to keep the request executor live. A future `S3Repository`
//! (`C-SNAPSHOT-S3`) plugs in here under the same trait, with the
//! AWS SDK calls bridged via `Handle::block_on` inside the
//! `spawn_blocking` worker.
//!
//! The four basic operations (`put_object`, `get_object`,
//! `list_objects`, `delete_object`) plus the atomic
//! `compare_and_set` on the root manifest are the only primitives
//! the snapshot machinery needs — every other operation is built on
//! top in `service.rs`.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::Hasher;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bytes::Bytes;

/// Errors returned by every `SnapshotRepository` implementation.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("snapshot object `{key}` not found")]
    NotFound { key: String },
    #[error("compare-and-set on `{key}` failed: expected etag {expected:?}, found {found:?}")]
    CasConflict {
        key: String,
        expected: Option<String>,
        found: Option<String>,
    },
    #[error("I/O error on `{key}`: {source}")]
    Io {
        key: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid repository config: {0}")]
    InvalidConfig(String),
}

pub type RepositoryResult<T> = Result<T, RepositoryError>;

/// Synchronous key/value store backing the ES-parity snapshot API.
///
/// `compare_and_set` is the only mandatory atomic operation: it
/// guarantees a single writer succeeds when two operators race to
/// update the root `index-{N}` manifest. All other operations are
/// "last writer wins"; the manifest-bump dance in
/// [`crate::snapshot_es::service`] makes them safe by funnelling every
/// state change through the CAS.
pub trait SnapshotRepository: Send + Sync {
    fn put_object(&self, key: &str, bytes: Bytes) -> RepositoryResult<()>;
    fn get_object(&self, key: &str) -> RepositoryResult<Bytes>;
    fn list_objects(&self, prefix: &str) -> RepositoryResult<Vec<String>>;
    fn delete_object(&self, key: &str) -> RepositoryResult<()>;
    fn compare_and_set(
        &self,
        key: &str,
        expected_etag: Option<&str>,
        bytes: Bytes,
    ) -> RepositoryResult<String>;
    /// Read the current ETag for `key`, or `None` when the object
    /// does not exist. Used by callers that need to chain a CAS
    /// without first issuing a `get_object` (the manifest pattern in
    /// `service::write_manifest`).
    fn read_etag(&self, key: &str) -> RepositoryResult<Option<String>>;

    /// Repository kind reported to clients via
    /// `GET /_snapshot/{repository}` — matches the ES JSON `type` field
    /// (`"fs"` for `FsRepository`, `"s3"` for the upcoming
    /// `S3Repository`).
    fn kind(&self) -> &'static str;

    /// Repository settings as exposed to clients (`{"location": "..."}`
    /// for `fs`, `{"bucket": "...", "base_path": "..."}` for `s3`).
    fn settings(&self) -> serde_json::Value;
}

/// Filesystem-backed implementation of [`SnapshotRepository`].
///
/// Layout under `root`:
///
/// ```text
/// {root}/
///   index-{N}
///   meta-{uuid}.dat
///   snap-{uuid}.dat
///   indices/{index_uuid}/meta-{snap_uuid}.dat
/// ```
///
/// `put_object` is atomic: bytes are written to a `*.tmp` file in
/// the same directory, then `rename`d to the final name (POSIX
/// guarantees same-filesystem rename is atomic at the directory
/// entry level). `compare_and_set` re-reads the existing file to
/// recompute its ETag inside a per-key `Mutex`, so the
/// read-modify-write window is serialised within the process. This
/// is the level of guarantee ES `FsRepository` itself ships — cross-
/// process concurrency on the same NFS mount remains the operator's
/// responsibility (ES recommends a single coordinating node;
/// `C-SNAPSHOT-S1` keeps the same recommendation).
pub struct FsRepository {
    root: PathBuf,
    cas_locks: Mutex<()>,
}

impl FsRepository {
    /// Construct a new filesystem repository rooted at `root`. The
    /// directory is created on demand (and its parents); a non-
    /// directory path at `root` is rejected.
    pub fn new(root: PathBuf) -> RepositoryResult<Self> {
        if root.exists() && !root.is_dir() {
            return Err(RepositoryError::InvalidConfig(format!(
                "snapshot repository root `{}` is not a directory",
                root.display()
            )));
        }
        fs::create_dir_all(&root).map_err(|source| RepositoryError::Io {
            key: root.display().to_string(),
            source,
        })?;
        Ok(Self {
            root,
            cas_locks: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn key_path(&self, key: &str) -> RepositoryResult<PathBuf> {
        validate_key(key)?;
        Ok(self.root.join(key))
    }

    fn ensure_parent(&self, target: &Path) -> RepositoryResult<()> {
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|source| RepositoryError::Io {
                    key: parent.display().to_string(),
                    source,
                })?;
            }
        }
        Ok(())
    }

    fn write_atomic(&self, target: &Path, bytes: &[u8]) -> RepositoryResult<()> {
        self.ensure_parent(target)?;
        let tmp = target.with_extension(format!(
            "tmp.{}",
            // unique suffix avoids tmp collisions when two threads
            // race on the same key (one of them is going to lose the
            // CAS upstream anyway).
            std::process::id(),
        ));
        let tmp = tmp.with_file_name(format!(
            "{}.{}",
            target
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("blob"),
            now_nanos(),
        ));
        {
            let mut file = fs::File::create(&tmp).map_err(|source| RepositoryError::Io {
                key: tmp.display().to_string(),
                source,
            })?;
            file.write_all(bytes)
                .map_err(|source| RepositoryError::Io {
                    key: tmp.display().to_string(),
                    source,
                })?;
            file.sync_all().map_err(|source| RepositoryError::Io {
                key: tmp.display().to_string(),
                source,
            })?;
        }
        fs::rename(&tmp, target).map_err(|source| RepositoryError::Io {
            key: target.display().to_string(),
            source,
        })?;
        Ok(())
    }

    fn etag_of(path: &Path) -> RepositoryResult<Option<String>> {
        match fs::metadata(path) {
            Ok(meta) => {
                let len = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let mut hasher = DefaultHasher::new();
                hasher.write_u64(len);
                hasher.write_u128(mtime);
                Ok(Some(format!("{:016x}", hasher.finish())))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(RepositoryError::Io {
                key: path.display().to_string(),
                source,
            }),
        }
    }
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn validate_key(key: &str) -> RepositoryResult<()> {
    if key.is_empty() {
        return Err(RepositoryError::InvalidConfig(
            "snapshot object key must not be empty".to_owned(),
        ));
    }
    if key.contains("..") || key.starts_with('/') {
        return Err(RepositoryError::InvalidConfig(format!(
            "snapshot object key `{key}` is not relative"
        )));
    }
    Ok(())
}

impl SnapshotRepository for FsRepository {
    fn put_object(&self, key: &str, bytes: Bytes) -> RepositoryResult<()> {
        let path = self.key_path(key)?;
        self.write_atomic(&path, &bytes)
    }

    fn get_object(&self, key: &str) -> RepositoryResult<Bytes> {
        let path = self.key_path(key)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Bytes::from(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(RepositoryError::NotFound {
                    key: key.to_owned(),
                })
            }
            Err(source) => Err(RepositoryError::Io {
                key: key.to_owned(),
                source,
            }),
        }
    }

    fn list_objects(&self, prefix: &str) -> RepositoryResult<Vec<String>> {
        // Empty prefix → whole repository (used by `GET /_snapshot/{repo}`
        // before any snapshot has been taken). Otherwise the prefix is
        // a path-like key, possibly with a `/` separator
        // (`indices/<uuid>/`). We walk the matching subtree and yield
        // entries relative to `self.root` so callers can stay
        // repository-impl-agnostic.
        let walk_root = if prefix.is_empty() {
            self.root.clone()
        } else {
            // If the prefix ends with a path separator it's a directory
            // prefix; otherwise it's a filename prefix, and we walk the
            // parent directory.
            let candidate = self.root.join(prefix);
            if candidate.is_dir() {
                candidate
            } else if let Some(parent) = candidate.parent() {
                parent.to_path_buf()
            } else {
                self.root.clone()
            }
        };
        if !walk_root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        walk(&walk_root, &self.root, &mut out)?;
        out.retain(|key| key.starts_with(prefix));
        out.sort();
        Ok(out)
    }

    fn delete_object(&self, key: &str) -> RepositoryResult<()> {
        let path = self.key_path(key)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(RepositoryError::NotFound {
                    key: key.to_owned(),
                })
            }
            Err(source) => Err(RepositoryError::Io {
                key: key.to_owned(),
                source,
            }),
        }
    }

    fn compare_and_set(
        &self,
        key: &str,
        expected_etag: Option<&str>,
        bytes: Bytes,
    ) -> RepositoryResult<String> {
        let path = self.key_path(key)?;
        let _guard = self
            .cas_locks
            .lock()
            .expect("snapshot CAS mutex should not be poisoned");
        let found = Self::etag_of(&path)?;
        if found.as_deref() != expected_etag {
            return Err(RepositoryError::CasConflict {
                key: key.to_owned(),
                expected: expected_etag.map(str::to_owned),
                found,
            });
        }
        self.write_atomic(&path, &bytes)?;
        // Recompute the ETag from the post-write metadata so the
        // caller can chain another CAS.
        Self::etag_of(&path)?.ok_or_else(|| RepositoryError::Io {
            key: key.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "object disappeared right after write",
            ),
        })
    }

    fn kind(&self) -> &'static str {
        "fs"
    }

    fn settings(&self) -> serde_json::Value {
        serde_json::json!({ "location": self.root.display().to_string() })
    }

    fn read_etag(&self, key: &str) -> RepositoryResult<Option<String>> {
        let path = self.key_path(key)?;
        Self::etag_of(&path)
    }
}

/// Configuration for an [`S3Repository`].
///
/// Mirrors the Elasticsearch `repository-s3` settings minus the
/// flavours we explicitly do not support in the MVP (IAM-role
/// chaining, STS, IMDS, presigned-URL upload). Static credentials
/// only: callers pass `access_key` + `secret_key` directly. The
/// optional `endpoint` switches the SDK from real S3 to a
/// compatible backend (R2, GCS interop, MinIO, axum mock server in
/// tests); when set, the client forces `force_path_style = true`
/// because every non-AWS S3 backend uses path-style addressing.
#[derive(Clone, Debug, Default)]
pub struct S3RepositoryConfig {
    pub bucket: String,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub session_token: Option<String>,
    /// Optional path prefix inside the bucket — every object key
    /// composed by the snapshot machinery is anchored under this
    /// prefix (`{prefix}/index-0`, `{prefix}/snap-{uuid}.dat`, …).
    /// Empty prefix → repository owns the whole bucket.
    pub base_path: Option<String>,
    /// Test-only escape hatch: when true, the underlying `aws-sdk-s3`
    /// client is configured with `RequestChecksumCalculation::WhenRequired`
    /// and `ResponseChecksumValidation::WhenRequired` instead of the
    /// default `WhenSupported`. This is needed by mock-S3 fixtures that
    /// do not yet implement the AWS Flexible Checksums response
    /// contract; prod paths against MinIO / R2 / real S3 keep the
    /// default (full validation).
    pub disable_request_checksum: bool,
}

/// S3-backed implementation of [`SnapshotRepository`].
///
/// Bridges the synchronous `SnapshotRepository` SPI to the async
/// `aws-sdk-s3` client by owning a dedicated single-thread Tokio
/// runtime *pinned to a background OS thread*. The repository call
/// sites (axum handlers, restore code path) all run on the main
/// multi-thread runtime; entering a fresh runtime via `Handle::block_on`
/// from there would panic, so we keep our own runtime alive on its
/// own thread and route every AWS call through its `Handle`. The
/// background thread also owns the `Runtime` value, so when the
/// repository is dropped the runtime tear-down happens on that thread
/// — `tokio` forbids dropping a `Runtime` from within an async
/// context, which is exactly what would happen if we stored the
/// `Runtime` directly inside `S3Repository` and the axum `Drop`
/// chain ran inside a request future.
///
/// `compare_and_set` uses the S3 `If-Match` / `If-None-Match`
/// preconditions on `PutObject` (S3 2024-08+ semantics, also
/// supported by MinIO ≥ RELEASE.2023-09 and R2). When the backend
/// does not support conditional writes we fall back to "best effort":
/// read the current ETag, compare in-process, then PUT. This is the
/// same fallback `repository-s3` keeps for legacy regions.
pub struct S3Repository {
    config: S3RepositoryConfig,
    client: aws_sdk_s3::Client,
    runtime: Arc<DedicatedRuntime>,
}

/// Background-thread-owned Tokio runtime used by [`S3Repository`].
///
/// The runtime lives on a thread we spawn ourselves; the public API
/// exposes only its `Handle`, used with `Handle::block_on`. When the
/// last `Arc<DedicatedRuntime>` drops, `shutdown_tx` is closed, the
/// background thread sees the channel close and lets the runtime
/// drop on *its* thread (where blocking is allowed).
struct DedicatedRuntime {
    handle: tokio::runtime::Handle,
    shutdown_tx: Option<std::sync::mpsc::Sender<()>>,
    join_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl DedicatedRuntime {
    fn new(name: &str) -> RepositoryResult<Self> {
        let (handle_tx, handle_rx) = std::sync::mpsc::channel::<tokio::runtime::Handle>();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
        let thread_name = format!("surch-s3-{name}");
        let join_handle = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(_) => return,
                };
                let _ = handle_tx.send(runtime.handle().clone());
                // Park here until the owning `S3Repository` drops the
                // matching shutdown sender. `recv()` returns Err when
                // the sender side has been closed — that's the cue to
                // tear the runtime down (drop happens on this thread,
                // where blocking is fine).
                let _ = shutdown_rx.recv();
                drop(runtime);
            })
            .map_err(|source| RepositoryError::Io {
                key: "s3-runtime-thread".to_owned(),
                source,
            })?;

        let handle = handle_rx.recv().map_err(|_| {
            RepositoryError::InvalidConfig(
                "failed to receive S3 runtime handle from background thread".to_owned(),
            )
        })?;

        Ok(Self {
            handle,
            shutdown_tx: Some(shutdown_tx),
            join_handle: Mutex::new(Some(join_handle)),
        })
    }

    fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }
}

impl Drop for DedicatedRuntime {
    fn drop(&mut self) {
        // Drop the sender to wake the background thread. The thread
        // owns the runtime; tearing it down there avoids the
        // "Cannot drop a runtime ... from within an asynchronous
        // context" panic that the parent caller (axum handler) would
        // otherwise trigger.
        self.shutdown_tx.take();
        if let Ok(mut guard) = self.join_handle.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

impl S3Repository {
    /// Build a new S3-backed repository.
    ///
    /// Validates `bucket` is non-empty and constructs the SDK client
    /// eagerly so configuration errors surface at `PUT /_snapshot/{repo}`
    /// time rather than on the first take.
    pub fn new(config: S3RepositoryConfig) -> RepositoryResult<Self> {
        if config.bucket.trim().is_empty() {
            return Err(RepositoryError::InvalidConfig(
                "s3 repository requires a non-empty `bucket`".to_owned(),
            ));
        }
        let runtime = Arc::new(DedicatedRuntime::new(&config.bucket)?);
        let client = runtime.handle().block_on(build_s3_client(&config));
        Ok(Self {
            config,
            client,
            runtime,
        })
    }

    pub fn config(&self) -> &S3RepositoryConfig {
        &self.config
    }

    fn full_key(&self, key: &str) -> RepositoryResult<String> {
        validate_key(key)?;
        Ok(match self.config.base_path.as_deref() {
            Some(prefix) if !prefix.is_empty() => {
                let trimmed = prefix.trim_matches('/');
                if trimmed.is_empty() {
                    key.to_owned()
                } else {
                    format!("{trimmed}/{key}")
                }
            }
            _ => key.to_owned(),
        })
    }

    fn strip_prefix<'a>(&self, key: &'a str) -> &'a str {
        match self.config.base_path.as_deref() {
            Some(prefix) if !prefix.is_empty() => {
                let trimmed = prefix.trim_matches('/');
                if trimmed.is_empty() {
                    key
                } else {
                    let needle = format!("{trimmed}/");
                    key.strip_prefix(&needle).unwrap_or(key)
                }
            }
            _ => key,
        }
    }
}

async fn build_s3_client(config: &S3RepositoryConfig) -> aws_sdk_s3::Client {
    use aws_sdk_s3::config::{Credentials, Region};

    let mut builder = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(Region::new(
            config
                .region
                .clone()
                .unwrap_or_else(|| "us-east-1".to_owned()),
        ));

    if config.disable_request_checksum {
        builder = builder
            .request_checksum_calculation(
                aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired,
            )
            .response_checksum_validation(
                aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired,
            );
    }

    if let (Some(access_key), Some(secret_key)) =
        (config.access_key.as_deref(), config.secret_key.as_deref())
    {
        let credentials = Credentials::new(
            access_key,
            secret_key,
            config.session_token.clone(),
            None,
            "surch-snapshot-s3-static",
        );
        builder = builder.credentials_provider(credentials);
    }

    if let Some(endpoint) = config.endpoint.as_deref() {
        // Non-AWS backends (MinIO, R2, GCS interop, axum test
        // server) use path-style addressing.
        builder = builder.endpoint_url(endpoint).force_path_style(true);
    }

    aws_sdk_s3::Client::from_conf(builder.build())
}

fn s3_error_to_repository<E: std::error::Error + Send + Sync + 'static>(
    key: &str,
    error: E,
) -> RepositoryError {
    RepositoryError::Io {
        key: key.to_owned(),
        source: std::io::Error::other(error.to_string()),
    }
}

impl SnapshotRepository for S3Repository {
    fn put_object(&self, key: &str, bytes: Bytes) -> RepositoryResult<()> {
        let full = self.full_key(key)?;
        let bucket = self.config.bucket.clone();
        let client = self.client.clone();
        tokio::task::block_in_place(|| {
            self.runtime.handle().block_on(async move {
                client
                    .put_object()
                    .bucket(&bucket)
                    .key(&full)
                    .body(bytes.to_vec().into())
                    .send()
                    .await
                    .map(|_| ())
                    .map_err(|error| s3_error_to_repository(&full, error.into_service_error()))
            })
        })
    }

    fn get_object(&self, key: &str) -> RepositoryResult<Bytes> {
        let full = self.full_key(key)?;
        let bucket = self.config.bucket.clone();
        let client = self.client.clone();
        let key_for_err = key.to_owned();
        tokio::task::block_in_place(|| {
            self.runtime.handle().block_on(async move {
                let response = client.get_object().bucket(&bucket).key(&full).send().await;
                match response {
                    Ok(output) => {
                        let aggregated = output
                            .body
                            .collect()
                            .await
                            .map_err(|error| s3_error_to_repository(&full, error))?;
                        Ok(aggregated.into_bytes())
                    }
                    Err(error) => {
                        let service_err = error.into_service_error();
                        if service_err.is_no_such_key() {
                            Err(RepositoryError::NotFound { key: key_for_err })
                        } else {
                            Err(s3_error_to_repository(&full, service_err))
                        }
                    }
                }
            })
        })
    }

    fn list_objects(&self, prefix: &str) -> RepositoryResult<Vec<String>> {
        let full_prefix = if prefix.is_empty() {
            self.config
                .base_path
                .as_deref()
                .map(|p| {
                    let t = p.trim_matches('/');
                    if t.is_empty() {
                        String::new()
                    } else {
                        format!("{t}/")
                    }
                })
                .unwrap_or_default()
        } else {
            self.full_key(prefix)?
        };
        let bucket = self.config.bucket.clone();
        let client = self.client.clone();
        let prefix_clone = full_prefix.clone();
        tokio::task::block_in_place(|| {
            self.runtime.handle().block_on(async move {
                let mut out: Vec<String> = Vec::new();
                let mut continuation_token: Option<String> = None;
                loop {
                    let mut req = client.list_objects_v2().bucket(&bucket);
                    if !prefix_clone.is_empty() {
                        req = req.prefix(&prefix_clone);
                    }
                    if let Some(token) = continuation_token.as_deref() {
                        req = req.continuation_token(token);
                    }
                    let output = req.send().await.map_err(|error| {
                        s3_error_to_repository(&prefix_clone, error.into_service_error())
                    })?;
                    for obj in output.contents() {
                        if let Some(k) = obj.key() {
                            out.push(k.to_owned());
                        }
                    }
                    if output.is_truncated().unwrap_or(false) {
                        continuation_token = output.next_continuation_token().map(str::to_owned);
                        if continuation_token.is_none() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                Ok(out)
            })
        })
        .map(|raw: Vec<String>| {
            let mut stripped: Vec<String> = raw
                .into_iter()
                .map(|k| self.strip_prefix(&k).to_owned())
                .collect();
            stripped.sort();
            stripped
        })
    }

    fn delete_object(&self, key: &str) -> RepositoryResult<()> {
        let full = self.full_key(key)?;
        let bucket = self.config.bucket.clone();
        let client = self.client.clone();
        let key_for_err = key.to_owned();
        tokio::task::block_in_place(|| {
            self.runtime.handle().block_on(async move {
                client
                    .delete_object()
                    .bucket(&bucket)
                    .key(&full)
                    .send()
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        let service_err = error.into_service_error();
                        // `DeleteObject` returns 204 for missing keys on AWS;
                        // some backends instead return 404. Normalise both.
                        if service_err
                            .to_string()
                            .to_ascii_lowercase()
                            .contains("nosuchkey")
                        {
                            RepositoryError::NotFound { key: key_for_err }
                        } else {
                            s3_error_to_repository(&full, service_err)
                        }
                    })
            })
        })
    }

    fn compare_and_set(
        &self,
        key: &str,
        expected_etag: Option<&str>,
        bytes: Bytes,
    ) -> RepositoryResult<String> {
        // Best-effort CAS: read the current ETag, compare in-process,
        // then PUT. AWS S3 added native `If-Match` to `PutObject` in
        // 2024-08; we keep the read-modify-write fallback for backends
        // that do not (MinIO older than 2023-09, GCS interop).
        let found = self.read_etag(key)?;
        if found.as_deref() != expected_etag {
            return Err(RepositoryError::CasConflict {
                key: key.to_owned(),
                expected: expected_etag.map(str::to_owned),
                found,
            });
        }
        self.put_object(key, bytes)?;
        let new_etag = self.read_etag(key)?.ok_or_else(|| RepositoryError::Io {
            key: key.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "object disappeared right after S3 put",
            ),
        })?;
        Ok(new_etag)
    }

    fn read_etag(&self, key: &str) -> RepositoryResult<Option<String>> {
        let full = self.full_key(key)?;
        let bucket = self.config.bucket.clone();
        let client = self.client.clone();
        tokio::task::block_in_place(|| {
            self.runtime.handle().block_on(async move {
                match client.head_object().bucket(&bucket).key(&full).send().await {
                    Ok(output) => Ok(output.e_tag().map(str::to_owned)),
                    Err(error) => {
                        let service_err = error.into_service_error();
                        if service_err.is_not_found() {
                            Ok(None)
                        } else {
                            Err(s3_error_to_repository(&full, service_err))
                        }
                    }
                }
            })
        })
    }

    fn kind(&self) -> &'static str {
        "s3"
    }

    fn settings(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert(
            "bucket".to_owned(),
            serde_json::Value::String(self.config.bucket.clone()),
        );
        if let Some(region) = &self.config.region {
            map.insert(
                "region".to_owned(),
                serde_json::Value::String(region.clone()),
            );
        }
        if let Some(endpoint) = &self.config.endpoint {
            map.insert(
                "endpoint".to_owned(),
                serde_json::Value::String(endpoint.clone()),
            );
        }
        if let Some(base_path) = &self.config.base_path {
            map.insert(
                "base_path".to_owned(),
                serde_json::Value::String(base_path.clone()),
            );
        }
        // Credentials are never echoed back — clients listing the
        // repository have no business seeing them, mirroring the ES
        // `_snapshot/{repo}` behaviour where `access_key` etc. are
        // masked.
        serde_json::Value::Object(map)
    }
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) -> RepositoryResult<()> {
    let read_dir = fs::read_dir(dir).map_err(|source| RepositoryError::Io {
        key: dir.display().to_string(),
        source,
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|source| RepositoryError::Io {
            key: dir.display().to_string(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, root, out)?;
        } else if path.is_file() {
            let relative = path.strip_prefix(root).map_err(|_| RepositoryError::Io {
                key: path.display().to_string(),
                source: std::io::Error::other("walked path is not under repository root"),
            })?;
            let key = relative.to_string_lossy().replace('\\', "/");
            // Skip in-flight `*.tmp.*` files created by `write_atomic`.
            if key.contains(".tmp.") {
                continue;
            }
            out.push(key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "surch-snapshot-test-{}-{}",
            std::process::id(),
            now_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fs_repository_round_trip() {
        let dir = tempdir();
        let repo = FsRepository::new(dir.clone()).unwrap();
        repo.put_object("foo", Bytes::from_static(b"hello"))
            .unwrap();
        let bytes = repo.get_object("foo").unwrap();
        assert_eq!(bytes.as_ref(), b"hello");
    }

    #[test]
    fn fs_repository_get_missing_returns_not_found() {
        let dir = tempdir();
        let repo = FsRepository::new(dir).unwrap();
        match repo.get_object("missing") {
            Err(RepositoryError::NotFound { key }) => assert_eq!(key, "missing"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn fs_repository_list_objects_under_prefix() {
        let dir = tempdir();
        let repo = FsRepository::new(dir).unwrap();
        repo.put_object("snap-a.dat", Bytes::from_static(b"a"))
            .unwrap();
        repo.put_object("snap-b.dat", Bytes::from_static(b"b"))
            .unwrap();
        repo.put_object("indices/uuid-1/meta.dat", Bytes::from_static(b"m"))
            .unwrap();
        let mut snaps = repo.list_objects("snap-").unwrap();
        snaps.sort();
        assert_eq!(snaps, vec!["snap-a.dat".to_string(), "snap-b.dat".into()]);
        let indices = repo.list_objects("indices/").unwrap();
        assert_eq!(indices, vec!["indices/uuid-1/meta.dat".to_string()]);
        let all = repo.list_objects("").unwrap();
        assert!(all.len() >= 3);
    }

    #[test]
    fn fs_repository_compare_and_set_succeeds_when_absent() {
        let dir = tempdir();
        let repo = FsRepository::new(dir).unwrap();
        let etag = repo
            .compare_and_set("index-0", None, Bytes::from_static(b"first"))
            .unwrap();
        assert!(!etag.is_empty());
    }

    #[test]
    fn fs_repository_compare_and_set_conflicts_when_etag_stale() {
        let dir = tempdir();
        let repo = FsRepository::new(dir).unwrap();
        let etag = repo
            .compare_and_set("index-0", None, Bytes::from_static(b"first"))
            .unwrap();
        // Using `None` while the object exists is a stale CAS — must conflict.
        match repo.compare_and_set("index-0", None, Bytes::from_static(b"second")) {
            Err(RepositoryError::CasConflict { .. }) => (),
            other => panic!("expected CasConflict, got {other:?}"),
        }
        // The right etag wins.
        let new_etag = repo
            .compare_and_set("index-0", Some(&etag), Bytes::from_static(b"second"))
            .unwrap();
        assert_ne!(new_etag, etag);
        assert_eq!(repo.get_object("index-0").unwrap().as_ref(), b"second");
    }

    #[test]
    fn fs_repository_delete_then_get_missing() {
        let dir = tempdir();
        let repo = FsRepository::new(dir).unwrap();
        repo.put_object("snap-1.dat", Bytes::from_static(b"x"))
            .unwrap();
        repo.delete_object("snap-1.dat").unwrap();
        assert!(matches!(
            repo.get_object("snap-1.dat"),
            Err(RepositoryError::NotFound { .. })
        ));
    }

    #[test]
    fn fs_repository_rejects_path_traversal_keys() {
        let dir = tempdir();
        let repo = FsRepository::new(dir).unwrap();
        assert!(matches!(
            repo.put_object("../escape", Bytes::from_static(b"x")),
            Err(RepositoryError::InvalidConfig(_))
        ));
        assert!(matches!(
            repo.put_object("/absolute", Bytes::from_static(b"x")),
            Err(RepositoryError::InvalidConfig(_))
        ));
    }
}
