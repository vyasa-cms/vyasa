//! Postgres-backed job queue with `FOR UPDATE SKIP LOCKED`.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use vyasa_common::AppError;
use vyasa_db::content_models::JobRow;

/// Enqueues a job for execution at `run_at` (defaults to now if `None`).
///
/// # Errors
///
/// Returns `AppError::Db` on database failure.
pub async fn enqueue(
    pool: &PgPool,
    kind: &str,
    payload: serde_json::Value,
    run_at: Option<DateTime<Utc>>,
) -> Result<i64, AppError> {
    let id = vyasa_common::next_id_i64();
    sqlx::query(
        "INSERT INTO jobs (id, kind, payload, run_at, status) VALUES ($1, $2, $3, COALESCE($4, now()), 'queued')",
    )
    .bind(id)
    .bind(kind)
    .bind(payload)
    .bind(run_at)
    .execute(pool)
    .await
    .map_err(|err| AppError::db(format!("enqueue failed: {err}")))?;
    Ok(id)
}

/// Attempts after which a failing (or repeatedly abandoned) job is dead.
pub const MAX_ATTEMPTS: i32 = 5;

/// How long a `running` job may go unheard from before it counts as
/// abandoned by a worker that died with it. Far past any job here --
/// derivatives, a webhook, an email.
const LEASE: &str = "10 minutes";

/// A job this worker holds, and the proof that it still holds it.
///
/// `claimed_at` is the lease token: it changes every time the job is
/// claimed, so a worker whose lease expired and whose job was re-claimed
/// by another cannot complete or fail the other worker's run.
#[derive(Debug, Clone)]
pub struct Claimed {
    /// The job as it is now (status `running`, attempts counted).
    pub job: JobRow,
    /// When this claim was made; pass it back to [`complete`] / [`fail`].
    pub claimed_at: DateTime<Utc>,
}

/// Claims up to `batch` jobs that are due (`run_at <= now()`) and `queued`,
/// or `running` past their lease. Marks them `running` and bumps
/// `attempts` -- a reclaim is an attempt too, so a job that keeps killing
/// its worker is dead-lettered after [`MAX_ATTEMPTS`] instead of being
/// retried for ever. Uses `FOR UPDATE SKIP LOCKED` for safe concurrent
/// workers.
///
/// # Errors
///
/// Returns `AppError::Db` on database failure.
pub async fn claim(pool: &PgPool, batch: i64) -> Result<Vec<Claimed>, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| AppError::db(format!("tx begin failed: {err}")))?;
    // Abandoned with no attempts left: dead, not claimable.
    sqlx::query(&format!(
        "UPDATE jobs SET status = 'dead',
             last_error = COALESCE(last_error || '; ', '') || 'abandoned by its worker'
         WHERE status = 'running' AND claimed_at < now() - interval '{LEASE}'
           AND attempts >= $1"
    ))
    .bind(MAX_ATTEMPTS)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::db(format!("dead-letter abandoned failed: {err}")))?;
    // clock_timestamp(), not now(): the token must differ between two
    // claims of the same job even within one transaction's timestamp.
    let rows: Vec<(i64, DateTime<Utc>)> = sqlx::query_as(&format!(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1,
                         claimed_at = clock_timestamp()
         WHERE id IN (
             SELECT id FROM jobs
             WHERE (status = 'queued' AND run_at <= now())
                OR (status = 'running' AND claimed_at < now() - interval '{LEASE}')
             ORDER BY run_at ASC
             LIMIT $1
             FOR UPDATE SKIP LOCKED)
         RETURNING id, claimed_at"
    ))
    .bind(batch)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::db(format!("claim failed: {err}")))?;
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let jobs: Vec<JobRow> = sqlx::query_as(
        "SELECT id, kind, payload, run_at, status, attempts, last_error, created_at
         FROM jobs WHERE id = ANY($1) ORDER BY run_at ASC",
    )
    .bind(&ids)
    .fetch_all(&mut *tx)
    .await
    .map_err(|err| AppError::db(format!("claim read failed: {err}")))?;
    tx.commit()
        .await
        .map_err(|err| AppError::db(format!("tx commit failed: {err}")))?;
    Ok(jobs
        .into_iter()
        .filter_map(|job| {
            let claimed_at = rows.iter().find(|(id, _)| *id == job.id)?.1;
            Some(Claimed { job, claimed_at })
        })
        .collect())
}

/// Marks a job as `done`, if this claim still holds it.
///
/// Returns `false` when the claim was lost (the lease expired and the job
/// was re-claimed, or it was dead-lettered): the other claimant's outcome
/// stands.
///
/// # Errors
///
/// Returns `AppError::Db` on database failure.
pub async fn complete(pool: &PgPool, claim: &Claimed) -> Result<bool, AppError> {
    let done = sqlx::query(
        "UPDATE jobs SET status = 'done'
         WHERE id = $1 AND status = 'running' AND claimed_at = $2",
    )
    .bind(claim.job.id)
    .bind(claim.claimed_at)
    .execute(pool)
    .await
    .map_err(|err| AppError::db(format!("complete failed: {err}")))?;
    Ok(done.rows_affected() == 1)
}

