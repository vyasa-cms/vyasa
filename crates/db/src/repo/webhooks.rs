//! Webhooks repository: registration + delivery log.

use sqlx::PgPool;
use vyasa_common::AppError;

/// One registered webhook endpoint.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct WebhookRow {
    /// Snowflake id.
    pub id: i64,
    /// Target URL.
    pub url: String,
    /// HMAC secret (shown once at creation).
    #[serde(skip_serializing)]
    pub secret: String,
    /// Subscribed event names (JSON array).
    pub events: serde_json::Value,
    /// Whether delivery is active.
    pub enabled: bool,
}

/// One recorded delivery attempt.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct DeliveryRow {
    /// Row id.
    pub id: i64,
    /// Owning webhook.
    pub webhook_id: i64,
    /// Event name.
    pub event: String,
    /// success | failed | dead
    pub status: String,
    /// HTTP status from the receiver (0 = no response).
    pub response_code: Option<i32>,
    /// Attempt count for this (webhook, event) pair so far.
    pub attempts: i32,
    /// When this attempt happened.
    pub last_attempt: chrono::DateTime<chrono::Utc>,
    /// The exact signed body that was sent (for redelivery and debugging).
    pub payload: Option<String>,
    /// The first part of what the receiver answered.
    pub response_body: Option<String>,
    /// The transport error, when there was no response at all.
    pub error: Option<String>,
}

/// The newest delivery per webhook, for the list.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct LastDelivery {
    /// Owning webhook.
    pub webhook_id: i64,
    /// success | failed | dead
    pub status: String,
    /// When.
    pub last_attempt: chrono::DateTime<chrono::Utc>,
}

/// Data access for webhooks + deliveries.
#[derive(Clone, Debug)]
pub struct WebhooksRepo {
    pool: PgPool,
}

