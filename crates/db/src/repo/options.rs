//! Options repository (typed JSONB key-value).

use serde_json::Value;
use sqlx::PgPool;
use vyasa_common::AppError;

/// Data access for the `options` table.
#[derive(Clone, Debug)]
pub struct OptionsRepo {
    pool: PgPool,
}

impl OptionsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Published posts whose addresses may change with site settings.
    /// # Errors
    /// Returns a database error on failure.
    pub async fn published_posts(&self) -> Result<Vec<crate::content_models::PostRow>, AppError> {
        sqlx::query_as(
            "SELECT *, type AS post_type FROM posts WHERE status = 'published' AND type = 'post'",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(e.to_string()))
    }

    /// Saves a setting and its address redirects atomically.
    /// # Errors
    /// Returns a database error on failure; nothing is committed.
    pub async fn set_with_redirects(
        &self,
        key: &str,
        value: &Value,
        redirects: &[(String, String)],
    ) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::db(e.to_string()))?;
        for (from, to) in redirects {
            sqlx::query("DELETE FROM redirects WHERE from_path = $1")
                .bind(to)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::db(e.to_string()))?;
            sqlx::query("UPDATE redirects SET to_path = $2 WHERE to_path = $1")
                .bind(from)
                .bind(to)
                .execute(&mut *tx)
                .await
                .map_err(|e| AppError::db(e.to_string()))?;
            sqlx::query("INSERT INTO redirects (from_path, to_path) VALUES ($1, $2) ON CONFLICT (from_path) DO UPDATE SET to_path = $2").bind(from).bind(to).execute(&mut *tx).await.map_err(|e| AppError::db(e.to_string()))?;
        }
        sqlx::query("INSERT INTO options (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()")
            .bind(key).bind(value).execute(&mut *tx).await.map_err(|e| AppError::db(e.to_string()))?;
        tx.commit().await.map_err(|e| AppError::db(e.to_string()))
    }

    /// Fetches an option value by key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing, [`AppError::Db`] on failure.
    pub async fn get(&self, key: &str) -> Result<Value, AppError> {
        sqlx::query_scalar::<_, Value>("SELECT value FROM options WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("option get failed: {err}")))?
            .ok_or_else(|| AppError::not_found("option", key))
    }

    /// Fetches an option or returns `default` when missing.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn get_or_default(&self, key: &str, default: Value) -> Result<Value, AppError> {
        match self.get(key).await {
            Ok(value) => Ok(value),
            Err(AppError::NotFound { .. }) => Ok(default),
            Err(err) => Err(err),
        }
    }

    /// Sets an option (upsert).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set(&self, key: &str, value: &Value) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO options (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("option set failed: {err}")))?;
        Ok(())
    }

    /// The capability names of the custom role `slug`, or `None` when
    /// there is no such role: what validating an option that names a role
    /// needs to know.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn custom_role_capabilities(
        &self,
        slug: &str,
    ) -> Result<Option<Vec<String>>, AppError> {
        sqlx::query_scalar("SELECT capabilities FROM roles WHERE slug = $1")
            .bind(slug)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("role lookup failed: {err}")))
    }
}