/// Records a failed run, if this claim still holds the job: back to
/// `queued` with exponential backoff + jitter, or `dead` once attempts
/// reach [`MAX_ATTEMPTS`]. `error` is stored in `last_error`.
///
/// Returns `false` when the claim was lost, as [`complete`] does.
///
/// # Errors
///
/// Returns `AppError::Db` on database failure.
pub async fn fail(pool: &PgPool, claim: &Claimed, error: &str) -> Result<bool, AppError> {
    // Exponential backoff from one minute: 1m, 2m, 4m, 8m, 16m, then
    // dead -- about half an hour in all. From one second it was 2s,
    // 4s, 8s, 16s, 32s: a receiver down for a minute lost the event.
    // Computed from the attempts on the row, in the same statement that
    // checks the claim.
    let jitter: f64 = rand::random::<f64>();
    let done = sqlx::query(
        "UPDATE jobs SET
             status = CASE WHEN attempts >= $3 THEN 'dead' ELSE 'queued' END,
             run_at = CASE WHEN attempts >= $3 THEN run_at
                      ELSE now() + make_interval(
                          secs => 60 * power(2, GREATEST(attempts, 1) - 1) + $5)
                      END,
             last_error = $4
         WHERE id = $1 AND status = 'running' AND claimed_at = $2",
    )
    .bind(claim.job.id)
    .bind(claim.claimed_at)
    .bind(MAX_ATTEMPTS)
    .bind(error)
    .bind(jitter)
    .execute(pool)
    .await
    .map_err(|err| AppError::db(format!("fail update failed: {err}")))?;
    Ok(done.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::{claim, complete, enqueue, fail, MAX_ATTEMPTS};
    use serde_json::json;
    use vyasa_testkit::TestDb;

    #[tokio::test]
    async fn enqueue_claim_complete() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let id = enqueue(&pool, "test_kind", json!({"a":1}), None)
            .await
            .expect("enqueue");
        let claimed = claim(&pool, 10).await.expect("claim");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].job.id, id);
        assert_eq!(claimed[0].job.kind, "test_kind");
        // Second claim should be empty (already running)
        let empty = claim(&pool, 10).await.expect("claim2");
        assert!(empty.is_empty());
        assert!(complete(&pool, &claimed[0]).await.expect("complete"));
        let status: String = sqlx::query_scalar("SELECT status::text FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("fetch2");
        assert_eq!(status, "done");
    }

    #[tokio::test]
    async fn a_job_abandoned_by_a_dead_worker_is_claimed_again() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        enqueue(&pool, "media_derivatives", json!({"media_id": 1}), None)
            .await
            .expect("enqueue");
        let first = claim(&pool, 10).await.expect("claim");
        assert_eq!(first.len(), 1);
        // Still running, and nobody has heard from the worker for an hour.
        assert!(
            claim(&pool, 10).await.expect("claim").is_empty(),
            "held by its worker"
        );
        sqlx::query("UPDATE jobs SET claimed_at = now() - interval '1 hour'")
            .execute(&pool)
            .await
            .unwrap();
        let again = claim(&pool, 10).await.expect("claim");
        assert_eq!(again.len(), 1, "an abandoned job stays lost");
        assert_eq!(again[0].job.attempts, 2, "and the retry is counted");
        // The first worker wakes up late: its claim is gone, so neither
        // its success nor its failure overrides the new run.
        assert!(!complete(&pool, &first[0]).await.expect("complete"));
        assert!(!fail(&pool, &first[0], "late").await.expect("fail"));
        assert!(complete(&pool, &again[0]).await.expect("complete"));
    }

    #[tokio::test]
    async fn a_job_that_keeps_killing_its_worker_is_dead_lettered() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let id = enqueue(&pool, "crashy", json!({}), None)
            .await
            .expect("enqueue");
        for n in 1..=MAX_ATTEMPTS {
            let got = claim(&pool, 10).await.expect("claim");
            assert_eq!(got.len(), 1, "claim {n}");
            assert_eq!(got[0].job.attempts, n);
            // The worker dies: nobody completes or fails it.
            sqlx::query("UPDATE jobs SET claimed_at = now() - interval '1 hour'")
                .execute(&pool)
                .await
                .unwrap();
        }
        assert!(claim(&pool, 10).await.expect("claim").is_empty());
        let status: String = sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "dead");
    }

    #[tokio::test]
    async fn retry_and_dead_letter() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let id = enqueue(&pool, "failing", json!({}), None)
            .await
            .expect("enqueue");
        for i in 0..5 {
            let claimed = claim(&pool, 10).await.expect("claim");
            assert_eq!(claimed.len(), 1, "round {i}");
            assert!(fail(&pool, &claimed[0], "boom").await.expect("fail"));
            // Make it immediately claimable for the next iteration (except after the last fail where it should be dead)
            if i < 4 {
                sqlx::query("UPDATE jobs SET run_at = now() WHERE id = $1")
                    .bind(id)
                    .execute(&pool)
                    .await
                    .expect("reset run_at");
            }
        }
        // After 5 fails, should be dead (needs one more claim to transition? Actually fail after 5th claim makes dead directly)
        // The 5th fail should have set dead, so next claim is empty and status is dead.
        let status: String = sqlx::query_scalar("SELECT status::text FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("fetch");
        assert_eq!(status, "dead");
        // Also test that claim on dead doesn't return it
        let claimed = claim(&pool, 10).await.expect("claim dead");
        assert!(claimed.is_empty());
    }
}
