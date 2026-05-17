//! SLM policy types and in-memory registry.
//!
//! Mirrors the ES 7.17 / OS 2.x `_slm/policy/*` wire shape verbatim:
//!
//! ```json
//! {
//!   "schedule": "0 30 1 * * ?",
//!   "name": "<deces-{now/d}>",
//!   "repository": "my-fs-repo",
//!   "config": { "indices": ["deces"], "include_global_state": false },
//!   "retention": { "expire_after": "30d", "min_count": 5, "max_count": 50 }
//! }
//! ```
//!
//! Retention is parsed and stored but not enforced yet — that pass is
//! the `C-SLM` phase 2 follow-up.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum number of execution records kept per policy. ES bounds this
/// at 100 (see `xpack.slm.history_index_max_size`); same bound here.
pub const EXECUTION_LOG_CAPACITY: usize = 100;

/// One SLM policy, as accepted by `PUT /_slm/policy/{id}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlmPolicy {
    /// Policy id (URL segment).
    pub name: String,
    /// Cron expression — 5 or 6 fields, see [`crate::slm::cron`].
    pub schedule: String,
    /// Snapshot name pattern. `{now/d}` is expanded to today's UTC
    /// date as `YYYYMMDD` at execution time. Angle-brackets (`<…>`)
    /// wrapping the pattern are tolerated and stripped — ES accepts
    /// both `<deces-{now/d}>` and `deces-{now/d}`.
    pub name_pattern: String,
    /// Repository that the snapshot is written to. Must already be
    /// registered via `PUT /_snapshot/{repository}` — the scheduler
    /// surfaces an execution error otherwise.
    pub repository: String,
    /// Take-config: indices selector + `include_global_state`.
    pub config: SlmConfig,
    /// Retention policy (accepted-and-stored — pruning is phase 2).
    #[serde(default)]
    pub retention: Option<SlmRetention>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SlmConfig {
    /// Index list (comma-separated or array of strings). Empty / `["*"]`
    /// means "every known index" at execution time.
    #[serde(default)]
    pub indices: Vec<String>,
    #[serde(default)]
    pub include_global_state: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlmRetention {
    /// ES duration string (`"30d"`, `"12h"`). Parsed lazily by phase 2.
    #[serde(default)]
    pub expire_after: Option<String>,
    #[serde(default)]
    pub min_count: Option<u32>,
    #[serde(default)]
    pub max_count: Option<u32>,
}

/// One execution record — feeds `GET /_slm/policy/{id}` and
/// `GET /_slm/policy/{id}/_executions`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlmExecution {
    pub snapshot_name: String,
    pub state: SlmExecutionState,
    pub started_at_millis: i64,
    pub finished_at_millis: i64,
    /// Non-empty on failure.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SlmExecutionState {
    Success,
    Failed,
}

impl SlmExecutionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
        }
    }
}

/// In-memory policy + execution registry.
///
/// Cloning the handle is cheap (`Arc` shares); every clone observes
/// the same underlying state.
#[derive(Clone, Default)]
pub struct SlmPolicyRegistry {
    inner: Arc<RwLock<SlmRegistryInner>>,
}

#[derive(Default)]
struct SlmRegistryInner {
    policies: BTreeMap<String, SlmPolicy>,
    executions: BTreeMap<String, VecDeque<SlmExecution>>,
    /// Next scheduled fire for each policy — refreshed by the scheduler
    /// every tick. `None` if the cron expression is unsatisfiable.
    next_fire: BTreeMap<String, Option<DateTime<Utc>>>,
}

