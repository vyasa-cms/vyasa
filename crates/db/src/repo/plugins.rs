//! Plugins repository: registry rows, versioned wasm bytes, status.

use sqlx::PgPool;
use vyasa_common::AppError;

/// Registry row for one plugin.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PluginRow {
    /// Snowflake id.
    pub id: i64,
    /// Unique package name.
    pub name: String,
    /// Active version string.
    pub version: String,
    /// Whether dispatch may use it.
    pub enabled: bool,
    /// Lifecycle status (`installed|loaded|degraded|errored|disabled`).
    pub status: String,
    /// Declared capabilities (JSON array of strings).
    pub capabilities: serde_json::Value,
    /// sha256 of the active wasm artifact.
    pub wasm_sha256: String,
    /// From the manifest.
    #[sqlx(default)]
    pub description: String,
    /// From the manifest.
    #[sqlx(default)]
    pub author: String,
    /// From the manifest.
    #[sqlx(default)]
    pub homepage: String,
    /// From the manifest.
    #[sqlx(default)]
    pub license: String,
    /// Why the status is degraded or errored, when it is.
    #[sqlx(default)]
    pub status_reason: String,
}

/// One audit row: a denied capability, a fetch, or a quota hit.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct PluginAuditRow {
    /// When.
    pub ts: chrono::DateTime<chrono::Utc>,
    /// `deny`, `fetch` or `quota`.
    pub kind: String,
    /// The capability involved.
    pub capability: String,
    /// What happened.
    pub detail: String,
}

/// One stored wasm artifact.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PluginVersionRow {
    /// Row id.
    pub id: i64,
    /// Owning plugin.
    pub plugin_id: i64,
    /// Version string.
    pub version: String,
    /// Component bytes.
    pub wasm: Vec<u8>,
    /// sha256 hex of `wasm`.
    pub sha256: String,
}

/// Data access for plugins + versions.
#[derive(Clone, Debug)]
pub struct PluginsRepo {
    pool: PgPool,
}

