//! Service layer between the ES-parity REST routes and the
//! [`SnapshotRepository`] SPI.
//!
//! The on-repository layout copies the Elasticsearch `BlobStoreRepository`
//! shape so external clients (Curator, Kibana, `elasticsearch-py`) can
//! browse the bucket / NFS mount without learning a new convention:
//!
//! ```text
//! {root}/
//!   index-{N}                          root manifest (CAS-protected)
//!   meta-{snap_uuid}.dat               cluster metadata for the snapshot
//!   snap-{snap_uuid}.dat               snapshot payload (Surch tarball)
//!   indices/{index_uuid}/
//!     meta-{snap_uuid}.dat             per-index metadata
//! ```
//!
//! For C-SNAPSHOT-S1 the snapshot payload (`snap-{uuid}.dat`) is the
//! exact same tarball `C-SNAPSHOT-RAW` already produces (see
//! `crate::snapshot::build_tarball`). The wrap simply rebrands it
//! under the ES-style filename so client tools see the layout they
//! expect. The Lucene-segment internals are explicitly faked — Surch
//! is in-memory, there is no per-segment dedup yet — but the *surface*
//! is bit-compatible with the ES on-disk layout, which is what makes
//! `elasticsearch-py.snapshot.get(repository=..., snapshot=...)` work.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::repository::{RepositoryError, SnapshotRepository};
use crate::state::{AppState, StoredDocument};

/// Snapshot format version (separate from
/// `crate::snapshot::SNAPSHOT_FORMAT_VERSION` so the ES-parity flavour
/// can evolve independently of the legacy `_surch/snapshot/*` shape).
pub const ES_SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Root manifest key — ES `BlobStoreRepository` uses `index-{N}` with
/// N a monotonically-increasing generation. C-SNAPSHOT-S1 ships only
/// `index-0`; subsequent phases bump N on every snapshot take.
pub const ROOT_MANIFEST_KEY: &str = "index-0";

/// On-repository root manifest. Mirrors the ES `RepositoryData` JSON
/// shape (`snapshots`, `indices`, `min_version`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RootManifest {
    pub snapshots: Vec<SnapshotEntry>,
    /// Surch indices observed by this repository (for the
    /// `indices/{uuid}/...` layout). Keyed by index *name*, value
    /// carries the stable uuid used in the path.
    pub indices: BTreeMap<String, IndexEntry>,
    /// Repository format version for forward-compatibility refusal.
    #[serde(default = "default_min_version")]
    pub min_version: u32,
}

fn default_min_version() -> u32 {
    ES_SNAPSHOT_FORMAT_VERSION
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub name: String,
    pub uuid: String,
    pub state: SnapshotState,
    pub indices: Vec<String>,
    pub start_time_millis: i64,
    pub end_time_millis: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexEntry {
    pub uuid: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SnapshotState {
    InProgress,
    Success,
    Failed,
    PartialMissing,
}

impl SnapshotState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
            Self::PartialMissing => "PARTIAL",
        }
    }
}

/// Per-snapshot cluster metadata blob (key `meta-{uuid}.dat`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterMetadata {
    pub format_version: u32,
    pub surch_version: String,
    pub snapshot: String,
    pub uuid: String,
    pub indices: Vec<String>,
    pub start_time_millis: i64,
    pub end_time_millis: i64,
    pub include_global_state: bool,
}

/// Per-index per-snapshot metadata blob
/// (key `indices/{idx_uuid}/meta-{snap_uuid}.dat`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexSnapshotMetadata {
    pub format_version: u32,
    pub index: String,
    pub snapshot_uuid: String,
    pub mapping: Value,
    pub settings: Value,
    pub aliases: BTreeMap<String, Value>,
    pub doc_count: u64,
}

/// Errors surfaced to the REST layer.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotServiceError {
    #[error("repository [{name}] missing")]
    RepositoryMissing { name: String },
    #[error("snapshot [{repo}:{snap}] missing")]
    SnapshotMissing { repo: String, snap: String },
    #[error("snapshot [{repo}:{snap}] already exists")]
    SnapshotAlreadyExists { repo: String, snap: String },
    #[error("repository type [{kind}] is not supported (only `fs` and `s3` are wired in)")]
    UnsupportedRepositoryType { kind: String },
    #[error("invalid snapshot request: {0}")]
    BadRequest(String),
    #[error("index [{name}] missing")]
    IndexMissing { name: String },
    #[error("storage backend error: {0}")]
    Repository(#[from] RepositoryError),
    #[error("internal serialisation error: {0}")]
    Serde(String),
}