impl SlmPolicyRegistry {
    pub fn list(&self) -> Vec<SlmPolicy> {
        let g = self.inner.read().expect("slm registry lock poisoned");
        g.policies.values().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<SlmPolicy> {
        let g = self.inner.read().expect("slm registry lock poisoned");
        g.policies.get(name).cloned()
    }

    pub fn upsert(&self, policy: SlmPolicy) {
        let mut g = self.inner.write().expect("slm registry lock poisoned");
        g.policies.insert(policy.name.clone(), policy);
    }

    pub fn remove(&self, name: &str) -> bool {
        let mut g = self.inner.write().expect("slm registry lock poisoned");
        let removed = g.policies.remove(name).is_some();
        g.executions.remove(name);
        g.next_fire.remove(name);
        removed
    }

    pub fn record_execution(&self, policy_name: &str, exec: SlmExecution) {
        let mut g = self.inner.write().expect("slm registry lock poisoned");
        let q = g.executions.entry(policy_name.to_owned()).or_default();
        q.push_back(exec);
        while q.len() > EXECUTION_LOG_CAPACITY {
            q.pop_front();
        }
    }

    pub fn executions(&self, policy_name: &str) -> Vec<SlmExecution> {
        let g = self.inner.read().expect("slm registry lock poisoned");
        g.executions
            .get(policy_name)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn last_execution(&self, policy_name: &str) -> Option<SlmExecution> {
        let g = self.inner.read().expect("slm registry lock poisoned");
        g.executions
            .get(policy_name)
            .and_then(|q| q.back().cloned())
    }

    pub fn execution_count(&self, policy_name: &str) -> usize {
        let g = self.inner.read().expect("slm registry lock poisoned");
        g.executions.get(policy_name).map(|q| q.len()).unwrap_or(0)
    }

    pub fn set_next_fire(&self, policy_name: &str, next: Option<DateTime<Utc>>) {
        let mut g = self.inner.write().expect("slm registry lock poisoned");
        g.next_fire.insert(policy_name.to_owned(), next);
    }

    pub fn next_fire(&self, policy_name: &str) -> Option<DateTime<Utc>> {
        let g = self.inner.read().expect("slm registry lock poisoned");
        g.next_fire.get(policy_name).copied().flatten()
    }

    /// Returns `(policy, next_fire)` for every registered policy —
    /// used by the scheduler tick loop.
    pub fn snapshot_for_scheduling(&self) -> Vec<(SlmPolicy, Option<DateTime<Utc>>)> {
        let g = self.inner.read().expect("slm registry lock poisoned");
        g.policies
            .values()
            .map(|p| (p.clone(), g.next_fire.get(&p.name).copied().flatten()))
            .collect()
    }
}

/// JSON representation matching the ES wire shape — exposed for the
/// `GET /_slm/policy/{id}` handler. Includes the last execution + the
/// next scheduled fire (ms epoch).
pub fn policy_to_es_json(
    policy: &SlmPolicy,
    last_execution: Option<&SlmExecution>,
    next_execution: Option<DateTime<Utc>>,
) -> Value {
    serde_json::json!({
        "version": 1,
        "modified_date_millis": 0,
        "policy": {
            "name": policy.name_pattern,
            "schedule": policy.schedule,
            "repository": policy.repository,
            "config": {
                "indices": policy.config.indices,
                "include_global_state": policy.config.include_global_state,
            },
            "retention": policy.retention.as_ref().map(|r| serde_json::json!({
                "expire_after": r.expire_after,
                "min_count": r.min_count,
                "max_count": r.max_count,
            })).unwrap_or(Value::Null),
        },
        "next_execution_millis": next_execution.map(|t| t.timestamp_millis()),
        "last_execution": last_execution.map(|e| serde_json::json!({
            "snapshot_name": e.snapshot_name,
            "state": e.state.as_str(),
            "start_time_millis": e.started_at_millis,
            "end_time_millis": e.finished_at_millis,
            "error": e.error,
        })),
    })
}

/// Expand the SLM name pattern. Strips one layer of `<…>` and replaces
/// `{now/d}` with today's UTC date as `YYYYMMDD`. Unknown placeholders
/// are left in place so the operator notices the gap.
pub fn expand_name_pattern(pattern: &str, now: DateTime<Utc>) -> String {
    let stripped = pattern
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(pattern);
    let date = now.format("%Y%m%d").to_string();
    stripped.replace("{now/d}", &date)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn expand_name_pattern_strips_angle_brackets_and_substitutes_now_d() {
        let now = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
        assert_eq!(
            expand_name_pattern("<deces-{now/d}>", now),
            "deces-20260516"
        );
        assert_eq!(expand_name_pattern("deces-{now/d}", now), "deces-20260516");
    }

    #[test]
    fn registry_records_executions_with_bounded_capacity() {
        let reg = SlmPolicyRegistry::default();
        let p = SlmPolicy {
            name: "p1".into(),
            schedule: "*/5 * * * * *".into(),
            name_pattern: "snap-{now/d}".into(),
            repository: "repo".into(),
            config: SlmConfig::default(),
            retention: None,
        };
        reg.upsert(p);
        for i in 0..(EXECUTION_LOG_CAPACITY + 10) {
            reg.record_execution(
                "p1",
                SlmExecution {
                    snapshot_name: format!("s-{i}"),
                    state: SlmExecutionState::Success,
                    started_at_millis: i as i64,
                    finished_at_millis: i as i64 + 1,
                    error: None,
                },
            );
        }
        assert_eq!(reg.execution_count("p1"), EXECUTION_LOG_CAPACITY);
        assert_eq!(
            reg.last_execution("p1").unwrap().snapshot_name,
            format!("s-{}", EXECUTION_LOG_CAPACITY + 9)
        );
    }

    #[test]
    fn remove_clears_associated_state() {
        let reg = SlmPolicyRegistry::default();
        reg.upsert(SlmPolicy {
            name: "p".into(),
            schedule: "*/5 * * * * *".into(),
            name_pattern: "x-{now/d}".into(),
            repository: "r".into(),
            config: SlmConfig::default(),
            retention: None,
        });
        reg.record_execution(
            "p",
            SlmExecution {
                snapshot_name: "x".into(),
                state: SlmExecutionState::Success,
                started_at_millis: 0,
                finished_at_millis: 0,
                error: None,
            },
        );
        reg.set_next_fire(
            "p",
            Some(Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()),
        );
        assert!(reg.remove("p"));
        assert!(reg.get("p").is_none());
        assert_eq!(reg.execution_count("p"), 0);
        assert!(reg.next_fire("p").is_none());
    }
}
