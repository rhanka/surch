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
//!   snap-{snap_uuid}.dat               snapshot summary
//!   indices/{index_uuid}/
//!     meta-{snap_uuid}.dat             per-index metadata (v1 or v2)
//!     __snap-{snap_uuid}.dat           v1 only: logical payload (tarball)
//!   blobs/<blake3>                     v2 only: content-addressed files
//! ```
//!
//! Two per-index payload formats coexist:
//!
//! - **v1 logical** (`format_version` 1): the `crate::snapshot` tarball of
//!   mapping, settings, aliases and every `_source`. Restore re-ingests the
//!   documents (hours at the matchID 29.3 M scale on one core). Written when
//!   the node runs without `SURCH_DATA_DIR`, always readable.
//! - **v2 segments** (`format_version` 2, L6.2, `docs/ops/snapshot-v2.md`):
//!   the sealed L6 files of the index directory uploaded as is, see
//!   [`super::segments`]. Restore downloads them and opens the index without
//!   re-analysing a document. Written whenever `SURCH_DATA_DIR` is set;
//!   restoring one requires `SURCH_DATA_DIR` on the restoring node.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::rename::{filter_indices, renamed_indices, RenameError};
use super::repository::{RepositoryError, SnapshotRepository};
use super::segments::{self, SegmentSnapshot, UploadStats, BLOB_PREFIX};
use crate::state::{AppState, SnapshotStateError, StoredDocument};

/// Snapshot format version (separate from
/// `crate::snapshot::SNAPSHOT_FORMAT_VERSION` so the ES-parity flavour
/// can evolve independently of the legacy `_surch/snapshot/*` shape).
/// Root manifest / cluster metadata version: unchanged by L6.2 (the v2
/// payload is announced per index).
pub const ES_SNAPSHOT_FORMAT_VERSION: u32 = 1;
/// Per-index metadata version of a v2 (segment files) index snapshot.
pub const ES_SNAPSHOT_SEGMENTS_FORMAT_VERSION: u32 = 2;

/// `SnapshotEntry::format` / response `snapshot.surch.format` values.
pub const FORMAT_LOGICAL_V1: &str = "logical-v1";
pub const FORMAT_SEGMENTS_V2: &str = "segments-v2";

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
    /// L6.2: payload format of the snapshot ([`FORMAT_SEGMENTS_V2`] or
    /// [`FORMAT_LOGICAL_V1`]); absent in entries written before L6.2
    /// (= logical v1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl SnapshotEntry {
    pub fn format(&self) -> &str {
        self.format.as_deref().unwrap_or(FORMAT_LOGICAL_V1)
    }
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
    /// L6.2 (`format_version` 2): the segment files of the index directory.
    /// Absent = v1 logical payload `__snap-{snap_uuid}.dat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segments: Option<SegmentSnapshot>,
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
    /// ES `IndexNotFoundException` (`no such index [x]`).
    #[error("no such index [{name}]")]
    NoSuchIndex { name: String },
    /// ES `RepositoryException` on a `readonly` repository
    /// (`SnapshotsService.createSnapshot`, ES 8.6.1).
    #[error("[{repo}] {reason}")]
    ReadonlyRepository { repo: String, reason: String },
    /// ES `SnapshotRestoreException`: `[repo:snap/uuid] reason`.
    #[error("[{repo}:{snap}/{uuid}] {reason}")]
    Restore {
        repo: String,
        snap: String,
        uuid: String,
        reason: String,
    },
    /// L6.2: a snapshotted file or a downloaded blob does not match its
    /// recorded length / blake3 (fail closed).
    #[error("snapshot integrity check failed: {0}")]
    Integrity(String),
    /// L6.2: persistence error on the local data directory.
    #[error("data directory error: {0}")]
    DataDir(String),
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

fn index_meta_key(idx_uuid: &str, snap_uuid: &str) -> String {
    format!("indices/{idx_uuid}/meta-{snap_uuid}.dat")
}

fn payload_key(idx_uuid: &str, snap_uuid: &str) -> String {
    format!("indices/{idx_uuid}/__snap-{snap_uuid}.dat")
}

