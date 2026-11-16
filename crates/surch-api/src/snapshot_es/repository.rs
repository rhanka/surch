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

    /// ES `readonly` repository setting: a readonly repository refuses
    /// snapshot creation and deletion (`SnapshotsService.createSnapshot`,
    /// ES 8.6.1) but serves listing and restore.
    fn readonly(&self) -> bool {
        false
    }

    /// `true` when `key` exists. Used by the L6.2 segment snapshots to
    /// skip a content-addressed blob already present in the repository.
    fn object_exists(&self, key: &str) -> RepositoryResult<bool> {
        Ok(self.read_etag(key)?.is_some())
    }

    /// Store `path[offset .. offset + len)` under `key`. The default reads
    /// the range into memory; the `fs` and `s3` implementations stream it
    /// (a sealed postings file of the 29 M matchID index is several GiB).
    fn put_file_range(
        &self,
        key: &str,
        path: &Path,
        offset: u64,
        len: u64,
    ) -> RepositoryResult<()> {
        let bytes = read_file_range(path, offset, len)?;
        self.put_object(key, Bytes::from(bytes))
    }

    /// Stream the object `key` into `sink`, returning the byte count. The
    /// default buffers the whole object; `fs` and `s3` stream it.
    fn get_object_to_writer(&self, key: &str, sink: &mut dyn Write) -> RepositoryResult<u64> {
        let bytes = self.get_object(key)?;
        sink.write_all(&bytes)
            .map_err(|source| RepositoryError::Io {
                key: key.to_owned(),
                source,
            })?;
        Ok(bytes.len() as u64)
    }
}

fn read_file_range(path: &Path, offset: u64, len: u64) -> RepositoryResult<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let io_err = |source| RepositoryError::Io {
        key: path.display().to_string(),
        source,
    };
    let mut file = fs::File::open(path).map_err(io_err)?;
    file.seek(SeekFrom::Start(offset)).map_err(io_err)?;
    let mut out = vec![0u8; usize::try_from(len).unwrap_or(usize::MAX)];
    file.read_exact(&mut out).map_err(io_err)?;
    Ok(out)
}

/// Copy `path[offset .. offset + len)` into `sink` through a bounded buffer.
fn copy_file_range_to(
    path: &Path,
    offset: u64,
    len: u64,
    sink: &mut dyn Write,
) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let copied = std::io::copy(&mut (&mut file).take(len), sink)?;
    if copied != len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("{} is shorter than {} bytes", path.display(), offset + len),
        ));
    }
    Ok(())
}

/// ES renders every repository setting as a string in
/// `GET /_snapshot/{repo}` (settings are flattened `Settings`). Credentials
/// and Surch test switches are never echoed.
pub(crate) fn echo_settings(settings: &serde_json::Value) -> serde_json::Value {
    const HIDDEN: &[&str] = &[
        "access_key",
        "secret_key",
        "session_token",
        "disable_request_checksum",
        "_disable_request_checksum",
    ];
    let mut out = serde_json::Map::new();
    if let Some(map) = settings.as_object() {
        for (key, value) in map {
            if HIDDEN.contains(&key.as_str()) {
                continue;
            }
            let rendered = match value {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Null => continue,
                other => other.to_string(),
            };
            out.insert(key.clone(), serde_json::Value::String(rendered));
        }
    }
    serde_json::Value::Object(out)
}

