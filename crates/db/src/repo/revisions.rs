//! Revisions repository.

use sqlx::PgPool;
use vyasa_common::AppError;

use crate::content_models::PostRevisionRow;

const REVISION_COLUMNS: &str =
    "id, post_id, title, content, layout, fields, author_id, is_autosave, created_at";

/// Data access for the `post_revisions` table.
#[derive(Clone, Debug)]
pub struct RevisionsRepo {
    pool: PgPool,
}

impl RevisionsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a revision.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn insert(&self, rev: &NewRevision<'_>) -> Result<PostRevisionRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO post_revisions
                 (id, post_id, title, content, author_id, is_autosave, layout, fields)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING {REVISION_COLUMNS}"
        )))
        .bind(rev.id)
        .bind(rev.post_id)
        .bind(rev.title)
        .bind(&rev.content)
        .bind(rev.author_id)
        .bind(rev.is_autosave)
        .bind(&rev.layout)
        .bind(&rev.fields)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("revision insert failed: {err}")))
    }

    /// Fetches a revision by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<PostRevisionRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {REVISION_COLUMNS} FROM post_revisions WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("revision lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("revision", id))
    }

    /// Lists revisions for a post, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_for_post(
        &self,
        post_id: i64,
        include_autosave: bool,
    ) -> Result<Vec<PostRevisionRow>, AppError> {
        let rows = if include_autosave {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT {REVISION_COLUMNS} FROM post_revisions
                 WHERE post_id = $1 ORDER BY created_at DESC, id DESC"
            )))
            .bind(post_id)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT {REVISION_COLUMNS} FROM post_revisions
                 WHERE post_id = $1 AND is_autosave = false
                 ORDER BY created_at DESC, id DESC"
            )))
            .bind(post_id)
            .fetch_all(&self.pool)
            .await
        };
        rows.map_err(|err| AppError::db(format!("revision list failed: {err}")))
    }

    /// Fetches the latest revision for a post (if any).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn latest_for_post(&self, post_id: i64) -> Result<Option<PostRevisionRow>, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {REVISION_COLUMNS} FROM post_revisions
             WHERE post_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1"
        )))
        .bind(post_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("revision latest failed: {err}")))
    }

    /// Deletes revisions by ids.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete_ids(&self, ids: &[i64]) -> Result<u64, AppError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let result = sqlx::query("DELETE FROM post_revisions WHERE id = ANY($1)")
            .bind(ids)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("revision bulk delete failed: {err}")))?;
        Ok(result.rows_affected())
    }

    /// Deletes the autosave for a given post+author (if any).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete_autosave(&self, post_id: i64, author_id: i64) -> Result<u64, AppError> {
        let result = sqlx::query(
            "DELETE FROM post_revisions WHERE post_id = $1 AND author_id = $2 AND is_autosave = true",
        )
        .bind(post_id)
        .bind(author_id)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("autosave delete failed: {err}")))?;
        Ok(result.rows_affected())
    }

    /// Counts revisions for a post.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_for_post(&self, post_id: i64) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM post_revisions WHERE post_id = $1")
            .bind(post_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("revision count failed: {err}")))
    }
}

/// Parameters for inserting a revision.
pub struct NewRevision<'a> {
    /// Snowflake id.
    pub id: i64,
    /// Post id.
    pub post_id: i64,
    /// Snapshot title.
    pub title: &'a str,
    /// Snapshot content (BlockDocument json).
    pub content: serde_json::Value,
    /// Author of the snapshot.
    pub author_id: i64,
    /// Whether this is an autosave.
    pub is_autosave: bool,
    /// Snapshot section tree, when the entry composes itself.
    pub layout: Option<serde_json::Value>,
    /// Snapshot field values (`meta.fields`); `None` records none.
    pub fields: Option<serde_json::Value>,
}