impl PluginsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a plugin with its first version.
    ///
    /// # Errors
    /// [`AppError::Conflict`] on duplicate name; [`AppError::Db`] otherwise.
    pub async fn create(
        &self,
        name: &str,
        version: &str,
        wasm: &[u8],
        sha256: &str,
        capabilities: &serde_json::Value,
    ) -> Result<PluginRow, AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::db(format!("plugin tx: {e}")))?;
        let id = vyasa_common::next_id_i64();
        let row = sqlx::query_as::<_, PluginRow>(
            "INSERT INTO plugins (id, name, version, capabilities, wasm_sha256)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, name, version, enabled, status, capabilities, wasm_sha256",
        )
        .bind(id)
        .bind(name)
        .bind(version)
        .bind(capabilities)
        .bind(sha256)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| match &err {
            sqlx::Error::Database(db)
                if db
                    .constraint()
                    .is_some_and(|c| c.contains("unique") || c.contains("_key")) =>
            {
                AppError::conflict(format!("plugin {name} already installed"))
            }
            _ => AppError::db(format!("plugin insert failed: {err}")),
        })?;
        sqlx::query("INSERT INTO plugin_versions (id, plugin_id, version, wasm, sha256) VALUES ($1, $2, $3, $4, $5)")
            .bind(vyasa_common::next_id_i64())
            .bind(row.id)
            .bind(version)
            .bind(wasm)
            .bind(sha256)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::db(format!("plugin version insert failed: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| AppError::db(format!("plugin commit: {e}")))?;
        Ok(row)
    }

    /// Adds another version for an existing plugin.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when the plugin is missing.
    pub async fn add_version(
        &self,
        plugin_id: i64,
        version: &str,
        wasm: &[u8],
        sha256: &str,
        capabilities: &serde_json::Value,
    ) -> Result<(), AppError> {
        // Capabilities come from the signed manifest of the version being
        // installed, so they must move with it. They used to be written
        // only by `create`, so upgrading a plugin left the row asserting
        // the capabilities of the version it replaced: one that dropped a
        // capability kept being granted it, and one that added a
        // capability could never use it.
        let updated = sqlx::query(
            "UPDATE plugins SET version = $2, wasm_sha256 = $3, capabilities = $4 WHERE id = $1",
        )
        .bind(plugin_id)
        .bind(version)
        .bind(sha256)
        .bind(capabilities)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin version bump failed: {e}")))?
        .rows_affected();
        if updated == 0 {
            return Err(AppError::not_found("plugin", plugin_id));
        }
        sqlx::query("INSERT INTO plugin_versions (id, plugin_id, version, wasm, sha256) VALUES ($1, $2, $3, $4, $5)")
            .bind(vyasa_common::next_id_i64())
            .bind(plugin_id)
            .bind(version)
            .bind(wasm)
            .bind(sha256)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("plugin version insert failed: {e}")))?;
        Ok(())
    }

    /// Enabled plugins ready for load.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list_enabled(&self) -> Result<Vec<PluginRow>, AppError> {
        sqlx::query_as::<_, PluginRow>(
            "SELECT id, name, version, enabled, status, capabilities, wasm_sha256, description, author, homepage, license, status_reason
             FROM plugins WHERE enabled AND status != 'errored' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin list failed: {e}")))
    }

    /// All plugins (admin listing).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn list(&self) -> Result<Vec<PluginRow>, AppError> {
        sqlx::query_as::<_, PluginRow>(
            "SELECT id, name, version, enabled, status, capabilities, wasm_sha256, description, author, homepage, license, status_reason
             FROM plugins ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin list failed: {e}")))
    }

    /// Wasm bytes + hash for a plugin's active version.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing; [`AppError::Db`] on failure.
    pub async fn wasm_bytes(
        &self,
        plugin_id: i64,
        version: &str,
    ) -> Result<(Vec<u8>, String), AppError> {
        sqlx::query_as::<_, PluginVersionRow>(
            "SELECT id, plugin_id, version, wasm, sha256
             FROM plugin_versions WHERE plugin_id = $1 AND version = $2",
        )
        .bind(plugin_id)
        .bind(version)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin wasm load failed: {e}")))?
        .map(|r| (r.wasm, r.sha256))
        .ok_or_else(|| AppError::not_found("plugin_version", format!("{plugin_id}@{version}")))
    }

    /// Records the manifest's metadata after an install or upgrade.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_metadata(
        &self,
        plugin_id: i64,
        description: &str,
        author: &str,
        homepage: &str,
        license: &str,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE plugins SET description = $2, author = $3, homepage = $4, license = $5
             WHERE id = $1",
        )
        .bind(plugin_id)
        .bind(description)
        .bind(author)
        .bind(homepage)
        .bind(license)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin metadata failed: {e}")))?;
        Ok(())
    }

    /// Sets the status and the reason together; an empty reason clears it.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_status_reason(
        &self,
        plugin_id: i64,
        status: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        sqlx::query("UPDATE plugins SET status = $2, status_reason = $3 WHERE id = $1")
            .bind(plugin_id)
            .bind(status)
            .bind(reason.chars().take(500).collect::<String>())
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("plugin status failed: {e}")))?;
        Ok(())
    }

    /// The newest audit rows for a plugin.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn audit_tail(
        &self,
        plugin_id: i64,
        limit: i64,
    ) -> Result<Vec<PluginAuditRow>, AppError> {
        sqlx::query_as::<_, PluginAuditRow>(
            "SELECT ts, kind, capability, detail FROM plugin_audit
             WHERE plugin_id = $1 ORDER BY ts DESC LIMIT $2",
        )
        .bind(plugin_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("audit tail failed: {e}")))
    }

    /// Updates lifecycle status.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_status(&self, plugin_id: i64, status: &str) -> Result<(), AppError> {
        sqlx::query("UPDATE plugins SET status = $2 WHERE id = $1")
            .bind(plugin_id)
            .bind(status)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("plugin status update failed: {e}")))?;
        Ok(())
    }

    /// Enable/disable; disabled plugins are excluded at load time.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_enabled(&self, plugin_id: i64, enabled: bool) -> Result<(), AppError> {
        sqlx::query("UPDATE plugins SET enabled = $2 WHERE id = $1")
            .bind(plugin_id)
            .bind(enabled)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("plugin enable update failed: {e}")))?;
        Ok(())
    }
}