/// ES boolean setting parsing (`Booleans.parseBoolean`): `true` / `false`
/// as JSON booleans or strings; anything else is a configuration error.
pub(crate) fn setting_bool(
    settings: &serde_json::Value,
    key: &str,
) -> RepositoryResult<Option<bool>> {
    match settings.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(value)) => Ok(Some(*value)),
        Some(serde_json::Value::String(text)) if text == "true" => Ok(Some(true)),
        Some(serde_json::Value::String(text)) if text == "false" => Ok(Some(false)),
        Some(other) => {
            Err(RepositoryError::InvalidConfig(format!(
            "Failed to parse value [{}] as only [true] or [false] are allowed for setting [{key}]",
            other.as_str().map(str::to_owned).unwrap_or_else(|| other.to_string())
        )))
        }
    }
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
    readonly: bool,
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
            readonly: false,
        })
    }

    /// ES `readonly: true` on an `fs` repository (snapshot creation and
    /// deletion refused, restore allowed).
    pub fn with_readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
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
        self.write_atomic_with(target, |file| file.write_all(bytes))
    }

    fn write_atomic_with(
        &self,
        target: &Path,
        fill: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
    ) -> RepositoryResult<()> {
        self.ensure_parent(target)?;
        // `<name>.tmp.<pid>.<nanos>.<seq>`: unique per writer (two threads
        // racing on one key, one of them loses the CAS upstream anyway) and
        // skipped by `list_objects` thanks to the `.tmp.` marker.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = target.with_file_name(format!(
            "{}.tmp.{}.{}.{seq}",
            target
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("blob"),
            std::process::id(),
            now_nanos(),
        ));
        {
            let mut file = fs::File::create(&tmp).map_err(|source| RepositoryError::Io {
                key: tmp.display().to_string(),
                source,
            })?;
            if let Err(source) = fill(&mut file) {
                drop(file);
                let _ = fs::remove_file(&tmp);
                return Err(RepositoryError::Io {
                    key: tmp.display().to_string(),
                    source,
                });
            }
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
        let mut settings = serde_json::json!({ "location": self.root.display().to_string() });
        if self.readonly {
            settings["readonly"] = serde_json::Value::String("true".to_owned());
        }
        settings
    }

    fn readonly(&self) -> bool {
        self.readonly
    }

    fn put_file_range(
        &self,
        key: &str,
        path: &Path,
        offset: u64,
        len: u64,
    ) -> RepositoryResult<()> {
        let target = self.key_path(key)?;
        self.write_atomic_with(&target, |file| {
            let mut writer = std::io::BufWriter::with_capacity(1 << 20, file);
            copy_file_range_to(path, offset, len, &mut writer)?;
            writer.flush()
        })
    }

    fn get_object_to_writer(&self, key: &str, sink: &mut dyn Write) -> RepositoryResult<u64> {
        let path = self.key_path(key)?;
        let mut file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(RepositoryError::NotFound {
                    key: key.to_owned(),
                })
            }
            Err(source) => {
                return Err(RepositoryError::Io {
                    key: key.to_owned(),
                    source,
                })
            }
        };
        std::io::copy(&mut file, sink).map_err(|source| RepositoryError::Io {
            key: key.to_owned(),
            source,
        })
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
    /// ES `path_style_access`. `None` keeps the historical Surch default:
    /// path-style addressing whenever a custom `endpoint` is set, the
    /// SDK default (virtual-hosted) otherwise.
    pub path_style_access: Option<bool>,
    /// ES `protocol` (`http` / `https`, default `https`), used to build the
    /// endpoint URL when `endpoint` is a bare host (`s3.fr-par.scw.cloud`,
    /// the ES convention).
    pub protocol: Option<String>,
    /// ES `client`: the named client whose credentials are read from the
    /// environment (`SURCH_S3_CLIENT_<NAME>_ACCESS_KEY`, see
    /// [`resolve_s3_credentials`]). ES reads them from its keystore
    /// (`s3.client.<name>.access_key`), which Surch does not have.
    pub client: Option<String>,
    /// ES `readonly`: snapshot creation / deletion refused.
    pub readonly: bool,
    /// ES `buffer_size`: objects larger than this go through the S3
    /// multipart upload API, in parts of this size. `0` = default 100 MiB
    /// (ES default `100mb`).
    pub buffer_size: u64,
    /// The declared settings as `GET /_snapshot/{repo}` echoes them
    /// (strings, credentials removed). `None` = built from the fields.
    pub echo: Option<serde_json::Value>,
}

