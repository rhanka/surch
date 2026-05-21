//! ES-parity Snapshot Lifecycle Management (`C-SLM`).
//!
//! Surch ships the Elasticsearch `_slm/policy/*` REST surface verbatim
//! so existing tooling (Curator, the Kibana SLM UI, `elasticsearch-py`)
//! keeps working when pointed at Surch — see `docs/ops/snapshot-plan.md`
//! § Phase S3 for the architectural anchor.
//!
//! The MVP is intentionally narrow:
//!
//! - In-memory policy registry (`Arc<RwLock<BTreeMap<_, _>>>`), no
//!   on-disk persistence yet (comes in a follow-up phase together with
//!   the `surch.toml` policy table).
//! - Cron expression parsing covers the 5-field (POSIX) and 6-field
//!   (Quartz, with leading "seconds") shapes — the only two flavours
//!   the ES SLM surface accepts. See [`cron::CronSchedule`].
//! - Name pattern expansion is `{now/d}` -> `YYYYMMDD`. Other ES
//!   placeholders (`{now/h}`, `{now/M}`) are not implemented yet —
//!   they fall through unchanged so the operator notices the gap.
//! - The scheduler is a single background `tokio::task` that wakes up
//!   every 30 seconds (configurable per-test via
//!   [`scheduler::SchedulerConfig::tick_interval`]) and executes every
//!   policy whose `next_fire` instant has elapsed. Retention enforces
//!   `max_count` and `expire_after`, while `min_count` protects the
//!   newest successful snapshots from pruning.
//!
//! Wire shape (mirrors ES 7.17 / OS 2.x):
//!
//! ```json
//! PUT /_slm/policy/daily-deces
//! {
//!   "schedule": "0 30 1 * * ?",
//!   "name": "<deces-{now/d}>",
//!   "repository": "my-fs-repo",
//!   "config": { "indices": ["deces"], "include_global_state": false },
//!   "retention": { "expire_after": "30d", "min_count": 5, "max_count": 50 }
//! }
//! ```

pub mod cron;
pub mod policy;
pub mod routes;
pub mod scheduler;

pub use policy::{
    expand_name_pattern, SlmConfig, SlmExecution, SlmExecutionState, SlmPolicy, SlmPolicyRegistry,
    SlmRetention,
};
pub use scheduler::{SchedulerConfig, SchedulerHandle};