/// What a snapshot take did, beyond the ES entry (`snapshot.surch` in the
/// create response).
#[derive(Clone, Debug, Serialize)]
pub struct CreateReport {
    /// [`FORMAT_SEGMENTS_V2`] or [`FORMAT_LOGICAL_V1`].
    pub format: String,
    /// Why the logical v1 format was used, when it was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(flatten)]
    pub upload: UploadStats,
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
    create_snapshot_with_report(
        repo,
        repo_name,
        snap_name,
        indices,
        include_global_state,
        state,
    )
    .map(|(entry, _)| entry)
}

/// [`create_snapshot`] plus the format / upload report.
///
/// Format choice (L6.2): with `SURCH_DATA_DIR` every index is snapshotted
/// as its sealed segment files (v2); without it, as the logical v1 payload,
/// and the report says so (`format: logical-v1`, `reason`) — the snapshot
/// stays restorable, by re-ingestion.
pub fn create_snapshot_with_report(
    repo: &dyn SnapshotRepository,
    repo_name: &str,
    snap_name: &str,
    indices: &[String],
    include_global_state: bool,
    state: &AppState,
) -> ServiceResult<(SnapshotEntry, CreateReport)> {
    if repo.readonly() {
        return Err(SnapshotServiceError::ReadonlyRepository {
            repo: repo_name.to_owned(),
            reason: "cannot create snapshot in a readonly repository".to_owned(),
        });
    }
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

    let segments_mode = state.data_dir().is_some();
    let mut report = CreateReport {
        format: if segments_mode {
            FORMAT_SEGMENTS_V2
        } else {
            FORMAT_LOGICAL_V1
        }
        .to_owned(),
        reason: (!segments_mode).then(|| {
            "SURCH_DATA_DIR is not set on this node: indices were snapshotted as documents \
             (logical v1) and a restore re-ingests them"
                .to_owned()
        }),
        upload: UploadStats::default(),
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

    for index in &resolved {
        let idx_uuid = manifest
            .indices
            .get(index)
            .expect("index uuid just inserted")
            .uuid
            .clone();
        let index_meta = if segments_mode {
            snapshot_index_segments(repo, state, index, &snap_uuid, &mut report.upload)?
        } else {
            snapshot_index_logical(repo, state, index, &idx_uuid, &snap_uuid)?
        };
        repo.put_object(
            &index_meta_key(&idx_uuid, &snap_uuid),
            Bytes::from(serde_json::to_vec_pretty(&index_meta)?),
        )?;
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
    repo.put_object(
        &format!("meta-{snap_uuid}.dat"),
        Bytes::from(serde_json::to_vec_pretty(&cluster_meta)?),
    )?;

    // Top-level `snap-{snap_uuid}.dat` summary — a small JSON blob
    // listing every per-index metadata key for this snapshot. Clients
    // walking the repository read it first to learn the layout.
    let snap_summary = json!({
        "format_version": ES_SNAPSHOT_FORMAT_VERSION,
        "surch_format": report.format,
        "name": snap_name,
        "uuid": snap_uuid,
        "indices": resolved.iter().map(|index| {
            let idx_uuid = &manifest.indices.get(index).expect("uuid").uuid;
            let mut entry = json!({
                "name": index,
                "uuid": idx_uuid,
                "meta_key": index_meta_key(idx_uuid, &snap_uuid),
            });
            if !segments_mode {
                entry["payload_key"] = Value::String(payload_key(idx_uuid, &snap_uuid));
            }
            entry
        }).collect::<Vec<_>>(),
        "start_time_millis": start,
        "end_time_millis": end,
    });
    repo.put_object(
        &format!("snap-{snap_uuid}.dat"),
        Bytes::from(serde_json::to_vec_pretty(&snap_summary)?),
    )?;

    let entry = SnapshotEntry {
        name: snap_name.to_owned(),
        uuid: snap_uuid,
        state: SnapshotState::Success,
        indices: resolved,
        start_time_millis: start,
        end_time_millis: end,
        format: Some(report.format.clone()),
    };
    manifest.snapshots.push(entry.clone());
    write_manifest(repo, &manifest, etag.as_deref())?;
    Ok((entry, report))
}

/// v1: the logical tarball (`crate::snapshot` shape) + metadata.
fn snapshot_index_logical(
    repo: &dyn SnapshotRepository,
    state: &AppState,
    index: &str,
    idx_uuid: &str,
    snap_uuid: &str,
) -> ServiceResult<IndexSnapshotMetadata> {
    let metadata =
        state
            .index_metadata(index)
            .ok_or_else(|| SnapshotServiceError::IndexMissing {
                name: index.to_owned(),
            })?;
    let documents = state.documents(index);
    let payload =
        crate::snapshot::build_index_tarball_for_es_snapshot(index, &metadata, &documents)
            .map_err(SnapshotServiceError::BadRequest)?;
    repo.put_object(&payload_key(idx_uuid, snap_uuid), Bytes::from(payload))?;
    Ok(IndexSnapshotMetadata {
        format_version: ES_SNAPSHOT_FORMAT_VERSION,
        index: index.to_owned(),
        snapshot_uuid: snap_uuid.to_owned(),
        mapping: metadata.mapping,
        settings: metadata.settings,
        aliases: metadata.aliases,
        doc_count: documents.len() as u64,
        segments: None,
    })
}

fn state_error(error: SnapshotStateError) -> SnapshotServiceError {
    match error {
        SnapshotStateError::IndexMissing(name) => SnapshotServiceError::IndexMissing { name },
        other => SnapshotServiceError::DataDir(other.to_string()),
    }
}

/// v2: commit + pin the index generation, upload its files as blobs.
fn snapshot_index_segments(
    repo: &dyn SnapshotRepository,
    state: &AppState,
    index: &str,
    snap_uuid: &str,
    stats: &mut UploadStats,
) -> ServiceResult<IndexSnapshotMetadata> {
    let pinned = state.pin_index_for_snapshot(index).map_err(state_error)?;
    let uploaded = segments::upload_pinned(repo, &pinned, stats);
    let _ = std::fs::remove_dir_all(&pinned.dir);
    let segments = uploaded?;
    Ok(IndexSnapshotMetadata {
        format_version: ES_SNAPSHOT_SEGMENTS_FORMAT_VERSION,
        index: index.to_owned(),
        snapshot_uuid: snap_uuid.to_owned(),
        mapping: pinned.metadata.mapping,
        settings: pinned.metadata.settings,
        aliases: pinned.metadata.aliases,
        doc_count: pinned.doc_count,
        segments: Some(segments),
    })
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

fn read_index_meta(
    repo: &dyn SnapshotRepository,
    idx_uuid: &str,
    snap_uuid: &str,
) -> ServiceResult<IndexSnapshotMetadata> {
    let bytes = repo.get_object(&index_meta_key(idx_uuid, snap_uuid))?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// `DELETE /_snapshot/{repository}/{snapshot}`. v2 blobs are shared between
/// snapshots: after the manifest no longer lists the snapshot, every blob
/// no remaining snapshot references is deleted (best effort — a leftover
/// blob is only wasted space, never a dangling reference).
pub fn delete_snapshot(
    repo: &dyn SnapshotRepository,
    repo_name: &str,
    snap_name: &str,
) -> ServiceResult<()> {
    if repo.readonly() {
        return Err(SnapshotServiceError::ReadonlyRepository {
            repo: repo_name.to_owned(),
            reason: "cannot delete snapshot from a readonly repository".to_owned(),
        });
    }
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
    // Commit point first: the snapshot disappears atomically, then its
    // objects are removed.
    write_manifest(repo, &manifest, etag.as_deref())?;

    let mut had_segments = false;
    for index in &entry.indices {
        if let Some(idx_entry) = manifest.indices.get(index) {
            let idx_uuid = &idx_entry.uuid;
            if let Ok(meta) = read_index_meta(repo, idx_uuid, &entry.uuid) {
                had_segments |= meta.segments.is_some();
            }
            let _ = repo.delete_object(&index_meta_key(idx_uuid, &entry.uuid));
            let _ = repo.delete_object(&payload_key(idx_uuid, &entry.uuid));
        }
    }
    let _ = repo.delete_object(&format!("meta-{}.dat", entry.uuid));
    let _ = repo.delete_object(&format!("snap-{}.dat", entry.uuid));

    if had_segments {
        if let Err(error) = collect_unreferenced_blobs(repo, &manifest) {
            tracing::warn!(target: "surch_api::snapshot", repository = repo_name, %error,
                "could not garbage-collect unreferenced snapshot blobs");
        }
    }
    Ok(())
}

/// Delete every `blobs/*` object no snapshot of `manifest` references.
/// Refuses to delete anything when a remaining snapshot's metadata cannot
/// be read (it might reference any blob).
pub fn collect_unreferenced_blobs(
    repo: &dyn SnapshotRepository,
    manifest: &RootManifest,
) -> ServiceResult<u64> {
    let mut referenced = BTreeSet::new();
    for snapshot in &manifest.snapshots {
        for index in &snapshot.indices {
            let Some(idx_entry) = manifest.indices.get(index) else {
                continue;
            };
            let meta = read_index_meta(repo, &idx_entry.uuid, &snapshot.uuid)?;
            if let Some(segments) = meta.segments {
                referenced.extend(segments.blob_keys());
            }
        }
    }
    let mut removed = 0;
    for key in repo.list_objects(BLOB_PREFIX)? {
        if !referenced.contains(&key) && repo.delete_object(&key).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// `_restore` request options (ES `RestoreSnapshotRequest`).
#[derive(Clone, Debug)]
pub struct RestoreOptions {
    pub indices: Vec<String>,
    pub ignore_unavailable: bool,
    pub include_global_state: bool,
    pub include_aliases: bool,
    pub partial: bool,
    pub rename_pattern: Option<String>,
    pub rename_replacement: Option<String>,
    /// Flattened `index_settings` (`index.number_of_replicas` -> value).
    pub index_settings: BTreeMap<String, Value>,
    /// Normalised `ignore_index_settings` keys.
    pub ignore_index_settings: Vec<String>,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            indices: Vec::new(),
            ignore_unavailable: false,
            include_global_state: false,
            include_aliases: true,
            partial: false,
            rename_pattern: None,
            rename_replacement: None,
            index_settings: BTreeMap::new(),
            ignore_index_settings: Vec::new(),
        }
    }
}

/// ES `RestoreService.UNMODIFIABLE_SETTINGS` (8.6.1).
const UNMODIFIABLE_SETTINGS: &[&str] = &[
    "index.number_of_shards",
    "index.version.created",
    "index.uuid",
    "index.creation_date",
    "index.history.uuid",
];
/// ES `RestoreService.UNREMOVABLE_SETTINGS`: the above plus replicas.
const UNREMOVABLE_EXTRA: &[&str] = &["index.number_of_replicas", "index.auto_expand_replicas"];

/// Flatten a settings object into `index.`-prefixed dotted keys (ES
/// `Settings.builder().put(...).normalizePrefix("index.")`).
pub fn flatten_index_settings(value: &Value) -> Result<BTreeMap<String, Value>, String> {
    fn walk(prefix: &str, value: &Value, out: &mut BTreeMap<String, Value>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    walk(&path, child, out);
                }
            }
            other => {
                out.insert(prefix.to_owned(), other.clone());
            }
        }
    }
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::Object(_) => {
            let mut flat = BTreeMap::new();
            walk("", value, &mut flat);
            Ok(flat
                .into_iter()
                .map(|(key, value)| (normalize_index_key(&key), value))
                .collect())
        }
        _ => Err("[index_settings] must be an object".to_owned()),
    }
}

