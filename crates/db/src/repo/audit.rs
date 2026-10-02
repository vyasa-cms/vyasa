//! Audit log for destructive administrative actions.
//!
//! Writes are best-effort and never block the action being audited: a
//! logging failure that refused a legitimate deletion would be a worse
//! outcome than a missing row, and the caller has already been authorised.

use sqlx::PgPool;
use vyasa_common::AppError;

/// One recorded action.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct AuditRow {
    /// Snowflake id.
    pub id: i64,
    /// Acting user, or `None` if that account has since been deleted.
    pub actor_id: Option<i64>,
    /// Actor's display name, captured at the time so the row stays readable.
    pub actor_name: String,
    /// What happened, e.g. `user.delete`.
    pub action: String,
    /// What it happened to, e.g. `user:42`.
    pub target: String,
    /// Anything else worth keeping.
    pub detail: serde_json::Value,
    /// Client address, as far as it can be known.
    pub ip: String,
    /// When.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Audit-log repository.
#[derive(Clone, Debug)]
pub struct AuditRepo {
    pool: PgPool,
}

impl AuditRepo {
    /// Wraps a pool.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Records one action.
    ///
    /// # Errors
    /// Propagates database failures. Callers should log and continue rather
    /// than fail the action.
    pub async fn record(
        &self,
        actor_id: Option<i64>,
        actor_name: &str,
        action: &str,
        target: &str,
        detail: serde_json::Value,
        ip: &str,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO audit_log (id, actor_id, actor_name, action, target, detail, ip) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(actor_id)
        .bind(actor_name)
        .bind(action)
        .bind(target)
        .bind(detail)
        .bind(ip)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("audit write failed: {e}")))?;
        Ok(())
    }

    /// Most recent entries, newest first.
    ///
    /// # Errors
    /// Propagates database failures.
    pub async fn recent(&self, limit: i64) -> Result<Vec<AuditRow>, AppError> {
        sqlx::query_as(
            "SELECT id, actor_id, actor_name, action, target, detail, ip, created_at \
             FROM audit_log ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("audit list failed: {e}")))
    }
}
