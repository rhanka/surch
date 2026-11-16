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
//! Two payload formats (`docs/ops/snapshot-v2.md`): with `SURCH_DATA_DIR`
//! the sealed L6 index files are uploaded as blake3-addressed blobs
//! (`segments`, format v2, restored without re-indexing); without it, one
//! logical tarball of documents per index (v1, restored by re-ingestion).
//! `fs` and `s3` repositories; the registry is in memory (repositories are
//! re-registered after a restart, as the matchID Job does on every run).
//! A v2 take commits and pins one index generation, so concurrent writes
//! are neither lost nor mixed into the snapshot.

pub mod rename;
pub mod repository;
pub mod routes;
pub mod segments;
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
