//! API key repository.

use sqlx::PgPool;
use vyasa_common::AppError;

use super::users::USER_COLUMNS;
use crate::models::{ApiKeyRow, UserRow};

/// Data access for the `api_keys` table.
#[derive(Clone, Debug)]
pub struct ApiKeysRepo {
    pool: PgPool,
}

const KEY_COLUMNS: &str = "id, user_id, name, key_hash, capabilities, last_used_at,
                           created_at, revoked_at";

impl ApiKeysRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Resolves a live (non-revoked) key by hash, together with its owner.
    /// A suspended owner's keys do not resolve: suspension ended the
    /// account's sessions but used to leave every key it had working. Nor
    /// do those of an owner whose address is unconfirmed (phase 98).
    /// Updates `last_used_at` best-effort.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when the key is unknown or revoked or
    /// its owner is suspended or unconfirmed, or [`AppError::Db`] on
    /// database failure.
    pub async fn resolve(&self, key_hash: &str) -> Result<(ApiKeyRow, UserRow), AppError> {
        let key: ApiKeyRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {KEY_COLUMNS} FROM api_keys
             WHERE key_hash = $1 AND revoked_at IS NULL"
        )))
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("api key lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("api_key", "hash"))?;

        let user: UserRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE id = $1"
        )))
        .bind(key.user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("api key owner lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("user", key.user_id))?;
        // Nor do the keys of an account whose address was never confirmed:
        // it cannot sign in, so it has no business holding a working key.
        if user.suspended_at.is_some() || user.email_verified_at.is_none() {
            return Err(AppError::not_found("api_key", "hash"));
        }

        // Best-effort usage stamp; failures are not fatal.
        let _ = sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
            .bind(key.id)
            .execute(&self.pool)
            .await;

        Ok((key, user))
    }

    /// Creates a key; returns the stored row (hash only — the raw key is
    /// shown once by the caller).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn insert(
        &self,
        id: i64,
        user_id: i64,
        name: &str,
        key_hash: &str,
        capabilities: &serde_json::Value,
    ) -> Result<ApiKeyRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO api_keys (id, user_id, name, key_hash, capabilities)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING {KEY_COLUMNS}"
        )))
        .bind(id)
        .bind(user_id)
        .bind(name)
        .bind(key_hash)
        .bind(capabilities)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("api key insert failed: {err}")))
    }

    /// Revokes a key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn revoke(&self, id: i64) -> Result<(), AppError> {
        sqlx::query("UPDATE api_keys SET revoked_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("api key revoke failed: {err}")))?;
        Ok(())
    }

    /// Revokes every live key a user holds; returns how many.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn revoke_all_for_user(&self, user_id: i64) -> Result<u64, AppError> {
        sqlx::query(
            "UPDATE api_keys SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected())
        .map_err(|err| AppError::db(format!("api key revoke failed: {err}")))
    }

    /// Lists a user's keys.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_for_user(&self, user_id: i64) -> Result<Vec<ApiKeyRow>, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {KEY_COLUMNS} FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC"
        )))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("api key list failed: {err}")))
    }
}
