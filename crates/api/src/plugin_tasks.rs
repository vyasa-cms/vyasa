//! Scheduled plugin work.
//!
//! A plugin declares `[{ name, every-seconds }]` from `register-schedule`
//! and implements `run-task`; this runs them. State lives in the
//! `plugin_tasks` table rather than in memory so that a task scheduled
//! every six hours is not re-run by every deploy, and so an operator can
//! see when each one last ran and whether it worked.

use vyasa_common::AppError;
use vyasa_plugins::host::{HostEnv, V2Outcome};

use crate::state::AppState;

/// How often the runner looks for due work.
///
/// The shortest interval a task may declare is a minute, so checking twice
/// a minute keeps drift under the resolution anyone asked for.
const TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// Records the declared tasks, keeping the run history of ones that
/// already existed and dropping ones a plugin no longer declares.
///
/// # Errors
/// Propagates storage failures; the caller logs and carries on, because a
/// site that will not boot because a plugin's cron table is unavailable is
/// worse than a site whose cron is late.
pub async fn reconcile(state: &AppState) -> Result<(), AppError> {
    let declared = state.plugin_surface.tasks().await;
    for task in &declared {
        sqlx::query(
            "INSERT INTO plugin_tasks (plugin_id, name, every_seconds)
             VALUES ($1, $2, $3)
             ON CONFLICT (plugin_id, name) DO UPDATE SET every_seconds = EXCLUDED.every_seconds",
        )
        .bind(task.plugin_id)
        .bind(&task.name)
        .bind(task.every_seconds)
        .execute(&state.pool)
        .await
        .map_err(|e| AppError::internal_msg(e.to_string()))?;
    }
    // Anything not declared this boot is gone: the plugin was disabled,
    // upgraded, or simply dropped the task.
    //
    // Matched as a pair of columns rather than as `plugin_id || ':' || name`.
    // A task named `1:sync` on plugin 2 joins to the same string as a task
    // named `sync` on plugin 21, so the join could keep a row that was no
    // longer declared — or delete one that was.
    let plugin_ids: Vec<i64> = declared.iter().map(|t| t.plugin_id).collect();
    let names: Vec<String> = declared.iter().map(|t| t.name.clone()).collect();
    sqlx::query(
        "DELETE FROM plugin_tasks
         WHERE (plugin_id, name) <> ALL (
             SELECT unnest($1::bigint[]), unnest($2::text[])
         )",
    )
    .bind(&plugin_ids)
    .bind(&names)
    .execute(&state.pool)
    .await
    .map_err(|e| AppError::internal_msg(e.to_string()))?;
    if !declared.is_empty() {
        tracing::info!(
            "plugin tasks: {}",
            declared
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

/// Starts the background runner. Returns immediately.
pub fn spawn_runner(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            for (plugin_id, name) in claim_due(&state).await {
                run_one(&state, plugin_id, &name).await;
            }
        }
    });
}

/// Claims every task that is due, marking it run before running it.
///
/// Claiming first is what keeps a task that panics or times out from being
/// retried on the very next tick forever, and the `UPDATE ... RETURNING`
/// makes the claim atomic, so two servers against one database do not both
/// run the same task.
async fn claim_due(state: &AppState) -> Vec<(i64, String)> {
    let claimed: Result<Vec<(i64, String)>, _> = sqlx::query_as(
        "UPDATE plugin_tasks SET last_run_at = now()
         WHERE (plugin_id, name) IN (
             SELECT plugin_id, name FROM plugin_tasks
             WHERE last_run_at IS NULL
                OR last_run_at + make_interval(secs => every_seconds) <= now()
             FOR UPDATE SKIP LOCKED
         )
         RETURNING plugin_id, name",
    )
    .fetch_all(&state.pool)
    .await;
    match claimed {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("could not claim plugin tasks: {e}");
            Vec::new()
        }
    }
}

async fn run_one(state: &AppState, plugin_id: i64, name: &str) {
    // A scheduled task serves nobody, so there is no viewer to report.
    let env = HostEnv::background(std::sync::Arc::clone(&state.broker), state.pool.clone());
    let started = std::time::Instant::now();
    let (status, error) = match state.plugin_host.call_run_task(plugin_id, name, &env).await {
        V2Outcome::Ok(()) => ("ok", None),
        V2Outcome::Unsupported => (
            "failed",
            Some(String::from("plugin does not implement run-task")),
        ),
        V2Outcome::Failed(reason) => ("failed", Some(reason)),
    };
    if let Some(reason) = &error {
        tracing::warn!(plugin_id, task = name, "task failed: {reason}");
        state.metrics.inc_plugin_failure();
    } else {
        tracing::debug!(
            plugin_id,
            task = name,
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "task ran"
        );
    }
    let recorded = sqlx::query(
        "UPDATE plugin_tasks SET last_status = $3, last_error = $4
         WHERE plugin_id = $1 AND name = $2",
    )
    .bind(plugin_id)
    .bind(name)
    .bind(status)
    .bind(
        error
            .as_deref()
            .map(|e| e.chars().take(500).collect::<String>()),
    )
    .execute(&state.pool)
    .await;
    if let Err(e) = recorded {
        tracing::warn!(plugin_id, task = name, "could not record task result: {e}");
    }
}
