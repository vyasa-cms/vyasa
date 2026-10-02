//! Themes repository: versioned theme rows + single-active pin.

use serde_json::Value;
use sqlx::PgPool;
use vyasa_common::AppError;

/// One stored theme version.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThemeRow {
    /// Snowflake id.
    pub id: i64,
    /// Package name (slug-ish).
    pub name: String,
    /// Integer package version.
    pub version: i32,
    /// Whether this version is the active one.
    pub is_active: bool,
    /// Design tokens (JSONB).
    pub tokens: Value,
    /// Layout composition (JSONB).
    pub layout: Value,
    /// Optional Tera overrides keyed by template name.
    pub templates: Option<Value>,
    /// CSS and JavaScript the theme ships with itself.
    pub assets: Option<Value>,
    /// Parent theme for child themes.
    pub parent_theme_id: Option<i64>,
    /// The package manifest's version for an installed package; `None`
    /// for a studio publish.
    pub package_version: Option<i32>,
}

/// A bundled file to store with a theme version.
#[derive(Debug, Clone)]
pub struct ThemeFileInput {
    /// Path under `assets/` (`images/hero.jpg`).
    pub path: String,
    /// MIME type decided at install.
    pub content_type: String,
    /// Hex SHA-256 of `bytes`, the ETag the route serves.
    pub sha256: String,
    /// Contents.
    pub bytes: Vec<u8>,
}

/// A bundled file's description, for a listing.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThemeFileMeta {
    /// Path under `assets/`.
    pub path: String,
    /// MIME type.
    pub content_type: String,
    /// Hex SHA-256 of the bytes.
    pub sha256: String,
    /// Size in bytes.
    pub size: i32,
}

/// A bundled file read back for serving.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThemeFileRow {
    /// Path under `assets/`.
    pub path: String,
    /// MIME type.
    pub content_type: String,
    /// Hex SHA-256 of `bytes`.
    pub sha256: String,
    /// Contents.
    pub bytes: Vec<u8>,
}

/// Data access for the `themes` table.
#[derive(Clone, Debug)]
pub struct ThemesRepo {
    pool: PgPool,
}