impl WebhooksRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Registers a webhook.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn create(
        &self,
        url: &str,
        secret: &str,
        events: &serde_json::Value,
    ) -> Result<WebhookRow, AppError> {
        sqlx::query_as::<_, WebhookRow>(
            "INSERT INTO webhooks (id, url, secret, events) VALUES ($1, $2, $3, $4)
             RETURNING id, url, secret, events, enabled",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(url)
        .bind(secret)
        .bind(events)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("webhook create failed: {e}")))
    }

    /// All enabled webhooks subscribed to `event`.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list_for_event(&self, event: &str) -> Result<Vec<WebhookRow>, AppError> {
        sqlx::query_as::<_, WebhookRow>(
            "SELECT id, url, secret, events, enabled FROM webhooks
             WHERE enabled AND events ? $1",
        )
        .bind(event)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("webhook fanout query failed: {e}")))
    }

    /// All webhooks (admin listing).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list(&self) -> Result<Vec<WebhookRow>, AppError> {
        sqlx::query_as::<_, WebhookRow>(
            "SELECT id, url, secret, events, enabled FROM webhooks ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("webhook list failed: {e}")))
    }

    /// Fetches one webhook by id.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<WebhookRow, AppError> {
        sqlx::query_as::<_, WebhookRow>(
            "SELECT id, url, secret, events, enabled FROM webhooks WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("webhook get failed: {e}")))?
        .ok_or_else(|| AppError::not_found("webhook", id))
    }

    /// Changes the target, subscriptions or enabled flag; `None` keeps.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn update(
        &self,
        id: i64,
        url: Option<&str>,
        events: Option<&serde_json::Value>,
        enabled: Option<bool>,
    ) -> Result<WebhookRow, AppError> {
        sqlx::query_as::<_, WebhookRow>(
            "UPDATE webhooks SET url = COALESCE($2, url), events = COALESCE($3, events),
                    enabled = COALESCE($4, enabled)
             WHERE id = $1 RETURNING id, url, secret, events, enabled",
        )
        .bind(id)
        .bind(url)
        .bind(events)
        .bind(enabled)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("webhook update failed: {e}")))?
        .ok_or_else(|| AppError::not_found("webhook", id))
    }

    /// Replaces the signing secret.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn rotate_secret(&self, id: i64, secret: &str) -> Result<(), AppError> {
        let n = sqlx::query("UPDATE webhooks SET secret = $2 WHERE id = $1")
            .bind(id)
            .bind(secret)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("webhook secret rotate failed: {e}")))?
            .rows_affected();
        if n == 0 {
            return Err(AppError::not_found("webhook", id));
        }
        Ok(())
    }

    /// Deletes a webhook (deliveries cascade).
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let updated = sqlx::query("DELETE FROM webhooks WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("webhook delete failed: {e}")))?
            .rows_affected();
        if updated == 0 {
            return Err(AppError::not_found("webhook", id));
        }
        Ok(())
    }

    /// Records one delivery attempt; increments attempts and sets status.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn record_delivery(
        &self,
        webhook_id: i64,
        event: &str,
        status: &str,
        response_code: Option<i32>,
    ) -> Result<(), AppError> {
        self.record_delivery_detail(webhook_id, event, status, response_code, None, None, None)
            .await
    }

    /// Records one delivery attempt with what was sent and what came back.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_delivery_detail(
        &self,
        webhook_id: i64,
        event: &str,
        status: &str,
        response_code: Option<i32>,
        payload: Option<&str>,
        response_body: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), AppError> {
        let prior = self.attempts_so_far(webhook_id, event).await.unwrap_or(0);
        sqlx::query(
            "INSERT INTO webhook_deliveries
                (id, webhook_id, event, status, response_code, attempts, payload, response_body, error)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(webhook_id)
        .bind(event)
        .bind(status)
        .bind(response_code)
        .bind(prior + 1)
        .bind(payload)
        .bind(response_body.map(|b| b.chars().take(2000).collect::<String>()))
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("delivery insert failed: {e}")))?;
        Ok(())
    }

    async fn attempts_so_far(&self, webhook_id: i64, event: &str) -> Result<i32, AppError> {
        sqlx::query_scalar::<_, i32>(
            "SELECT COALESCE(MAX(attempts), 0) FROM webhook_deliveries
             WHERE webhook_id = $1 AND event = $2 AND last_attempt > now() - interval '24 hours'",
        )
        .bind(webhook_id)
        .bind(event)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("attempts lookup failed: {e}")))
    }

    /// Recent deliveries for a webhook.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn deliveries(
        &self,
        webhook_id: i64,
        limit: i64,
    ) -> Result<Vec<DeliveryRow>, AppError> {
        sqlx::query_as::<_, DeliveryRow>(
            "SELECT id, webhook_id, event, status, response_code, attempts, last_attempt,
                    payload, response_body, error
             FROM webhook_deliveries WHERE webhook_id = $1
             ORDER BY last_attempt DESC LIMIT $2",
        )
        .bind(webhook_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("deliveries query failed: {e}")))
    }

    /// One delivery, for redelivery.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing.
    pub async fn delivery(&self, webhook_id: i64, id: i64) -> Result<DeliveryRow, AppError> {
        sqlx::query_as::<_, DeliveryRow>(
            "SELECT id, webhook_id, event, status, response_code, attempts, last_attempt,
                    payload, response_body, error
             FROM webhook_deliveries WHERE webhook_id = $1 AND id = $2",
        )
        .bind(webhook_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("delivery query failed: {e}")))?
        .ok_or_else(|| AppError::not_found("delivery", id))
    }

    /// The newest attempt for every webhook.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn last_deliveries(&self) -> Result<Vec<LastDelivery>, AppError> {
        sqlx::query_as::<_, LastDelivery>(
            "SELECT DISTINCT ON (webhook_id) webhook_id, status, last_attempt
             FROM webhook_deliveries ORDER BY webhook_id, last_attempt DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("last deliveries query failed: {e}")))
    }

    /// Enabled webhooks whose newest attempt did not succeed.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn failing_count(&self) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM webhooks w
             WHERE w.enabled AND (
               SELECT d.status FROM webhook_deliveries d WHERE d.webhook_id = w.id
               ORDER BY d.last_attempt DESC LIMIT 1
             ) IN ('failed', 'dead')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("failing webhooks query failed: {e}")))
    }
}
