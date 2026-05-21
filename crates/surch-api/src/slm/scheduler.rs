//! In-process SLM cron scheduler.
//!
//! Spawned at app boot. Sleeps `tick_interval` between iterations,
//! then for every policy in the registry:
//!
//! 1. (Re-)parse the cron expression to compute `next_fire`.
//! 2. If `next_fire <= now`, derive the snapshot name (via
//!    [`super::policy::expand_name_pattern`]) and call
//!    [`crate::snapshot_es::service::create_snapshot`] against the
//!    registered repository.
//! 3. Record the outcome in the policy execution log.
//! 4. Update the in-registry `next_fire` so `GET /_slm/policy/{id}`
//!    surfaces a fresh value to clients.
//!
//! The handle returned by [`spawn`] aborts the task when dropped —
//! tests use this to verify that `DELETE /_slm/policy/{id}` plus
//! shutdown leaves no orphan tokio task.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::task::JoinHandle;

use super::cron::CronSchedule;
use super::policy::{
    expand_name_pattern, SlmExecution, SlmExecutionState, SlmPolicy, SlmPolicyRegistry,
};
use crate::snapshot_es::service::{
    create_snapshot, delete_snapshot, list_snapshots, RegisteredRepository,
    SnapshotRepositoryRegistry,
};
use crate::state::AppState;

/// Tuning knobs for the scheduler tick loop. The defaults match the
/// "wake up every 30 seconds" target from the work-package; tests
/// override `tick_interval` so they can observe a fire inside ~1 s.
#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    pub tick_interval: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_secs(30),
        }
    }
}

/// Handle to the background scheduler task. Dropping it aborts the
/// task — required so test harnesses can rebuild a router cleanly.
pub struct SchedulerHandle {
    join: Option<JoinHandle<()>>,
}

impl SchedulerHandle {
    pub fn abort(&mut self) {
        if let Some(j) = self.join.take() {
            j.abort();
        }
    }
}

impl Drop for SchedulerHandle {
    fn drop(&mut self) {
        self.abort();
    }
}

/// Spawn the background scheduler. The returned handle owns the task;
/// dropping it cancels future ticks immediately.
pub fn spawn(
    config: SchedulerConfig,
    policies: SlmPolicyRegistry,
    repositories: SnapshotRepositoryRegistry,
    app: AppState,
) -> SchedulerHandle {
    let policies_arc = Arc::new(policies);
    let repositories_arc = Arc::new(repositories);
    let app_arc = Arc::new(app);

    let join = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(config.tick_interval);
        // Skip the immediate "missed tick" first-fire so freshly-created
        // policies don't double-fire — wait one tick before the first
        // pass so PUT-then-DELETE within the tick window is a no-op
        // for the scheduler.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            run_tick(
                policies_arc.as_ref(),
                repositories_arc.as_ref(),
                app_arc.as_ref(),
            )
            .await;
        }
    });

    SchedulerHandle { join: Some(join) }
}

/// One iteration of the scheduler loop. Public-in-crate so tests can
/// drive it directly without waiting on the tick interval.
pub(crate) async fn run_tick(
    policies: &SlmPolicyRegistry,
    repositories: &SnapshotRepositoryRegistry,
    app: &AppState,
) {
    let now = Utc::now();
    for (policy, current_next_fire) in policies.snapshot_for_scheduling() {
        let schedule = match CronSchedule::parse(&policy.schedule) {
            Ok(s) => s,
            Err(error) => {
                tracing::warn!(
                    policy = %policy.name,
                    schedule = %policy.schedule,
                    error = %error,
                    "slm: invalid cron expression, policy disabled until updated",
                );
                policies.set_next_fire(&policy.name, None);
                continue;
            }
        };

        // (Re)compute the next fire if we don't have one cached. We
        // anchor "after" at the latest of (now - tick_interval, last
        // execution) so a policy that just landed before this tick
        // fires immediately rather than waiting a full cycle.
        let next_fire = current_next_fire.or_else(|| {
            let anchor = policies
                .last_execution(&policy.name)
                .map(|e| chrono::DateTime::<Utc>::from_timestamp_millis(e.finished_at_millis))
                .unwrap_or(None)
                .unwrap_or_else(|| now - chrono::Duration::seconds(1));
            schedule.next_fire_after(anchor)
        });

        let Some(fire_at) = next_fire else {
            policies.set_next_fire(&policy.name, None);
            continue;
        };

        if fire_at > now {
            policies.set_next_fire(&policy.name, Some(fire_at));
            continue;
        }

        // Time to fire. Run the take then advance `next_fire`.
        execute_policy(&policy, now, repositories, app, policies).await;
        let new_next = schedule.next_fire_after(Utc::now());
        policies.set_next_fire(&policy.name, new_next);
    }
}