/// ES `buffer_size` default (`100mb`) and minimum (`5mb`, the S3 part
/// floor).
pub const S3_DEFAULT_BUFFER_SIZE: u64 = 100 * 1024 * 1024;
pub const S3_MIN_BUFFER_SIZE: u64 = 5 * 1024 * 1024;

impl S3RepositoryConfig {
    fn part_size(&self) -> u64 {
        if self.buffer_size == 0 {
            S3_DEFAULT_BUFFER_SIZE
        } else {
            self.buffer_size
        }
    }

    /// Endpoint URL handed to the SDK: `endpoint` as is when it carries a
    /// scheme, `<protocol>://<endpoint>` otherwise (ES semantics).
    pub fn endpoint_url(&self) -> RepositoryResult<Option<String>> {
        let Some(endpoint) = self.endpoint.as_deref().map(str::trim) else {
            return Ok(None);
        };
        if endpoint.is_empty() {
            return Ok(None);
        }
        if endpoint.contains("://") {
            return Ok(Some(endpoint.to_owned()));
        }
        let protocol = self
            .protocol
            .as_deref()
            .unwrap_or("https")
            .to_ascii_lowercase();
        if protocol != "http" && protocol != "https" {
            return Err(RepositoryError::InvalidConfig(format!(
                "invalid s3 `protocol` [{protocol}] (expected `http` or `https`)"
            )));
        }
        Ok(Some(format!("{protocol}://{endpoint}")))
    }

    /// Effective path-style addressing (see [`Self::path_style_access`]).
    pub fn effective_path_style(&self) -> bool {
        self.path_style_access.unwrap_or_else(|| {
            self.endpoint
                .as_deref()
                .is_some_and(|e| !e.trim().is_empty())
        })
    }
}

/// Static S3 credentials and where they were found (logged, never echoed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedS3Credentials {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
    pub source: String,
}

/// Environment variable prefix of the per-client credentials, the Surch
/// stand-in for the ES keystore entries `s3.client.<name>.access_key` /
/// `secret_key` / `session_token`.
pub const S3_CLIENT_ENV_PREFIX: &str = "SURCH_S3_CLIENT_";

/// `SURCH_S3_CLIENT_<NAME>_<SUFFIX>`: client name upper-cased, every
/// character outside `[A-Z0-9]` replaced by `_` (`default` ->
/// `SURCH_S3_CLIENT_DEFAULT_ACCESS_KEY`).
pub fn s3_client_env_name(client: &str, suffix: &str) -> String {
    let name: String = client
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{S3_CLIENT_ENV_PREFIX}{name}_{suffix}")
}

/// Credential resolution, first match wins:
///
/// 1. `access_key` / `secret_key` (+ `session_token`) in the repository
///    settings (historical Surch shape; ES 8 rejects them there),
/// 2. `SURCH_S3_CLIENT_<CLIENT>_ACCESS_KEY` / `_SECRET_KEY` /
///    `_SESSION_TOKEN` for the repository's `client` (default `default`),
/// 3. the standard AWS variables `AWS_ACCESS_KEY_ID` /
///    `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN`.
///
/// `None` = no credentials found: the repository still registers (as in
/// ES, where missing keystore entries surface on first use) and the S3
/// calls fail with the SDK credentials error.
pub fn resolve_s3_credentials(
    config: &S3RepositoryConfig,
    env: impl Fn(&str) -> Option<String>,
) -> Option<ResolvedS3Credentials> {
    let non_empty = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
    if let (Some(access_key), Some(secret_key)) = (
        non_empty(config.access_key.clone()),
        non_empty(config.secret_key.clone()),
    ) {
        return Some(ResolvedS3Credentials {
            access_key,
            secret_key,
            session_token: non_empty(config.session_token.clone()),
            source: "repository settings".to_owned(),
        });
    }
    let client = config.client.as_deref().unwrap_or("default");
    let access_name = s3_client_env_name(client, "ACCESS_KEY");
    let secret_name = s3_client_env_name(client, "SECRET_KEY");
    if let (Some(access_key), Some(secret_key)) =
        (non_empty(env(&access_name)), non_empty(env(&secret_name)))
    {
        return Some(ResolvedS3Credentials {
            access_key,
            secret_key,
            session_token: non_empty(env(&s3_client_env_name(client, "SESSION_TOKEN"))),
            source: format!("{access_name} / {secret_name}"),
        });
    }
    if let (Some(access_key), Some(secret_key)) = (
        non_empty(env("AWS_ACCESS_KEY_ID")),
        non_empty(env("AWS_SECRET_ACCESS_KEY")),
    ) {
        return Some(ResolvedS3Credentials {
            access_key,
            secret_key,
            session_token: non_empty(env("AWS_SESSION_TOKEN")),
            source: "AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY".to_owned(),
        });
    }
    None
}

