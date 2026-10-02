#![allow(clippy::pedantic, clippy::missing_errors_doc)]
#![allow(missing_docs)]
//! Job runner with N workers and dispatch table.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{error, info, warn};

use crate::queue::{claim, complete, fail};

/// A handler the embedding application supplies for job kinds this crate
/// does not know — the AI jobs live in the API crate, which owns the model
/// registry, and this keeps the dependency direction intact.
///
/// `handle` returns `None` for kinds it does not own, so the built-in
/// dispatch can take them.
pub trait JobHandler: Send + Sync {
    fn handle<'a>(&'a self, kind: &'a str, payload: serde_json::Value) -> HandlerFuture<'a>;
}

/// What a [`JobHandler`] returns: `None` when the kind is not its own.
pub type HandlerFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<Result<(), String>>> + Send + 'a>>;

/// How many jobs to claim per poll.
const BATCH: i64 = 10;

/// Spawns `workers` background tasks that poll the queue.
/// Returns handles and a notifier to wake them.
pub fn spawn(
    pool: PgPool,
    workers: u32,
    notify: Arc<Notify>,
    smtp: Option<vyasa_common::SmtpConfig>,
    extra: Option<Arc<dyn JobHandler>>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let workers = workers.max(1) as usize;
    (0..workers)
        .map(|i| {
            let pool = pool.clone();
            let notify = notify.clone();
            let smtp = smtp.clone();
            let extra = extra.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(5));
                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            if let Err(e) = tick(&pool, i, smtp.as_ref(), extra.as_deref()).await {
                                error!(worker = i, "tick failed: {e}");
                            }
                        }
                        () = notify.notified() => {
                            if let Err(e) = tick(&pool, i, smtp.as_ref(), extra.as_deref()).await {
                                error!(worker = i, "nudge failed: {e}");
                            }
                        }
                    }
                }
            })
        })
        .collect()
}

async fn tick(
    pool: &PgPool,
    worker_id: usize,
    smtp: Option<&vyasa_common::SmtpConfig>,
    extra: Option<&dyn JobHandler>,
) -> Result<(), String> {
    let jobs = claim(pool, BATCH)
        .await
        .map_err(|e| format!("claim failed: {e}"))?;
    for claimed in jobs {
        let job = &claimed.job;
        info!(worker = worker_id, job_id = job.id, kind = %job.kind, "dispatching job");
        let result = dispatch(pool, job, smtp, extra).await;
        let recorded = match result {
            Ok(()) => complete(pool, &claimed).await,
            Err(err) => {
                error!(job_id = job.id, kind = %job.kind, "job failed: {err}");
                fail(pool, &claimed, &err).await
            }
        };
        match recorded {
            Ok(true) => {}
            // The lease ran out and another worker holds the job now;
            // its outcome is the one that counts.
            Ok(false) => warn!(
                job_id = job.id,
                "claim lost before the outcome was recorded"
            ),
            Err(e) => error!(job_id = job.id, "recording outcome failed: {e}"),
        }
    }
    Ok(())
}

async fn dispatch(
    pool: &PgPool,
    job: &vyasa_db::content_models::JobRow,
    smtp: Option<&vyasa_common::SmtpConfig>,
    extra: Option<&dyn JobHandler>,
) -> Result<(), String> {
    if let Some(handler) = extra {
        if let Some(outcome) = handler.handle(&job.kind, job.payload.clone()).await {
            return outcome;
        }
    }
    match job.kind.as_str() {
        // Reached only when no handler from the API process claimed the
        // kind — the API registers one that carries the configured backend.
        // A bare runner has nothing but the environment to go on.
        "media_derivatives" => {
            let base = std::env::var("VYASA_MEDIA_DIR").unwrap_or_else(|_| "media".to_string());
            let storage = vyasa_core::media::LocalFsBackend::new(base);
            crate::workers::media::handle(pool, &storage, job.payload.clone()).await
        }
        "webhook_deliver" => crate::workers::webhooks::handle(pool, job.payload.clone()).await,
        "send_email" => crate::workers::email::handle(smtp, job.payload.clone()).await,
        "indexnow_ping" => crate::workers::indexnow::handle(job.payload.clone()).await,
        "publish_due" => {
            // Direct publish without queue recursion.
            let n = crate::publisher::publish_due_once(pool)
                .await
                .map_err(|e| format!("{e}"))?;
            info!(published = n, "publish_due via queue");
            Ok(())
        }
        other => Err(format!("unknown job kind: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch;
    use serde_json::json;
    use vyasa_testkit::TestDb;

    #[tokio::test]
    async fn unknown_kind_fails() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let job = vyasa_db::content_models::JobRow {
            id: 1,
            kind: "nope".into(),
            payload: json!({}),
            run_at: chrono::Utc::now(),
            status: vyasa_db::content_models::JobStatus::Queued,
            attempts: 0,
            last_error: None,
            created_at: chrono::Utc::now(),
        };
        let res = dispatch(&pool, &job, None, None).await;
        assert!(res.is_err());
    }
}