/// Execute a single SLM policy run. Public-in-crate so the
/// `POST /_slm/policy/{id}/_execute` handler can share the code path.
pub(crate) async fn execute_policy(
    policy: &SlmPolicy,
    fire_time: chrono::DateTime<Utc>,
    repositories: &SnapshotRepositoryRegistry,
    app: &AppState,
    policies: &SlmPolicyRegistry,
) -> SlmExecution {
    let snapshot_name = expand_name_pattern(&policy.name_pattern, fire_time);
    let started = Utc::now().timestamp_millis();

    let repo = repositories.get(&policy.repository);
    let exec = match repo {
        None => SlmExecution {
            snapshot_name: snapshot_name.clone(),
            state: SlmExecutionState::Failed,
            started_at_millis: started,
            finished_at_millis: Utc::now().timestamp_millis(),
            error: Some(format!("repository [{}] missing", policy.repository)),
        },
        Some(repo) => {
            // `create_snapshot` is sync (filesystem / S3-blocking-on-runtime
            // already absorbed inside the repository impls). The take is
            // already short relative to the tick interval at MVP scale, so
            // calling it inline is fine; if it grows we wrap in
            // `spawn_blocking`.
            let indices = policy.config.indices.clone();
            let include_global_state = policy.config.include_global_state;
            let repo_name = policy.repository.clone();
            let policy_name = policy.name.clone();
            let result = tokio::task::block_in_place(|| {
                create_snapshot(
                    repo.as_ref(),
                    &repo_name,
                    &snapshot_name,
                    &indices,
                    include_global_state,
                    app,
                )
            });
            let finished = Utc::now().timestamp_millis();
            match result {
                Ok(entry) => SlmExecution {
                    snapshot_name: entry.name,
                    state: SlmExecutionState::Success,
                    started_at_millis: started,
                    finished_at_millis: finished,
                    error: None,
                },
                Err(err) => SlmExecution {
                    snapshot_name,
                    state: SlmExecutionState::Failed,
                    started_at_millis: started,
                    finished_at_millis: finished,
                    error: Some(format!("policy `{policy_name}`: {err}")),
                },
            }
        }
    };

    policies.record_execution(&policy.name, exec.clone());
    if exec.state == SlmExecutionState::Success {
        enforce_max_count_retention(policy, repositories, policies);
    }
    exec
}

// Silence the dead-code lint when only a subset of the helpers is
// exercised by the public surface.
#[allow(dead_code)]
fn _force_registered_repository_alive(_r: RegisteredRepository) {}

fn enforce_max_count_retention(
    policy: &SlmPolicy,
    repositories: &SnapshotRepositoryRegistry,
    policies: &SlmPolicyRegistry,
) {
    let Some(retention) = &policy.retention else {
        return;
    };
    let min_count = retention.min_count.unwrap_or(0) as usize;
    let Some(repo) = repositories.get(&policy.repository) else {
        return;
    };
    let existing_snapshots = match list_snapshots(repo.as_ref()) {
        Ok(snapshots) => snapshots
            .into_iter()
            .map(|snapshot| snapshot.name)
            .collect::<BTreeSet<_>>(),
        Err(error) => {
            tracing::warn!(
                policy = %policy.name,
                repository = %policy.repository,
                error = %error,
                "slm: failed to list snapshots before retention",
            );
            return;
        }
    };
    let successful = policies
        .executions(&policy.name)
        .into_iter()
        .filter(|e| e.state == SlmExecutionState::Success)
        .filter(|e| existing_snapshots.contains(&e.snapshot_name))
        .collect::<Vec<_>>();
    let mut prune = BTreeSet::new();

    if let Some(max_count) = retention.max_count.map(|n| n as usize) {
        let keep_count = max_count.max(min_count);
        if successful.len() > keep_count {
            for execution in successful.iter().take(successful.len() - keep_count) {
                prune.insert(execution.snapshot_name.clone());
            }
        }
    }

    if let Some(expire_after_millis) = retention
        .expire_after
        .as_deref()
        .and_then(parse_retention_duration_millis)
    {
        let cutoff = Utc::now().timestamp_millis() - expire_after_millis;
        let eligible_count = successful.len().saturating_sub(min_count);
        for execution in successful.iter().take(eligible_count) {
            if execution.finished_at_millis <= cutoff {
                prune.insert(execution.snapshot_name.clone());
            }
        }
    }

    for snapshot_name in prune {
        if let Err(error) = delete_snapshot(repo.as_ref(), &policy.repository, &snapshot_name) {
            tracing::warn!(
                policy = %policy.name,
                repository = %policy.repository,
                snapshot = %snapshot_name,
                error = %error,
                "slm: failed to prune snapshot during retention",
            );
        }
    }
}

fn parse_retention_duration_millis(value: &str) -> Option<i64> {
    let value = value.trim();
    for (suffix, multiplier) in [
        ("ms", 1_i64),
        ("s", 1_000),
        ("m", 60_000),
        ("h", 3_600_000),
        ("d", 86_400_000),
    ] {
        if let Some(number) = value.strip_suffix(suffix) {
            let quantity = number.parse::<i64>().ok()?;
            if quantity < 0 {
                return None;
            }
            return quantity.checked_mul(multiplier);
        }
    }
    None
}