/// Parse an ES byte-size setting (`100mb`, `5MB`, `1gb`, `512kb`, `1048576`).
pub fn parse_byte_size(text: &str) -> Option<u64> {
    let lower = text.trim().to_ascii_lowercase();
    let (digits, unit) = match lower.find(|c: char| !c.is_ascii_digit()) {
        Some(at) => lower.split_at(at),
        None => (lower.as_str(), ""),
    };
    let value: u64 = digits.parse().ok()?;
    let factor: u64 = match unit.trim() {
        "" | "b" => 1,
        "k" | "kb" => 1 << 10,
        "m" | "mb" => 1 << 20,
        "g" | "gb" => 1 << 30,
        "t" | "tb" => 1 << 40,
        _ => return None,
    };
    value.checked_mul(factor)
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
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    join_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl DedicatedRuntime {
    fn new(name: &str) -> RepositoryResult<Self> {
        let (handle_tx, handle_rx) = std::sync::mpsc::channel::<tokio::runtime::Handle>();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
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
                // Drive the runtime here until the owning `S3Repository`
                // drops the shutdown sender. L6.2 fix: on a current-thread
                // runtime only `Runtime::block_on` drives the IO and timer
                // drivers — `Handle::block_on` from the callers cannot (tokio
                // docs). The former plain `std::sync::mpsc::recv()` parked
                // this thread without driving anything, so every S3 request
                // waited forever for its socket. The runtime is then dropped
                // on this thread, where blocking is fine.
                runtime.block_on(async move {
                    let _ = shutdown_rx.await;
                });
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
        if config.buffer_size != 0 && config.buffer_size < S3_MIN_BUFFER_SIZE {
            return Err(RepositoryError::InvalidConfig(format!(
                "s3 `buffer_size` must be at least 5mb, got {} bytes",
                config.buffer_size
            )));
        }
        let endpoint_url = config.endpoint_url()?;
        let credentials = resolve_s3_credentials(&config, |name| std::env::var(name).ok());
        match &credentials {
            Some(found) => tracing::info!(target: "surch_api::snapshot", bucket = %config.bucket,
                source = %found.source, "s3 repository credentials resolved"),
            None => tracing::warn!(target: "surch_api::snapshot", bucket = %config.bucket,
                "s3 repository has no credentials (settings, SURCH_S3_CLIENT_<NAME>_ACCESS_KEY, \
                 AWS_ACCESS_KEY_ID): requests will fail unless the backend allows them"),
        }
        let runtime = Arc::new(DedicatedRuntime::new(&config.bucket)?);
        let client = runtime
            .handle()
            .block_on(build_s3_client(&config, endpoint_url, credentials));
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

async fn build_s3_client(
    config: &S3RepositoryConfig,
    endpoint_url: Option<String>,
    credentials: Option<ResolvedS3Credentials>,
) -> aws_sdk_s3::Client {
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

    if let Some(credentials) = credentials {
        builder = builder.credentials_provider(Credentials::new(
            credentials.access_key,
            credentials.secret_key,
            credentials.session_token,
            None,
            "surch-snapshot-s3-static",
        ));
    }

    if let Some(endpoint) = endpoint_url {
        builder = builder.endpoint_url(endpoint);
    }
    // ES `path_style_access`; unset keeps the historical Surch default
    // (path style for every custom endpoint: MinIO, R2, Scaleway, mocks).
    builder = builder.force_path_style(config.effective_path_style());

    aws_sdk_s3::Client::from_conf(builder.build())
}

fn s3_error_to_repository<E: std::error::Error + Send + Sync + 'static>(
    key: &str,
    error: E,
) -> RepositoryError {
    // `DisplayErrorContext` walks the source chain: the bare `to_string()`
    // of an SDK service error is often just "unhandled error".
    RepositoryError::Io {
        key: key.to_owned(),
        source: std::io::Error::other(aws_sdk_s3::error::DisplayErrorContext(&error).to_string()),
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

    fn readonly(&self) -> bool {
        self.config.readonly
    }

    fn put_file_range(
        &self,
        key: &str,
        path: &Path,
        offset: u64,
        len: u64,
    ) -> RepositoryResult<()> {
        use aws_sdk_s3::primitives::{ByteStream, Length};
        use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};

        let full = self.full_key(key)?;
        let bucket = self.config.bucket.clone();
        let client = self.client.clone();
        let part_size = self.config.part_size();
        let path = path.to_path_buf();
        let stream = |from: u64, take: u64| {
            let path = path.clone();
            async move {
                if take == 0 {
                    return Ok(ByteStream::from_static(b""));
                }
                ByteStream::read_from()
                    .path(&path)
                    .offset(from)
                    .length(Length::Exact(take))
                    .build()
                    .await
            }
        };
        tokio::task::block_in_place(|| {
            self.runtime.handle().block_on(async {
                if len <= part_size {
                    let body = stream(offset, len)
                        .await
                        .map_err(|error| s3_error_to_repository(&full, error))?;
                    return client
                        .put_object()
                        .bucket(&bucket)
                        .key(&full)
                        .content_length(len as i64)
                        .body(body)
                        .send()
                        .await
                        .map(|_| ())
                        .map_err(|error| {
                            s3_error_to_repository(&full, error.into_service_error())
                        });
                }
                // ES `buffer_size` semantics: beyond it, multipart upload in
                // parts of `buffer_size` bytes (S3: <= 5 GiB per PUT,
                // <= 10 000 parts).
                let created = client
                    .create_multipart_upload()
                    .bucket(&bucket)
                    .key(&full)
                    .send()
                    .await
                    .map_err(|error| s3_error_to_repository(&full, error.into_service_error()))?;
                let upload_id =
                    created
                        .upload_id()
                        .map(str::to_owned)
                        .ok_or_else(|| RepositoryError::Io {
                            key: full.clone(),
                            source: std::io::Error::other(
                                "CreateMultipartUpload returned no UploadId",
                            ),
                        })?;
                let upload = async {
                    let mut parts = Vec::new();
                    let mut done = 0u64;
                    let mut number = 1i32;
                    while done < len {
                        let take = part_size.min(len - done);
                        let body = stream(offset + done, take)
                            .await
                            .map_err(|error| s3_error_to_repository(&full, error))?;
                        let output = client
                            .upload_part()
                            .bucket(&bucket)
                            .key(&full)
                            .upload_id(&upload_id)
                            .part_number(number)
                            .content_length(take as i64)
                            .body(body)
                            .send()
                            .await
                            .map_err(|error| {
                                s3_error_to_repository(&full, error.into_service_error())
                            })?;
                        parts.push(
                            CompletedPart::builder()
                                .part_number(number)
                                .set_e_tag(output.e_tag().map(str::to_owned))
                                .build(),
                        );
                        done += take;
                        number += 1;
                    }
                    client
                        .complete_multipart_upload()
                        .bucket(&bucket)
                        .key(&full)
                        .upload_id(&upload_id)
                        .multipart_upload(
                            CompletedMultipartUpload::builder()
                                .set_parts(Some(parts))
                                .build(),
                        )
                        .send()
                        .await
                        .map(|_| ())
                        .map_err(|error| s3_error_to_repository(&full, error.into_service_error()))
                };
                let result = upload.await;
                if result.is_err() {
                    let _ = client
                        .abort_multipart_upload()
                        .bucket(&bucket)
                        .key(&full)
                        .upload_id(&upload_id)
                        .send()
                        .await;
                }
                result
            })
        })
    }

    fn get_object_to_writer(&self, key: &str, sink: &mut dyn Write) -> RepositoryResult<u64> {
        let full = self.full_key(key)?;
        let bucket = self.config.bucket.clone();
        let client = self.client.clone();
        tokio::task::block_in_place(|| {
            self.runtime.handle().block_on(async {
                let mut output = match client.get_object().bucket(&bucket).key(&full).send().await {
                    Ok(output) => output,
                    Err(error) => {
                        let service_err = error.into_service_error();
                        return Err(if service_err.is_no_such_key() {
                            RepositoryError::NotFound {
                                key: key.to_owned(),
                            }
                        } else {
                            s3_error_to_repository(&full, service_err)
                        });
                    }
                };
                let mut written = 0u64;
                while let Some(chunk) = output
                    .body
                    .try_next()
                    .await
                    .map_err(|error| s3_error_to_repository(&full, error))?
                {
                    sink.write_all(&chunk)
                        .map_err(|source| RepositoryError::Io {
                            key: key.to_owned(),
                            source,
                        })?;
                    written += chunk.len() as u64;
                }
                Ok(written)
            })
        })
    }

    fn settings(&self) -> serde_json::Value {
        if let Some(echo) = &self.config.echo {
            return echo.clone();
        }
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
    fn fs_repository_streams_file_ranges_both_ways() {
        let dir = tempdir();
        let repo = FsRepository::new(dir.join("repo")).unwrap();
        let source = dir.join("source.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&source, &data).unwrap();
        repo.put_file_range("blobs/x", &source, 1000, 2_000_000)
            .unwrap();
        let mut out = Vec::new();
        assert_eq!(
            repo.get_object_to_writer("blobs/x", &mut out).unwrap(),
            2_000_000
        );
        assert_eq!(out, &data[1000..2_001_000]);
        assert!(repo.object_exists("blobs/x").unwrap());
        assert!(!repo.object_exists("blobs/y").unwrap());
        // A short source fails and leaves no object (nor a visible tmp).
        assert!(repo
            .put_file_range("blobs/short", &source, 2_999_000, 10_000)
            .is_err());
        assert!(!repo.object_exists("blobs/short").unwrap());
        assert_eq!(
            repo.list_objects("blobs/").unwrap(),
            vec!["blobs/x".to_owned()]
        );
        assert!(matches!(
            repo.get_object_to_writer("blobs/none", &mut Vec::new()),
            Err(RepositoryError::NotFound { .. })
        ));
    }

    #[test]
    fn fs_readonly_is_reported_and_echoed() {
        let dir = tempdir();
        let repo = FsRepository::new(dir).unwrap().with_readonly(true);
        assert!(repo.readonly());
        assert_eq!(repo.settings()["readonly"], serde_json::json!("true"));
    }

    #[test]
    fn s3_endpoint_url_follows_es_protocol_setting() {
        let mut config = S3RepositoryConfig {
            bucket: "b".into(),
            endpoint: Some("s3.fr-par.scw.cloud".into()),
            ..Default::default()
        };
        assert_eq!(
            config.endpoint_url().unwrap().as_deref(),
            Some("https://s3.fr-par.scw.cloud")
        );
        config.protocol = Some("http".into());
        assert_eq!(
            config.endpoint_url().unwrap().as_deref(),
            Some("http://s3.fr-par.scw.cloud")
        );
        config.endpoint = Some("http://127.0.0.1:9000".into());
        assert_eq!(
            config.endpoint_url().unwrap().as_deref(),
            Some("http://127.0.0.1:9000")
        );
        config.endpoint = Some("minio:9000".into());
        config.protocol = Some("ftp".into());
        assert!(config.endpoint_url().is_err());
        // Path style: explicit wins, else on for custom endpoints.
        assert!(config.effective_path_style());
        config.path_style_access = Some(false);
        assert!(!config.effective_path_style());
        let aws = S3RepositoryConfig::default();
        assert!(!aws.effective_path_style());
    }

    #[test]
    fn s3_credentials_resolve_settings_then_client_env_then_aws_env() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        let base = S3RepositoryConfig {
            bucket: "b".into(),
            ..Default::default()
        };
        assert_eq!(resolve_s3_credentials(&base, env(&[])), None);
        let aws = resolve_s3_credentials(
            &base,
            env(&[("AWS_ACCESS_KEY_ID", "ak"), ("AWS_SECRET_ACCESS_KEY", "sk")]),
        )
        .unwrap();
        assert_eq!(
            (aws.access_key.as_str(), aws.secret_key.as_str()),
            ("ak", "sk")
        );
        let client = resolve_s3_credentials(
            &base,
            env(&[
                ("AWS_ACCESS_KEY_ID", "ak"),
                ("AWS_SECRET_ACCESS_KEY", "sk"),
                ("SURCH_S3_CLIENT_DEFAULT_ACCESS_KEY", "cak"),
                ("SURCH_S3_CLIENT_DEFAULT_SECRET_KEY", "csk"),
            ]),
        )
        .unwrap();
        assert_eq!(client.access_key, "cak");
        assert_eq!(
            client.source,
            "SURCH_S3_CLIENT_DEFAULT_ACCESS_KEY / SURCH_S3_CLIENT_DEFAULT_SECRET_KEY"
        );
        let named = S3RepositoryConfig {
            client: Some("scw-prod".into()),
            ..base.clone()
        };
        assert_eq!(
            s3_client_env_name("scw-prod", "ACCESS_KEY"),
            "SURCH_S3_CLIENT_SCW_PROD_ACCESS_KEY"
        );
        assert_eq!(
            resolve_s3_credentials(
                &named,
                env(&[
                    ("SURCH_S3_CLIENT_DEFAULT_ACCESS_KEY", "cak"),
                    ("SURCH_S3_CLIENT_DEFAULT_SECRET_KEY", "csk"),
                ])
            ),
            None,
            "a named client never borrows the default client's keys"
        );
        let in_settings = S3RepositoryConfig {
            access_key: Some("s".into()),
            secret_key: Some("t".into()),
            ..base
        };
        assert_eq!(
            resolve_s3_credentials(&in_settings, env(&[("AWS_ACCESS_KEY_ID", "ak")]))
                .unwrap()
                .access_key,
            "s"
        );
    }

    #[test]
    fn byte_sizes_and_booleans_parse_like_es() {
        assert_eq!(parse_byte_size("100mb"), Some(100 << 20));
        assert_eq!(parse_byte_size("5MB"), Some(5 << 20));
        assert_eq!(parse_byte_size("1gb"), Some(1 << 30));
        assert_eq!(parse_byte_size("1024"), Some(1024));
        assert_eq!(parse_byte_size("12parsecs"), None);
        let settings = serde_json::json!({"a": true, "b": "false", "c": "yes"});
        assert_eq!(setting_bool(&settings, "a").unwrap(), Some(true));
        assert_eq!(setting_bool(&settings, "b").unwrap(), Some(false));
        assert!(setting_bool(&settings, "c").is_err());
        assert_eq!(setting_bool(&settings, "missing").unwrap(), None);
        let echoed = echo_settings(&serde_json::json!({
            "bucket": "b", "readonly": true, "access_key": "x", "secret_key": "y", "buffer_size": 5
        }));
        assert_eq!(
            echoed,
            serde_json::json!({"bucket": "b", "readonly": "true", "buffer_size": "5"})
        );
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
