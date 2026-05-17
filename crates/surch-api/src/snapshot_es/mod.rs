//! ES-parity `_snapshot` API (`C-SNAPSHOT-S1`).
//!
//! Surch ships the Elasticsearch / OpenSearch snapshot surface
//! verbatim — same REST routes, same wire shapes, same on-repository
//! layout — because Surch is offered as a PaaS where existing ES
//! tooling (Curator, the `_snapshot` UI in Kibana, `elasticsearch-py`)
//! must keep working without code changes. See `docs/ops/snapshot-plan.md`
//! §4 for the binding decision.
//!
//! The Surch-specific `/_surch/snapshot/{export,import}` routes
//! (`C-SNAPSHOT-RAW`) stay alive in parallel for the matchID boot
//! flow that already relies on them; they are formally deprecated and
//! will be removed in `0.4.0`. The handlers here are the canonical
//! shape going forward.
//!
//! The MVP is intentionally narrow: filesystem repository only,
//! one tarball per snapshotted index, in-memory repository registry
//! (no `surch.toml` persistence yet — comes in the follow-up phase).
//! Concurrent writes during a snapshot take are not actively fenced
//! either; matchID is read-mostly and the operator is expected to
//! pause ingestion before triggering a take. The follow-up phase
//! adds the `RwLock<SnapshotState>` write fence and persistent
//! registration.

pub mod repository;
pub mod routes;
pub mod service;

pub use repository::{
    FsRepository, RepositoryError, RepositoryResult, S3Repository, S3RepositoryConfig,
    SnapshotRepository,
};
pub use service::{
    create_snapshot, delete_snapshot, get_snapshot, restore_snapshot, RegisteredRepository,
    RootManifest, SnapshotEntry, SnapshotRepositoryRegistry, SnapshotServiceError, SnapshotState,
    ES_SNAPSHOT_FORMAT_VERSION,
};
