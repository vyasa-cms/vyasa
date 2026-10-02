//! Session repository.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use vyasa_common::AppError;

use crate::models::SessionRow;

/// Data access for the `sessions` table.
#[derive(Clone, Debug)]
pub struct SessionsRepo {
    pool: PgPool,
}

impl SessionsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a session with `token` as its primary key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn insert(
        &self,
        token: &str,
        user_id: i64,
        ttl: Duration,
        user_agent: Option<&str>,
        ip: Option<&str>,
    ) -> Result<SessionRow, AppError> {
        sqlx::query_as(
            "INSERT INTO sessions (id, user_id, expires_at, user_agent, ip)
             VALUES ($1, $2, now() + make_interval(secs => $3), $4, $5::inet)
             RETURNING id, user_id, expires_at, created_at, user_agent, host(ip) AS ip",
        )
        .bind(token)
        .bind(user_id)
        .bind(ttl.num_seconds())
        .bind(user_agent)
        .bind(ip)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("session insert failed: {err}")))
    }

    /// Fetches a live (unexpired) session by token.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when the token is unknown or the
    /// session has expired, or [`AppError::Db`] on database failure.
    pub async fn get_live(&self, token: &str) -> Result<SessionRow, AppError> {
        sqlx::query_as(
            "SELECT id, user_id, expires_at, created_at, user_agent, host(ip) AS ip
             FROM sessions
             WHERE id = $1 AND expires_at > now()",
        )
        .bind(token)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("session lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("session", "token"))
    }

    /// Deletes a session by token.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete(&self, token: &str) -> Result<(), AppError> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("session delete failed: {err}")))?;
        Ok(())
    }

    /// Ends every session a user holds.
    ///
    /// A password reset that leaves existing sessions alive is not a reset:
    /// whoever had the old credential — the reason for resetting — keeps
    /// their access until each session expires on its own.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete_for_user(&self, user_id: i64) -> Result<u64, AppError> {
        let result = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("session revoke failed: {err}")))?;
        Ok(result.rows_affected())
    }

    /// Deletes all expired sessions (housekeeping).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn purge_expired(&self) -> Result<u64, AppError> {
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("session purge failed: {err}")))?;
        Ok(result.rows_affected())
    }

    /// Counts live sessions for a user (device management later).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_for_user(&self, user_id: i64) -> Result<i64, AppError> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE user_id = $1 AND expires_at > now()",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("session count failed: {err}")))
    }
}

/// Convenience: current time minus nothing; exists to keep call sites
/// readable when computing expiries.
#[must_use]
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}