impl PluginsRepo {
    /// Every stored setting for a plugin, as key/value pairs.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn settings(
        &self,
        plugin_id: i64,
    ) -> Result<Vec<(String, serde_json::Value)>, AppError> {
        sqlx::query_as::<_, (String, serde_json::Value)>(
            "SELECT key, value FROM plugin_settings WHERE plugin_id = $1 ORDER BY key",
        )
        .bind(plugin_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin settings read failed: {e}")))
    }

    /// Per-plugin settings kv: read one value.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn setting_get(
        &self,
        plugin_id: i64,
        key: &str,
    ) -> Result<Option<serde_json::Value>, AppError> {
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT value FROM plugin_settings WHERE plugin_id = $1 AND key = $2",
        )
        .bind(plugin_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin setting get failed: {e}")))
    }

    /// Per-plugin settings kv: write one value (upsert).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn setting_put(
        &self,
        plugin_id: i64,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO plugin_settings (plugin_id, key, value, updated_at)
             VALUES ($1, $2, $3, now())
             ON CONFLICT (plugin_id, key) DO UPDATE SET value = $3, updated_at = now()",
        )
        .bind(plugin_id)
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin setting put failed: {e}")))?;
        Ok(())
    }

    /// Audit summary counts per kind for a plugin.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn audit_summary(
        &self,
        pool: &sqlx::PgPool,
        plugin_id: i64,
    ) -> Result<Vec<(String, i64)>, AppError> {
        sqlx::query_as::<_, (String, i64)>(
            "SELECT kind, COUNT(*)::bigint FROM plugin_audit
             WHERE plugin_id = $1 GROUP BY kind ORDER BY kind",
        )
        .bind(plugin_id)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::db(format!("audit summary failed: {e}")))
    }
}

impl PluginsRepo {
    /// All installed version strings, newest first.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn versions(&self, plugin_id: i64) -> Result<Vec<String>, AppError> {
        sqlx::query_scalar::<_, String>(
            "SELECT version FROM plugin_versions WHERE plugin_id = $1 ORDER BY created_at DESC",
        )
        .bind(plugin_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("plugin versions failed: {e}")))
    }

    /// Points the active version at `version` (must exist).
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing; [`AppError::Db`] on failure.
    pub async fn set_version(&self, plugin_id: i64, version: &str) -> Result<(), AppError> {
        let updated = sqlx::query(
            "UPDATE plugins SET version = $2, wasm_sha256 = (
                 SELECT sha256 FROM plugin_versions
                 WHERE plugin_id = $1 AND version = $2
             ) WHERE id = $1",
        )
        .bind(plugin_id)
        .bind(version)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("set_version failed: {e}")))?
        .rows_affected();
        if updated == 0 {
            return Err(AppError::not_found("plugin", plugin_id));
        }
        Ok(())
    }
}

impl PluginsRepo {
    /// Uninstalls a plugin (versions, settings, audit cascade).
    ///
    /// # Errors
    /// [`AppError::NotFound`] when missing; [`AppError::Db`] on failure.
    pub async fn delete_plugin(&self, plugin_id: i64) -> Result<(), AppError> {
        let updated = sqlx::query("DELETE FROM plugins WHERE id = $1")
            .bind(plugin_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("plugin delete failed: {e}")))?
            .rows_affected();
        if updated == 0 {
            return Err(AppError::not_found("plugin", plugin_id));
        }
        Ok(())
    }
}