pub fn normalize_index_key(key: &str) -> String {
    if key.starts_with("index.") {
        key.to_owned()
    } else {
        format!("index.{key}")
    }
}

/// Apply ES `index_settings` / `ignore_index_settings` to a Surch settings
/// object. Surch keeps the settings object as declared at creation, either
/// nested under `index` or flat; the change goes where the index already
/// keeps its `index.*` keys, and any copy in the other place is removed.
pub fn apply_index_settings(
    settings: &mut Value,
    changes: &BTreeMap<String, Value>,
    removals: &[String],
) {
    if !settings.is_object() {
        *settings = Value::Object(Map::new());
    }
    let nested = matches!(settings.get("index"), Some(Value::Object(_)));
    let remove_path = |root: &mut Value, path: &[&str]| {
        let mut node = root;
        for part in &path[..path.len() - 1] {
            match node.get_mut(*part) {
                Some(child) => node = child,
                None => return,
            }
        }
        if let Some(map) = node.as_object_mut() {
            map.remove(path[path.len() - 1]);
        }
    };
    let strip = |key: &str| key.strip_prefix("index.").unwrap_or(key).to_owned();
    for key in removals.iter().chain(changes.keys()) {
        let short = strip(key);
        let parts: Vec<&str> = short.split('.').collect();
        remove_path(settings, &parts);
        if let Some(index) = settings.get_mut("index") {
            remove_path(index, &parts);
        }
        // Also the flat dotted spelling (`"index.number_of_replicas": 0`).
        if let Some(map) = settings.as_object_mut() {
            map.remove(key);
            map.remove(&short);
        }
    }
    for (key, value) in changes {
        let short = strip(key);
        let parts: Vec<&str> = short.split('.').collect();
        let mut node = if nested {
            settings.get_mut("index").expect("checked above")
        } else {
            &mut *settings
        };
        for part in &parts[..parts.len() - 1] {
            let map = node.as_object_mut().expect("settings nodes are objects");
            node = map
                .entry((*part).to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !node.is_object() {
                *node = Value::Object(Map::new());
            }
        }
        node.as_object_mut()
            .expect("settings nodes are objects")
            .insert(parts[parts.len() - 1].to_owned(), value.clone());
    }
}

/// Outcome of a restore (ES `RestoreInfo`).
#[derive(Clone, Debug)]
pub struct RestoreOutcome {
    pub snapshot: String,
    /// Restored index names (after renaming).
    pub indices: Vec<String>,
    /// Restored document count per restored index.
    pub doc_counts: Vec<u64>,
}

/// Prepared restore: validated selection, ready to run (possibly in the
/// background for `wait_for_completion=false`).
#[derive(Clone, Debug)]
pub struct RestorePlan {
    repo_name: String,
    entry: SnapshotEntry,
    /// `(snapshot index, restored name, index uuid in the repository)`.
    targets: Vec<(String, String, String)>,
    options: RestoreOptions,
}

impl RestorePlan {
    pub fn restored_indices(&self) -> Vec<String> {
        self.targets.iter().map(|(_, t, _)| t.clone()).collect()
    }
}

fn restore_error(
    repo: &str,
    entry: &SnapshotEntry,
    reason: impl Into<String>,
) -> SnapshotServiceError {
    SnapshotServiceError::Restore {
        repo: repo.to_owned(),
        snap: entry.name.clone(),
        uuid: entry.uuid.clone(),
        reason: reason.into(),
    }
}

/// Validate a `_restore` request against the snapshot and the cluster,
/// as `RestoreService.restoreSnapshot` does before changing anything.
pub fn plan_restore(
    repo: &dyn SnapshotRepository,
    repo_name: &str,
    snap_name: &str,
    options: &RestoreOptions,
    state: &AppState,
) -> ServiceResult<RestorePlan> {
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

    for key in options.index_settings.keys() {
        if UNMODIFIABLE_SETTINGS.contains(&key.as_str()) {
            return Err(restore_error(
                repo_name,
                &entry,
                format!("cannot modify setting [{key}] on restore"),
            ));
        }
    }
    for key in &options.ignore_index_settings {
        if UNMODIFIABLE_SETTINGS.contains(&key.as_str())
            || UNREMOVABLE_EXTRA.contains(&key.as_str())
        {
            return Err(restore_error(
                repo_name,
                &entry,
                format!("cannot remove setting [{key}] on restore"),
            ));
        }
    }

    let selected = filter_indices(&entry.indices, &options.indices, options.ignore_unavailable)
        .map_err(|error| match error {
            RenameError::IndexNotFound(name) => SnapshotServiceError::NoSuchIndex { name },
            RenameError::Restore(reason) => restore_error(repo_name, &entry, reason),
        })?;
    let renamed = renamed_indices(
        &selected,
        options.rename_pattern.as_deref(),
        options.rename_replacement.as_deref(),
    )
    .map_err(|error| match error {
        RenameError::IndexNotFound(name) => SnapshotServiceError::NoSuchIndex { name },
        RenameError::Restore(reason) => restore_error(repo_name, &entry, reason),
    })?;

    let mut targets = Vec::with_capacity(renamed.len());
    for (source, target) in renamed {
        if let Err(error) = crate::index::validate_index_name(&target) {
            return Err(restore_error(
                repo_name,
                &entry,
                format!(
                    "invalid restored index name [{target}]: {}",
                    error.error.reason
                ),
            ));
        }
        if state.index_exists(&target) {
            return Err(restore_error(
                repo_name,
                &entry,
                format!(
                    "cannot restore index [{target}] because an open index with same name already \
                     exists in the cluster. Either close or delete the existing index or restore \
                     the index under a different name by providing a rename pattern and \
                     replacement name"
                ),
            ));
        }
        if state.index_or_alias_exists(&target) {
            return Err(restore_error(
                repo_name,
                &entry,
                format!(
                    "cannot rename index [{source}] into [{target}] because of conflict with an \
                     alias with the same name"
                ),
            ));
        }
        let idx_uuid = manifest
            .indices
            .get(&source)
            .ok_or_else(|| SnapshotServiceError::IndexMissing {
                name: source.clone(),
            })?
            .uuid
            .clone();
        targets.push((source, target, idx_uuid));
    }
    Ok(RestorePlan {
        repo_name: repo_name.to_owned(),
        entry,
        targets,
        options: options.clone(),
    })
}

/// Run a validated restore. Each index is restored completely or not at
/// all; indices restored before a failure stay (ES semantics).
pub fn execute_restore(
    repo: &dyn SnapshotRepository,
    plan: &RestorePlan,
    state: &AppState,
) -> ServiceResult<RestoreOutcome> {
    let mut indices = Vec::with_capacity(plan.targets.len());
    let mut doc_counts = Vec::with_capacity(plan.targets.len());
    for (source, target, idx_uuid) in &plan.targets {
        let meta = read_index_meta(repo, idx_uuid, &plan.entry.uuid)?;
        let aliases = if plan.options.include_aliases {
            meta.aliases.clone()
        } else {
            BTreeMap::new()
        };
        let changes = plan.options.index_settings.clone();
        let removals = plan.options.ignore_index_settings.clone();
        let patch = move |settings: &mut Value| {
            apply_index_settings(settings, &changes, &removals);
            Ok(())
        };
        let count = match &meta.segments {
            Some(segments) => {
                restore_index_segments(repo, plan, state, segments, target, patch, aliases)?
            }
            None => {
                let payload = repo.get_object(&payload_key(idx_uuid, &plan.entry.uuid))?;
                crate::snapshot::restore_index_tarball_for_es_snapshot(
                    state,
                    target,
                    &payload,
                    patch,
                    plan.options.include_aliases,
                )
                .map_err(|reason| restore_error(&plan.repo_name, &plan.entry, reason))?
            }
        };
        tracing::info!(target: "surch_api::snapshot", snapshot = %plan.entry.name,
            index = %source, restored = %target, documents = count,
            format = if meta.segments.is_some() { FORMAT_SEGMENTS_V2 } else { FORMAT_LOGICAL_V1 },
            "index restored");
        indices.push(target.clone());
        doc_counts.push(count);
    }
    Ok(RestoreOutcome {
        snapshot: plan.entry.name.clone(),
        indices,
        doc_counts,
    })
}

/// v2: download into a staging directory, verify, open, go live.
fn restore_index_segments(
    repo: &dyn SnapshotRepository,
    plan: &RestorePlan,
    state: &AppState,
    segments: &SegmentSnapshot,
    target: &str,
    patch: impl FnOnce(&mut Value) -> Result<(), String>,
    aliases: BTreeMap<String, Value>,
) -> ServiceResult<u64> {
    let (root, dir) = state
        .restore_staging_dir(target)
        .map_err(|error| match error {
            SnapshotStateError::NoDataDir => restore_error(
                &plan.repo_name,
                &plan.entry,
                "this snapshot holds segment files (format segments-v2): restoring it requires \
             SURCH_DATA_DIR on this node",
            ),
            other => state_error(other),
        })?;
    let result = segments::download_into(repo, segments, &dir).and_then(|()| {
        state
            .install_restored_index(&dir, target, patch, aliases)
            .map_err(|error| match error {
                SnapshotStateError::IndexExists(_) | SnapshotStateError::AliasConflict(_) => {
                    restore_error(&plan.repo_name, &plan.entry, error.to_string())
                }
                SnapshotStateError::AliasIsIndex(alias) => restore_error(
                    &plan.repo_name,
                    &plan.entry,
                    format!("alias [{alias}] of restored index [{target}] conflicts with an existing index"),
                ),
                other => state_error(other),
            })
    });
    let _ = std::fs::remove_dir_all(&root);
    result
}

/// `POST /_snapshot/{repository}/{snapshot}/_restore` (synchronous).
pub fn restore_snapshot(
    repo: &dyn SnapshotRepository,
    repo_name: &str,
    snap_name: &str,
    options: &RestoreOptions,
    state: &AppState,
) -> ServiceResult<RestoreOutcome> {
    let plan = plan_restore(repo, repo_name, snap_name, options, state)?;
    execute_restore(repo, &plan, state)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_settings_flatten_and_normalise() {
        let flat = flatten_index_settings(&json!({"index.number_of_replicas": 0})).unwrap();
        assert_eq!(flat.get("index.number_of_replicas"), Some(&json!(0)));
        let flat = flatten_index_settings(&json!({"index": {"number_of_replicas": 0}})).unwrap();
        assert_eq!(flat.get("index.number_of_replicas"), Some(&json!(0)));
        let flat = flatten_index_settings(&json!({"number_of_replicas": "1"})).unwrap();
        assert_eq!(flat.get("index.number_of_replicas"), Some(&json!("1")));
        assert!(flatten_index_settings(&json!([1])).is_err());
    }

    #[test]
    fn apply_index_settings_keeps_the_declared_layout() {
        let changes: BTreeMap<String, Value> =
            [("index.number_of_replicas".to_owned(), json!(0))].into();
        // Nested layout: written under `index`, flat duplicate removed.
        let mut nested = json!({"index": {"number_of_replicas": 2, "creation_date": "1"},
                                "number_of_replicas": 5, "analysis": {"a": 1}});
        apply_index_settings(&mut nested, &changes, &[]);
        assert_eq!(
            nested,
            json!({"index": {"number_of_replicas": 0, "creation_date": "1"}, "analysis": {"a": 1}})
        );
        // Flat layout (the deces mapping): written at the root.
        let mut flat = json!({"analysis": {"a": 1}, "creation_date": "1"});
        apply_index_settings(&mut flat, &changes, &[]);
        assert_eq!(
            flat,
            json!({"analysis": {"a": 1}, "creation_date": "1", "number_of_replicas": 0})
        );
        // Removal.
        let mut with_refresh = json!({"refresh_interval": "1s", "number_of_replicas": 1});
        apply_index_settings(
            &mut with_refresh,
            &BTreeMap::new(),
            &["index.refresh_interval".to_owned()],
        );
        assert_eq!(with_refresh, json!({"number_of_replicas": 1}));
    }
}
