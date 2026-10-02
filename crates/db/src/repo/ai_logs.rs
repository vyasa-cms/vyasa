//! AI usage/cost log repository over `ai_logs` (migration 0013).

use sqlx::PgPool;
use vyasa_common::AppError;

/// One recorded completion.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AiLogRow {
    /// Snowflake id.
    pub id: i64,
    /// Provider name.
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Purpose tag.
    pub purpose: String,
    /// Input tokens.
    pub prompt_tokens: i32,
    /// Output tokens.
    pub completion_tokens: i32,
    /// Computed USD cost.
    pub cost_usd: f64,
}

/// Data access for ai_logs.
#[derive(Clone, Debug)]
pub struct AiLogRepo {
    pool: PgPool,
}

impl AiLogRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Records one completion.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn record(
        &self,
        provider: &str,
        model: &str,
        purpose: &str,
        prompt_tokens: i32,
        completion_tokens: i32,
        cost_usd: f64,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO ai_logs (id, provider, model, purpose, prompt_tokens, \\
             completion_tokens, cost_usd) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(provider)
        .bind(model)
        .bind(purpose)
        .bind(prompt_tokens)
        .bind(completion_tokens)
        .bind(cost_usd)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("ai_log insert failed: {e}")))?;
        Ok(())
    }

    /// Recent rows, newest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn recent(&self, limit: i64) -> Result<Vec<AiLogRow>, AppError> {
        sqlx::query_as::<_, AiLogRow>(
            "SELECT id, provider, model, purpose, prompt_tokens, completion_tokens, \\
             cost_usd FROM ai_logs ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("ai_log list failed: {e}")))
    }
}

impl AiLogRepo {
    /// Month-to-date spend grouped by purpose, UTC calendar month.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn month_spend_by_purpose(&self) -> Result<Vec<(String, f64)>, AppError> {
        sqlx::query_as::<_, (String, f64)>(
            "SELECT purpose, SUM(cost_usd)::float8 FROM ai_logs
             WHERE date_trunc('month', created_at) = date_trunc('month', now())
             GROUP BY purpose ORDER BY purpose",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("ai month spend failed: {e}")))
    }

    /// How many rows with `purpose` were recorded since `since`.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn count_purpose_since(
        &self,
        purpose: &str,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM ai_logs WHERE purpose = $1 AND created_at >= $2",
        )
        .bind(purpose)
        .bind(since)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("ai_logs count failed: {e}")))
    }

    /// Total month-to-date spend across all purposes.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn month_spend_total(&self) -> Result<f64, AppError> {
        sqlx::query_scalar::<_, f64>(
            "SELECT COALESCE(SUM(cost_usd), 0)::float8 FROM ai_logs
             WHERE date_trunc('month', created_at) = date_trunc('month', now())",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("ai month total failed: {e}")))
    }
}