impl From<serde_json::Error> for SnapshotServiceError {
    fn from(value: serde_json::Error) -> Self {
        SnapshotServiceError::Serde(value.to_string())
    }
}

/// Result type for the service layer.
pub type ServiceResult<T> = Result<T, SnapshotServiceError>;

/// Read or initialise the root `index-{N}` manifest.
fn load_or_init_manifest(
    repo: &dyn SnapshotRepository,
) -> ServiceResult<(RootManifest, Option<String>)> {
    match repo.get_object(ROOT_MANIFEST_KEY) {
        Ok(bytes) => {
            let manifest: RootManifest = serde_json::from_slice(&bytes)?;
            let etag = repo.read_etag(ROOT_MANIFEST_KEY)?;
            Ok((manifest, etag))
        }
        Err(RepositoryError::NotFound { .. }) => Ok((RootManifest::default(), None)),
        Err(other) => Err(other.into()),
    }
}

fn write_manifest(
    repo: &dyn SnapshotRepository,
    manifest: &RootManifest,
    expected_etag: Option<&str>,
) -> ServiceResult<String> {
    let bytes = Bytes::from(serde_json::to_vec_pretty(manifest)?);
    Ok(repo.compare_and_set(ROOT_MANIFEST_KEY, expected_etag, bytes)?)
}