impl ThemesRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a new theme version.
    ///
    /// # Errors
    /// Returns [`AppError::Conflict`] when `(name, version)` already
    /// exists, [`AppError::Db`] on failure.
    #[allow(clippy::too_many_arguments)] // one row, one column each
    pub async fn insert_version(
        &self,
        name: &str,
        version: i32,
        tokens: Value,
        layout: Value,
        templates: Option<Value>,
        assets: Option<Value>,
        parent_theme_id: Option<i64>,
    ) -> Result<ThemeRow, AppError> {
        let row = sqlx::query_as::<_, ThemeRow>(
            "INSERT INTO themes (id, name, version, tokens, layout, templates, assets, parent_theme_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id, name, version, is_active, tokens, layout, templates, assets, \
                       parent_theme_id, package_version",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(name)
        .bind(version)
        .bind(tokens)
        .bind(layout)
        .bind(templates)
        .bind(assets)
        .bind(parent_theme_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| match err {
            sqlx::Error::Database(ref db_err)
                if db_err.constraint().is_some_and(|c| c.contains("uniq")) =>
            {
                AppError::conflict(format!("theme {name} v{version} already installed"))
            }
            _ => AppError::db(format!("theme insert failed: {err}")),
        })?;
        Ok(row)
    }

    /// Records the package version an installed row came from.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn set_package_version(&self, id: i64, package_version: i32) -> Result<(), AppError> {
        sqlx::query("UPDATE themes SET package_version = $2 WHERE id = $1")
            .bind(id)
            .bind(package_version)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("theme package version: {e}")))?;
        Ok(())
    }

    /// The newest package version installed under `name`, if any row
    /// came from a package.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn latest_package_version(&self, name: &str) -> Result<Option<i32>, AppError> {
        sqlx::query_scalar::<_, Option<i32>>(
            "SELECT MAX(package_version) FROM themes WHERE name = $1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("theme package version: {e}")))
    }

    /// Whether a row under `name` came from exactly this package version.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn has_package_version(
        &self,
        name: &str,
        package_version: i32,
    ) -> Result<bool, AppError> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM themes WHERE name = $1 AND package_version = $2)",
        )
        .bind(name)
        .bind(package_version)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("theme package version: {e}")))
    }

    /// The newest theme row, of any theme, that bundles `path`: what a
    /// preview of a draft based on a non-active theme falls back to.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn file_any(&self, path: &str) -> Result<Option<ThemeFileRow>, AppError> {
        sqlx::query_as::<_, ThemeFileRow>(
            "SELECT f.path, f.content_type, f.sha256, f.bytes FROM theme_files f
             JOIN themes t ON t.id = f.theme_id WHERE f.path = $1
             ORDER BY t.is_active DESC, t.created_at DESC LIMIT 1",
        )
        .bind(path)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("theme file: {e}")))
    }

    /// Stores the files a theme version bundles, replacing any it had.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn put_files(&self, theme_id: i64, files: &[ThemeFileInput]) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::db(format!("theme files: {e}")))?;
        sqlx::query("DELETE FROM theme_files WHERE theme_id = $1")
            .bind(theme_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::db(format!("theme files: {e}")))?;
        for f in files {
            sqlx::query(
                "INSERT INTO theme_files (theme_id, path, content_type, sha256, bytes)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(theme_id)
            .bind(&f.path)
            .bind(&f.content_type)
            .bind(&f.sha256)
            .bind(&f.bytes)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::db(format!("theme file {}: {e}", f.path)))?;
        }
        tx.commit()
            .await
            .map_err(|e| AppError::db(format!("theme files: {e}")))
    }

    /// Copies every bundled file from one version to another, so a
    /// version the studio publishes keeps the pictures its base shipped.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn copy_files(&self, from_theme_id: i64, to_theme_id: i64) -> Result<u64, AppError> {
        let done = sqlx::query(
            "INSERT INTO theme_files (theme_id, path, content_type, sha256, bytes)
             SELECT $2, path, content_type, sha256, bytes FROM theme_files WHERE theme_id = $1
             ON CONFLICT (theme_id, path) DO NOTHING",
        )
        .bind(from_theme_id)
        .bind(to_theme_id)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("copy theme files: {e}")))?;
        Ok(done.rows_affected())
    }

    /// Adds or replaces one bundled file of a theme version.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn upsert_file(&self, theme_id: i64, file: &ThemeFileInput) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO theme_files (theme_id, path, content_type, sha256, bytes)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (theme_id, path) DO UPDATE
             SET content_type = EXCLUDED.content_type, sha256 = EXCLUDED.sha256,
                 bytes = EXCLUDED.bytes",
        )
        .bind(theme_id)
        .bind(&file.path)
        .bind(&file.content_type)
        .bind(&file.sha256)
        .bind(&file.bytes)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("theme file {}: {e}", file.path)))?;
        Ok(())
    }

    /// Removes one bundled file. `Ok(false)` when there was none.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn delete_file(&self, theme_id: i64, path: &str) -> Result<bool, AppError> {
        let done = sqlx::query("DELETE FROM theme_files WHERE theme_id = $1 AND path = $2")
            .bind(theme_id)
            .bind(path)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("theme file: {e}")))?;
        Ok(done.rows_affected() > 0)
    }

    /// Every bundled file of a theme version, without the bytes.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn files_meta(&self, theme_id: i64) -> Result<Vec<ThemeFileMeta>, AppError> {
        sqlx::query_as::<_, ThemeFileMeta>(
            "SELECT path, content_type, sha256, octet_length(bytes) AS size
             FROM theme_files WHERE theme_id = $1 ORDER BY path",
        )
        .bind(theme_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("theme files: {e}")))
    }

    /// Total bytes a theme version bundles.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn files_total_bytes(&self, theme_id: i64) -> Result<i64, AppError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT coalesce(sum(octet_length(bytes)), 0)::bigint FROM theme_files WHERE theme_id = $1",
        )
        .bind(theme_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("theme files: {e}")))
    }

    /// One bundled file of a theme version, by its path under `assets/`.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure; a missing file is `Ok(None)`.
    pub async fn file(&self, theme_id: i64, path: &str) -> Result<Option<ThemeFileRow>, AppError> {
        sqlx::query_as::<_, ThemeFileRow>(
            "SELECT path, content_type, sha256, bytes FROM theme_files
             WHERE theme_id = $1 AND path = $2",
        )
        .bind(theme_id)
        .bind(path)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("theme file: {e}")))
    }

    /// The paths a theme version bundles, sorted, without their bytes.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on failure.
    pub async fn file_paths(&self, theme_id: i64) -> Result<Vec<String>, AppError> {
        sqlx::query_scalar::<_, String>(
            "SELECT path FROM theme_files WHERE theme_id = $1 ORDER BY path",
        )
        .bind(theme_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("theme files: {e}")))
    }

    /// One theme version by id.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] when missing, [`AppError::Db`] on
    /// failure.
    pub async fn get(&self, id: i64) -> Result<ThemeRow, AppError> {
        sqlx::query_as::<_, ThemeRow>(
            "SELECT id, name, version, is_active, tokens, layout, templates, assets, parent_theme_id, package_version
             FROM themes WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("theme lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("theme", id))
    }

    /// The highest installed version of `name`, or 0 when none.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn latest_version(&self, name: &str) -> Result<i32, AppError> {
        sqlx::query_scalar::<_, i32>("SELECT COALESCE(MAX(version), 0) FROM themes WHERE name = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("theme version lookup failed: {err}")))
    }

    /// Deletes an installed version. Refuses the active one.
    ///
    /// # Errors
    /// Returns [`AppError::Conflict`] for the active theme,
    /// [`AppError::NotFound`] when missing, [`AppError::Db`] on failure.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let row = self.get(id).await?;
        if row.is_active {
            return Err(AppError::conflict(format!(
                "theme {} v{} is live; activate another theme first",
                row.name, row.version
            )));
        }
        sqlx::query("DELETE FROM themes WHERE id = $1 AND NOT is_active")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("theme delete failed: {err}")))?;
        Ok(())
    }

    /// Which theme is active, as `(name, version)` and nothing else.
    ///
    /// The public site asks this on every request to decide whether the
    /// bundle it already built is still the one to serve. That decision
    /// needs two small columns, not the templates, tokens and layout
    /// documents that [`Self::get_active`] carries.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Db` on database failure.
    pub async fn active_identity(&self) -> Result<Option<(String, i32)>, AppError> {
        sqlx::query_as::<_, (String, i32)>(
            "SELECT name, version FROM themes WHERE is_active LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("theme active_identity failed: {err}")))
    }

    /// The currently active theme version, if any.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn get_active(&self) -> Result<Option<ThemeRow>, AppError> {
        sqlx::query_as::<_, ThemeRow>(
            "SELECT id, name, version, is_active, tokens, layout, templates, assets, parent_theme_id, package_version
             FROM themes WHERE is_active LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("theme get_active failed: {err}")))
    }

    /// Activates `(name, version)`, deactivating anything else atomically.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] when the version is not installed,
    /// [`AppError::Db`] on failure.
    pub async fn set_active(&self, name: &str, version: i32) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("theme tx begin failed: {err}")))?;
        sqlx::query("UPDATE themes SET is_active = false WHERE is_active")
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("theme deactivate failed: {err}")))?;
        let updated =
            sqlx::query("UPDATE themes SET is_active = true WHERE name = $1 AND version = $2")
                .bind(name)
                .bind(version)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("theme activate failed: {err}")))?
                .rows_affected();
        if updated == 0 {
            return Err(AppError::not_found("theme", format!("{name} v{version}")));
        }
        // Exactly-one-active is enforced by the partial unique index, but a
        // same-statement swap keeps it honest under concurrency.
        tx.commit()
            .await
            .map_err(|err| AppError::db(format!("theme activate commit failed: {err}")))?;
        Ok(())
    }

    /// All installed versions, newest first.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self) -> Result<Vec<ThemeRow>, AppError> {
        sqlx::query_as::<_, ThemeRow>(
            "SELECT id, name, version, is_active, tokens, layout, templates, assets, parent_theme_id, package_version
             FROM themes ORDER BY name ASC, version DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("theme list failed: {err}")))
    }

    /// Latest installed version of `name` strictly below `before_version`
    /// (rollback target).
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] when no earlier version exists,
    /// [`AppError::Db`] on failure.
    pub async fn latest_below(
        &self,
        name: &str,
        before_version: i32,
    ) -> Result<ThemeRow, AppError> {
        sqlx::query_as::<_, ThemeRow>(
            "SELECT id, name, version, is_active, tokens, layout, templates, assets, parent_theme_id, package_version
             FROM themes WHERE name = $1 AND version < $2
             ORDER BY version DESC LIMIT 1",
        )
        .bind(name)
        .bind(before_version)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("theme rollback lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("theme", format!("{name} < v{before_version}")))
    }
}
