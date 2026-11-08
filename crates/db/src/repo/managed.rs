//! Managed entries: posts a plugin keeps in step with an outside source.

use sqlx::PgPool;
use vyasa_common::AppError;

/// One managed entry's bookkeeping row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ManagedRow {
    /// The entry.
    pub post_id: i64,
    /// The owning plugin.
    pub plugin_id: i64,
    /// The plugin's own identity for it.
    pub ext_key: String,
    /// What the plugin last wrote.
    pub source_hash: String,
    /// Where editors go to change it (empty for nowhere).
    pub edit_url: String,
    /// An author took the entry over; the plugin leaves it alone.
    pub detached: bool,
}

/// An entry's managed state with its plugin's name, for the editor.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ManagedInfoRow {
    /// Plugin name.
    pub plugin: String,
    /// The plugin's key for the entry.
    pub ext_key: String,
    /// Where editors go to change it.
    pub edit_url: String,
    /// Whether an author took it over.
    pub detached: bool,
}

const COLUMNS: &str = "post_id, plugin_id, ext_key, source_hash, edit_url, detached";

/// Data access for `managed_entries`.
#[derive(Clone, Debug)]
pub struct ManagedRepo {
    pool: PgPool,
}

impl ManagedRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The row for a plugin's key.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn find(&self, plugin_id: i64, key: &str) -> Result<Option<ManagedRow>, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM managed_entries WHERE plugin_id = $1 AND ext_key = $2"
        )))
        .bind(plugin_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("managed lookup failed: {e}")))
    }

    /// The managed state of an entry, when it has one.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn for_post(&self, post_id: i64) -> Result<Option<ManagedInfoRow>, AppError> {
        sqlx::query_as(
            "SELECT p.name AS plugin, m.ext_key, m.edit_url, m.detached
             FROM managed_entries m JOIN plugins p ON p.id = m.plugin_id
             WHERE m.post_id = $1",
        )
        .bind(post_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("managed lookup failed: {e}")))
    }

    /// Which of `ids` are managed and not detached (read-only in the editor).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn locked_among(&self, ids: &[i64]) -> Result<Vec<(i64, String, String)>, AppError> {
        sqlx::query_as(
            "SELECT m.post_id, p.name, m.edit_url
             FROM managed_entries m JOIN plugins p ON p.id = m.plugin_id
             WHERE m.post_id = ANY($1) AND NOT m.detached",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("managed lookup failed: {e}")))
    }

    /// Every entry of `post_type` a plugin manages and has not been
    /// detached from.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list(&self, plugin_id: i64, post_type: &str) -> Result<Vec<ManagedRow>, AppError> {
        sqlx::query_as(
            "SELECT m.post_id, m.plugin_id, m.ext_key, m.source_hash, m.edit_url, m.detached
             FROM managed_entries m JOIN posts p ON p.id = m.post_id
             WHERE m.plugin_id = $1 AND p.type = $2 AND NOT m.detached
             ORDER BY m.ext_key",
        )
        .bind(plugin_id)
        .bind(post_type)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("managed list failed: {e}")))
    }

    /// Records (or updates) a plugin's key for an entry.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn upsert_row(
        &self,
        post_id: i64,
        plugin_id: i64,
        key: &str,
        source_hash: &str,
        edit_url: &str,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO managed_entries (post_id, plugin_id, ext_key, source_hash, edit_url)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (post_id) DO UPDATE
                SET source_hash = $4, edit_url = $5, updated_at = now()",
        )
        .bind(post_id)
        .bind(plugin_id)
        .bind(key)
        .bind(source_hash)
        .bind(edit_url)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("managed write failed: {e}")))?;
        Ok(())
    }

    /// Marks an entry detached (an author took it over) or attached again.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_detached(&self, post_id: i64, detached: bool) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE managed_entries SET detached = $2, updated_at = now() WHERE post_id = $1",
        )
        .bind(post_id)
        .bind(detached)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("managed write failed: {e}")))?;
        Ok(())
    }

    /// Bumps `updated_at` without changing anything else.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn touch(&self, post_id: i64) -> Result<(), AppError> {
        sqlx::query("UPDATE managed_entries SET updated_at = now() WHERE post_id = $1")
            .bind(post_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("managed write failed: {e}")))?;
        Ok(())
    }
}