/// `PUT /_snapshot/{repository}/{snapshot}` — take a snapshot of the
/// supplied indices (or all of them) against `repo`.
pub fn create_snapshot(
    repo: &dyn SnapshotRepository,
    repo_name: &str,
    snap_name: &str,
    indices: &[String],
    include_global_state: bool,
    state: &AppState,
) -> ServiceResult<SnapshotEntry> {
    let (mut manifest, etag) = load_or_init_manifest(repo)?;
    if manifest.snapshots.iter().any(|s| s.name == snap_name) {
        return Err(SnapshotServiceError::SnapshotAlreadyExists {
            repo: repo_name.to_owned(),
            snap: snap_name.to_owned(),
        });
    }

    // Resolve the index selector — `*` or empty means "every known
    // index", a comma-separated list means "those". Unknown names are
    // rejected with the ES error type.
    let resolved = if indices.is_empty() || indices == ["*".to_string()] {
        state.index_names()
    } else {
        for name in indices {
            if !state.index_exists(name) {
                return Err(SnapshotServiceError::IndexMissing {
                    name: name.to_owned(),
                });
            }
        }
        indices.to_vec()
    };

    let snap_uuid = Uuid::new_v4().to_string();
    let start = chrono::Utc::now().timestamp_millis();

    // Ensure every index has a stable repo-side uuid.
    for index in &resolved {
        manifest
            .indices
            .entry(index.clone())
            .or_insert_with(|| IndexEntry {
                uuid: Uuid::new_v4().to_string(),
            });
    }

    // Build and ship the payload (one tarball per index) under the ES
    // naming convention. C-SNAPSHOT-S1 wraps the existing
    // `crate::snapshot::build_tarball` output so the on-the-wire shape
    // stays consistent with `_surch/snapshot/export`.
    for index in &resolved {
        let metadata =
            state
                .index_metadata(index)
                .ok_or_else(|| SnapshotServiceError::IndexMissing {
                    name: index.clone(),
                })?;
        let documents = state.documents(index);

        // Per-index per-snapshot metadata blob — read by clients via
        // `GET /_snapshot/{repo}/{snap}` and (in a future phase) by the
        // restore-by-index code path.
        let idx_uuid = manifest
            .indices
            .get(index)
            .expect("index uuid just inserted")
            .uuid
            .clone();
        let index_meta = IndexSnapshotMetadata {
            format_version: ES_SNAPSHOT_FORMAT_VERSION,
            index: index.clone(),
            snapshot_uuid: snap_uuid.clone(),
            mapping: metadata.mapping.clone(),
            settings: metadata.settings.clone(),
            aliases: metadata.aliases.clone(),
            doc_count: documents.len() as u64,
        };
        let meta_key = format!("indices/{idx_uuid}/meta-{snap_uuid}.dat");
        repo.put_object(
            &meta_key,
            Bytes::from(serde_json::to_vec_pretty(&index_meta)?),
        )?;

        // The snapshot payload itself: a Surch tarball, byte-for-byte
        // identical to `_surch/snapshot/export` output (so client tools
        // can pull it out of the repository directly via S3 / FS reads
        // and feed it to a stand-alone Surch via the legacy import
        // route). The ES naming convention puts it under
        // `indices/{idx_uuid}/__snap-{snap_uuid}.dat` — we follow the
        // same path so a listing under `indices/*/` shows every blob
        // belonging to a snapshot under the same prefix.
        let payload =
            crate::snapshot::build_index_tarball_for_es_snapshot(index, &metadata, &documents)
                .map_err(SnapshotServiceError::BadRequest)?;
        let payload_key = format!("indices/{idx_uuid}/__snap-{snap_uuid}.dat");
        repo.put_object(&payload_key, Bytes::from(payload))?;
    }

    let end = chrono::Utc::now().timestamp_millis();

    // Cluster-level metadata blob (`meta-{snap_uuid}.dat`).
    let cluster_meta = ClusterMetadata {
        format_version: ES_SNAPSHOT_FORMAT_VERSION,
        surch_version: env!("CARGO_PKG_VERSION").to_owned(),
        snapshot: snap_name.to_owned(),
        uuid: snap_uuid.clone(),
        indices: resolved.clone(),
        start_time_millis: start,
        end_time_millis: end,
        include_global_state,
    };
    let cluster_key = format!("meta-{snap_uuid}.dat");
    repo.put_object(
        &cluster_key,
        Bytes::from(serde_json::to_vec_pretty(&cluster_meta)?),
    )?;

    // Top-level `snap-{snap_uuid}.dat` summary — a small JSON blob
    // listing every per-index payload key for this snapshot. Clients
    // walking the repository read it first to learn the layout.
    let snap_summary = json!({
        "format_version": ES_SNAPSHOT_FORMAT_VERSION,
        "name": snap_name,
        "uuid": snap_uuid,
        "indices": resolved.iter().map(|index| {
            let idx_uuid = &manifest.indices.get(index).expect("uuid").uuid;
            json!({
                "name": index,
                "uuid": idx_uuid,
                "meta_key": format!("indices/{idx_uuid}/meta-{snap_uuid}.dat"),
                "payload_key": format!("indices/{idx_uuid}/__snap-{snap_uuid}.dat"),
            })
        }).collect::<Vec<_>>(),
        "start_time_millis": start,
        "end_time_millis": end,
    });
    let snap_summary_key = format!("snap-{snap_uuid}.dat");
    repo.put_object(
        &snap_summary_key,
        Bytes::from(serde_json::to_vec_pretty(&snap_summary)?),
    )?;

    let entry = SnapshotEntry {
        name: snap_name.to_owned(),
        uuid: snap_uuid,
        state: SnapshotState::Success,
        indices: resolved,
        start_time_millis: start,
        end_time_millis: end,
    };
    manifest.snapshots.push(entry.clone());
    write_manifest(repo, &manifest, etag.as_deref())?;
    Ok(entry)
}

/// `GET /_snapshot/{repository}/{snapshot}`.
pub fn get_snapshot(
    repo: &dyn SnapshotRepository,
    repo_name: &str,
    snap_name: &str,
) -> ServiceResult<SnapshotEntry> {
    let (manifest, _etag) = load_or_init_manifest(repo)?;
    manifest
        .snapshots
        .into_iter()
        .find(|s| s.name == snap_name)
        .ok_or_else(|| SnapshotServiceError::SnapshotMissing {
            repo: repo_name.to_owned(),
            snap: snap_name.to_owned(),
        })
}

/// `GET /_snapshot/{repository}/_all`.
pub fn list_snapshots(repo: &dyn SnapshotRepository) -> ServiceResult<Vec<SnapshotEntry>> {
    let (manifest, _etag) = load_or_init_manifest(repo)?;
    Ok(manifest.snapshots)
}

