//! Registered AI providers and models (migration 0022).

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use vyasa_common::AppError;

/// One provider with its stored credential.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AiProviderRow {
    /// `anthropic`, `openai` or `openrouter`.
    pub provider: String,
    /// Stored key, prefixed `v1:` (encrypted) or `plain:`; empty when unset.
    pub api_key: String,
    /// Base URL override; empty means the provider's default.
    pub base_url: String,
    /// Whether the provider may be used at all.
    pub enabled: bool,
    /// Last change.
    pub updated_at: DateTime<Utc>,
}

/// One registered model.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AiModelRow {
    /// Snowflake id.
    pub id: i64,
    /// Owning provider.
    pub provider: String,
    /// The provider's model identifier.
    pub model: String,
    /// What the model is registered for (`text`, `embedding`, …).
    pub kind: String,
    /// Display name.
    pub label: String,
    /// Whether features may use it.
    pub enabled: bool,
    /// The one model of its kind features reach for first.
    pub is_default: bool,
    /// Free-form per-model settings (max tokens, dimensions, image size…).
    pub settings: serde_json::Value,
    /// USD per million input tokens, when known.
    pub input_cost_per_mtok: Option<f64>,
    /// USD per million output tokens, when known.
    pub output_cost_per_mtok: Option<f64>,
    /// Position within its kind after the default; lower first.
    #[sqlx(default)]
    pub sort_order: i32,
    /// Result of the last connectivity probe.
    pub last_probe_ok: Option<bool>,
    /// Human-readable probe outcome.
    pub last_probe_detail: String,
    /// When the last probe ran.
    pub last_probe_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last change.
    pub updated_at: DateTime<Utc>,
}

/// Fields for a new model.
#[derive(Debug, Clone)]
pub struct NewAiModel<'a> {
    /// Owning provider.
    pub provider: &'a str,
    /// Provider model id.
    pub model: &'a str,
    /// Kind of job.
    pub kind: &'a str,
    /// Display name.
    pub label: &'a str,
    /// Per-model settings.
    pub settings: serde_json::Value,
    /// Input cost, USD per Mtok.
    pub input_cost_per_mtok: Option<f64>,
    /// Output cost, USD per Mtok.
    pub output_cost_per_mtok: Option<f64>,
}

/// Editable fields; `None` leaves the column alone.
#[derive(Debug, Clone, Default)]
pub struct AiModelUpdate<'a> {
    /// Display name.
    pub label: Option<&'a str>,
    /// Enabled flag.
    pub enabled: Option<bool>,
    /// Per-model settings.
    pub settings: Option<serde_json::Value>,
    /// Input cost, USD per Mtok (`Some(None)` clears).
    pub input_cost_per_mtok: Option<Option<f64>>,
    /// Output cost, USD per Mtok (`Some(None)` clears).
    pub output_cost_per_mtok: Option<Option<f64>>,
}

fn db_err(e: &sqlx::Error) -> AppError {
    AppError::db(format!("ai_models: {e}"))
}

const MODEL_COLS: &str = "id, provider, model, kind, label, enabled, is_default, settings, \
     input_cost_per_mtok, output_cost_per_mtok, last_probe_ok, last_probe_detail, \
     last_probe_at, created_at, updated_at, sort_order";

/// Data access for `ai_providers` and `ai_models`.
#[derive(Clone, Debug)]
pub struct AiModelsRepo {
    pool: PgPool,
}

