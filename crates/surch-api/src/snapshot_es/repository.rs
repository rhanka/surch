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
use std::sync::Mutex;
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
            file.write_all(bytes).map_err(|source| RepositoryError::Io {
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
        repo.put_object("foo", Bytes::from_static(b"hello")).unwrap();
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