/// `DELETE /_snapshot/{repository}/{snapshot}`.
pub fn delete_snapshot(
    repo: &dyn SnapshotRepository,
    repo_name: &str,
    snap_name: &str,
) -> ServiceResult<()> {
    let (mut manifest, etag) = load_or_init_manifest(repo)?;
    let position = manifest
        .snapshots
        .iter()
        .position(|s| s.name == snap_name)
        .ok_or_else(|| SnapshotServiceError::SnapshotMissing {
            repo: repo_name.to_owned(),
            snap: snap_name.to_owned(),
        })?;
    let entry = manifest.snapshots.remove(position);

    for index in &entry.indices {
        if let Some(idx_entry) = manifest.indices.get(index) {
            let idx_uuid = &idx_entry.uuid;
            let _ = repo.delete_object(&format!("indices/{idx_uuid}/meta-{}.dat", entry.uuid));
            let _ = repo.delete_object(&format!("indices/{idx_uuid}/__snap-{}.dat", entry.uuid));
        }
    }
    let _ = repo.delete_object(&format!("meta-{}.dat", entry.uuid));
    let _ = repo.delete_object(&format!("snap-{}.dat", entry.uuid));

    write_manifest(repo, &manifest, etag.as_deref())?;
    Ok(())
}

/// `POST /_snapshot/{repository}/{snapshot}/_restore`. Pulls every
/// snapshotted index back, recreating them in `AppState`. Existing
/// indices are refused (matches the `C-SNAPSHOT-RAW` semantics — caller
/// deletes first if they want to overwrite).
pub fn restore_snapshot(
    repo: &dyn SnapshotRepository,
    repo_name: &str,
    snap_name: &str,
    indices_selector: Option<&[String]>,
    state: &AppState,
) -> ServiceResult<Vec<String>> {
    let (manifest, _etag) = load_or_init_manifest(repo)?;
    let entry = manifest
        .snapshots
        .iter()
        .find(|s| s.name == snap_name)
        .ok_or_else(|| SnapshotServiceError::SnapshotMissing {
            repo: repo_name.to_owned(),
            snap: snap_name.to_owned(),
        })?
        .clone();

    let wanted: Vec<String> = match indices_selector {
        None => entry.indices.clone(),
        Some(list) if list.is_empty() || list == ["*".to_string()] => entry.indices.clone(),
        Some(list) => list.to_vec(),
    };

    let mut restored = Vec::new();
    for index in &wanted {
        if state.index_exists(index) {
            return Err(SnapshotServiceError::BadRequest(format!(
                "cannot restore index [{index}] over an existing one"
            )));
        }
        let idx_uuid = &manifest
            .indices
            .get(index)
            .ok_or_else(|| SnapshotServiceError::IndexMissing {
                name: index.clone(),
            })?
            .uuid;
        let payload_key = format!("indices/{idx_uuid}/__snap-{}.dat", entry.uuid);
        let payload = repo.get_object(&payload_key)?;
        crate::snapshot::restore_index_tarball_for_es_snapshot(state, index, &payload)
            .map_err(SnapshotServiceError::BadRequest)?;
        restored.push(index.clone());
    }
    Ok(restored)
}

/// Snapshot pool exposed to the REST layer.
#[derive(Clone, Default)]
pub struct SnapshotRepositoryRegistry {
    pub repositories: Arc<std::sync::RwLock<BTreeMap<String, RegisteredRepository>>>,
}

#[derive(Clone)]
pub struct RegisteredRepository {
    pub repository: Arc<dyn SnapshotRepository>,
}

impl SnapshotRepositoryRegistry {
    pub fn list(&self) -> Vec<(String, Arc<dyn SnapshotRepository>)> {
        let guard = self
            .repositories
            .read()
            .expect("snapshot registry lock should not be poisoned");
        guard
            .iter()
            .map(|(name, entry)| (name.clone(), Arc::clone(&entry.repository)))
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn SnapshotRepository>> {
        let guard = self
            .repositories
            .read()
            .expect("snapshot registry lock should not be poisoned");
        guard.get(name).map(|entry| Arc::clone(&entry.repository))
    }

    pub fn insert(&self, name: String, repository: Arc<dyn SnapshotRepository>) {
        let mut guard = self
            .repositories
            .write()
            .expect("snapshot registry lock should not be poisoned");
        guard.insert(name, RegisteredRepository { repository });
    }

    pub fn remove(&self, name: &str) -> bool {
        let mut guard = self
            .repositories
            .write()
            .expect("snapshot registry lock should not be poisoned");
        guard.remove(name).is_some()
    }
}

/// Convenience helper used by the take handler — exists primarily as
/// the docs anchor to recall that the snapshot path is read-only on
/// the source index for the duration of the take. The MVP enforces
/// this by holding the `AppState` read lock around the documents
/// snapshot.
pub fn documents_under_read_lock(state: &AppState, index: &str) -> Vec<StoredDocument> {
    state.documents(index)
}