impl AiModelsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Every provider row that exists.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list_providers(&self) -> Result<Vec<AiProviderRow>, AppError> {
        sqlx::query_as::<_, AiProviderRow>(
            "SELECT provider, api_key, base_url, enabled, updated_at FROM ai_providers \
             ORDER BY provider",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// One provider, if configured.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn get_provider(&self, provider: &str) -> Result<Option<AiProviderRow>, AppError> {
        sqlx::query_as::<_, AiProviderRow>(
            "SELECT provider, api_key, base_url, enabled, updated_at FROM ai_providers \
             WHERE provider = $1",
        )
        .bind(provider)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// Creates or updates a provider. `api_key = None` keeps the stored key.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn upsert_provider(
        &self,
        provider: &str,
        api_key: Option<&str>,
        base_url: &str,
        enabled: bool,
    ) -> Result<AiProviderRow, AppError> {
        sqlx::query_as::<_, AiProviderRow>(
            "INSERT INTO ai_providers (provider, api_key, base_url, enabled) \
             VALUES ($1, COALESCE($2, ''), $3, $4) \
             ON CONFLICT (provider) DO UPDATE SET \
               api_key = COALESCE($2, ai_providers.api_key), \
               base_url = EXCLUDED.base_url, \
               enabled = EXCLUDED.enabled, \
               updated_at = now() \
             RETURNING provider, api_key, base_url, enabled, updated_at",
        )
        .bind(provider)
        .bind(api_key)
        .bind(base_url)
        .bind(enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// Removes a provider and, through the foreign key, its models.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn delete_provider(&self, provider: &str) -> Result<bool, AppError> {
        let done = sqlx::query("DELETE FROM ai_providers WHERE provider = $1")
            .bind(provider)
            .execute(&self.pool)
            .await
            .map_err(|e| db_err(&e))?;
        Ok(done.rows_affected() > 0)
    }

    /// Every registered model, defaults first within a kind.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list_models(&self) -> Result<Vec<AiModelRow>, AppError> {
        sqlx::query_as::<_, AiModelRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {MODEL_COLS} FROM ai_models ORDER BY kind, is_default DESC, sort_order, created_at"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// Sets the fallback order of `ids` within `kind`: position in the
    /// list becomes `sort_order`. Ids of another kind are ignored.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_order(&self, kind: &str, ids: &[i64]) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::db(format!("order tx: {e}")))?;
        for (position, id) in ids.iter().enumerate() {
            sqlx::query("UPDATE ai_models SET sort_order = $1 WHERE id = $2 AND kind = $3")
                .bind(i32::try_from(position).unwrap_or(i32::MAX))
                .bind(id)
                .bind(kind)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::db(format!("order update: {e}")))?;
        }
        tx.commit()
            .await
            .map_err(|e| AppError::db(format!("order commit: {e}")))
    }

    /// Month-to-date calls and spend per model, UTC calendar month.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn month_spend_by_model(&self) -> Result<Vec<(String, String, i64, f64)>, AppError> {
        sqlx::query_as::<_, (String, String, i64, f64)>(
            "SELECT provider, model, count(*), COALESCE(SUM(cost_usd), 0)::float8 FROM ai_logs
             WHERE date_trunc('month', created_at) = date_trunc('month', now())
             GROUP BY provider, model",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("ai spend by model failed: {e}")))
    }

    /// Enabled models of one kind, the default first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list_enabled_by_kind(&self, kind: &str) -> Result<Vec<AiModelRow>, AppError> {
        sqlx::query_as::<_, AiModelRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {MODEL_COLS} FROM ai_models m \
             WHERE kind = $1 AND enabled \
               AND EXISTS (SELECT 1 FROM ai_providers p WHERE p.provider = m.provider AND p.enabled) \
             ORDER BY is_default DESC, sort_order, created_at"
        )))
        .bind(kind)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// One model by id.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing; [`AppError::Db`] on failure.
    pub async fn get_model(&self, id: i64) -> Result<AiModelRow, AppError> {
        sqlx::query_as::<_, AiModelRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {MODEL_COLS} FROM ai_models WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err(&e))?
        .ok_or_else(|| AppError::not_found("ai_model", id))
    }

    /// Registers a model. The first model of a kind becomes its default.
    ///
    /// # Errors
    /// [`AppError::Conflict`] when the same model is already registered for
    /// that kind; [`AppError::Db`] on failure.
    pub async fn insert_model(&self, new: &NewAiModel<'_>) -> Result<AiModelRow, AppError> {
        let result = sqlx::query_as::<_, AiModelRow>(sqlx::AssertSqlSafe(format!(
            "INSERT INTO ai_models (id, provider, model, kind, label, settings, \
               input_cost_per_mtok, output_cost_per_mtok, is_default) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
               NOT EXISTS (SELECT 1 FROM ai_models WHERE kind = $4 AND is_default)) \
             RETURNING {MODEL_COLS}"
        )))
        .bind(vyasa_common::next_id_i64())
        .bind(new.provider)
        .bind(new.model)
        .bind(new.kind)
        .bind(new.label)
        .bind(&new.settings)
        .bind(new.input_cost_per_mtok)
        .bind(new.output_cost_per_mtok)
        .fetch_one(&self.pool)
        .await;
        match result {
            Ok(row) => Ok(row),
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                Err(AppError::conflict(format!(
                    "{} is already registered at {} for {}",
                    new.model, new.provider, new.kind
                )))
            }
            Err(e) => Err(db_err(&e)),
        }
    }

    /// Edits a model's mutable fields.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing; [`AppError::Db`] on failure.
    pub async fn update_model(
        &self,
        id: i64,
        update: &AiModelUpdate<'_>,
    ) -> Result<AiModelRow, AppError> {
        sqlx::query_as::<_, AiModelRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE ai_models SET \
               label = COALESCE($2, label), \
               enabled = COALESCE($3, enabled), \
               settings = COALESCE($4, settings), \
               input_cost_per_mtok = CASE WHEN $5 THEN $6 ELSE input_cost_per_mtok END, \
               output_cost_per_mtok = CASE WHEN $7 THEN $8 ELSE output_cost_per_mtok END, \
               updated_at = now() \
             WHERE id = $1 RETURNING {MODEL_COLS}"
        )))
        .bind(id)
        .bind(update.label)
        .bind(update.enabled)
        .bind(&update.settings)
        .bind(update.input_cost_per_mtok.is_some())
        .bind(update.input_cost_per_mtok.flatten())
        .bind(update.output_cost_per_mtok.is_some())
        .bind(update.output_cost_per_mtok.flatten())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err(&e))?
        .ok_or_else(|| AppError::not_found("ai_model", id))
    }

    /// Makes `id` the default of its kind, clearing the previous default
    /// in the same transaction so the partial unique index never trips.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing; [`AppError::Db`] on failure.
    pub async fn set_default(&self, id: i64) -> Result<AiModelRow, AppError> {
        let mut tx = self.pool.begin().await.map_err(|e| db_err(&e))?;
        let kind: Option<String> = sqlx::query_scalar("SELECT kind FROM ai_models WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| db_err(&e))?;
        let Some(kind) = kind else {
            return Err(AppError::not_found("ai_model", id));
        };
        sqlx::query("UPDATE ai_models SET is_default = false WHERE kind = $1 AND is_default")
            .bind(&kind)
            .execute(&mut *tx)
            .await
            .map_err(|e| db_err(&e))?;
        let row = sqlx::query_as::<_, AiModelRow>(sqlx::AssertSqlSafe(format!(
            "UPDATE ai_models SET is_default = true, enabled = true, updated_at = now() \
             WHERE id = $1 RETURNING {MODEL_COLS}"
        )))
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| db_err(&e))?;
        tx.commit().await.map_err(|e| db_err(&e))?;
        Ok(row)
    }

    /// Records a probe outcome.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn record_probe(&self, id: i64, ok: bool, detail: &str) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE ai_models SET last_probe_ok = $2, last_probe_detail = $3, \
             last_probe_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(ok)
        .bind(detail)
        .execute(&self.pool)
        .await
        .map_err(|e| db_err(&e))?;
        Ok(())
    }

    /// Deletes a model. If it was the default, the oldest enabled model of
    /// the same kind takes over so features never lose their default silently.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn delete_model(&self, id: i64) -> Result<bool, AppError> {
        let mut tx = self.pool.begin().await.map_err(|e| db_err(&e))?;
        let victim: Option<(String, bool)> =
            sqlx::query_as("SELECT kind, is_default FROM ai_models WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| db_err(&e))?;
        let Some((kind, was_default)) = victim else {
            return Ok(false);
        };
        sqlx::query("DELETE FROM ai_models WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| db_err(&e))?;
        if was_default {
            sqlx::query(
                "UPDATE ai_models SET is_default = true WHERE id = \
                 (SELECT id FROM ai_models WHERE kind = $1 AND enabled ORDER BY created_at LIMIT 1)",
            )
            .bind(&kind)
            .execute(&mut *tx)
            .await
            .map_err(|e| db_err(&e))?;
        }
        tx.commit().await.map_err(|e| db_err(&e))?;
        Ok(true)
    }
}
