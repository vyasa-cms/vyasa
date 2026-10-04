//! Comments repository with threading support.

use sqlx::PgPool;
use vyasa_common::AppError;

use crate::content_models::{CommentRow, CommentStatus};

const COMMENT_COLUMNS: &str =
    "id, post_id, author_user_id, author_name, author_email, content, parent_id, status, created_at";

/// Data access for the `comments` table.
#[derive(Clone, Debug)]
pub struct CommentsRepo {
    pool: PgPool,
}

impl CommentsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a comment.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn insert(&self, comment: &NewComment<'_>) -> Result<CommentRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO comments (id, post_id, author_user_id, author_name, author_email, content, parent_id, status)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING {COMMENT_COLUMNS}"
        )))
        .bind(comment.id)
        .bind(comment.post_id)
        .bind(comment.author_user_id)
        .bind(comment.author_name)
        .bind(comment.author_email)
        .bind(comment.content)
        .bind(comment.parent_id)
        .bind(comment.status.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("comment insert failed: {err}")))
    }

    /// Fetches a comment by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<CommentRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COMMENT_COLUMNS} FROM comments WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("comment lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("comment", id))
    }

    /// Returns the depth of a comment (0 for top-level). Walks parent chain.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn depth_of(&self, comment_id: i64) -> Result<usize, AppError> {
        let mut depth = 0;
        let mut current = Some(comment_id);
        while let Some(id) = current {
            let parent: Option<i64> =
                sqlx::query_scalar("SELECT parent_id FROM comments WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|err| AppError::db(format!("depth query failed: {err}")))?
                    .flatten();
            if let Some(pid) = parent {
                depth += 1;
                if depth > 16 {
                    break; // safety cap
                }
                current = Some(pid);
            } else {
                break;
            }
        }
        Ok(depth)
    }

    /// Resolves the effective parent for a new comment, applying depth cap 8.
    /// If the requested parent would exceed depth 8, walks up to depth 7 ancestor.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn resolve_parent(
        &self,
        requested_parent: Option<i64>,
    ) -> Result<Option<i64>, AppError> {
        let Some(pid) = requested_parent else {
            return Ok(None);
        };
        let depth = self.depth_of(pid).await?;
        if depth < 7 {
            return Ok(Some(pid));
        }
        // Depth would be 8 or more; find ancestor at depth 7.
        let mut current = Some(pid);
        let mut depth = depth;
        while depth >= 7 {
            if let Some(id) = current {
                let parent: Option<i64> =
                    sqlx::query_scalar("SELECT parent_id FROM comments WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&self.pool)
                        .await
                        .map_err(|err| AppError::db(format!("parent walk failed: {err}")))?
                        .flatten();
                current = parent;
                if current.is_none() {
                    break;
                }
                depth -= 1;
            } else {
                break;
            }
        }
        Ok(current)
    }

    /// Lists approved comments for a post in threaded order (depth-first).
    /// Uses a recursive CTE to order by path.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_approved_threaded(&self, post_id: i64) -> Result<Vec<CommentRow>, AppError> {
        // CTE builds path as array of ids for ordering; depth is computed.
        // Qualify all columns in the first branch to avoid ambiguity.
        let rows = sqlx::query_as::<_, CommentRow>(
            "WITH RECURSIVE thread AS (
                SELECT c.id, c.post_id, c.author_user_id, c.author_name, c.author_email, c.content, c.parent_id, c.status, c.created_at,
                       ARRAY[c.id] AS path, 0 AS depth
                FROM comments c WHERE c.post_id = $1 AND c.parent_id IS NULL AND c.status = 'approved'
                UNION ALL
                SELECT c.id, c.post_id, c.author_user_id, c.author_name, c.author_email, c.content, c.parent_id, c.status, c.created_at,
                       thread.path || c.id, thread.depth + 1
                FROM comments c JOIN thread ON c.parent_id = thread.id
                WHERE c.post_id = $1 AND c.status = 'approved' AND thread.depth < 8
             )
             SELECT id, post_id, author_user_id, author_name, author_email, content, parent_id, status, created_at
             FROM thread ORDER BY path",
        )
        .bind(post_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("threaded list failed: {err}")))?;
        Ok(rows)
    }

    /// Lists comments by status, paginated.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_by_status(
        &self,
        status: CommentStatus,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<CommentRow>, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {COMMENT_COLUMNS} FROM comments WHERE status = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        )))
        .bind(status.as_str())
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("list by status failed: {err}")))
    }

    /// Counts comments by status.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_by_status(&self, status: CommentStatus) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM comments WHERE status = $1")
            .bind(status.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("count by status failed: {err}")))
    }

    /// Updates a comment's status.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn update_status(
        &self,
        id: i64,
        status: CommentStatus,
    ) -> Result<CommentRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE comments SET status = $2 WHERE id = $1 RETURNING {COMMENT_COLUMNS}"
        )))
        .bind(id)
        .bind(status.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("comment status update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("comment", id))
    }

    /// Counts whether an email has any approved comment.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn has_approved_for_email(&self, email: &str) -> Result<bool, AppError> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM comments WHERE author_email = $1 AND status = 'approved' LIMIT 1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("approved check failed: {err}")))?;
        Ok(found.is_some())
    }

    /// Counts links in content (simple heuristic).
    #[must_use]
    pub fn count_links(content: &str) -> usize {
        content.matches("http://").count() + content.matches("https://").count()
    }
}

/// Parameters for inserting a comment.
pub struct NewComment<'a> {
    /// Snowflake id.
    pub id: i64,
    /// Post id.
    pub post_id: i64,
    /// Author user id, if logged in.
    pub author_user_id: Option<i64>,
    /// Display name.
    pub author_name: &'a str,
    /// Email.
    pub author_email: &'a str,
    /// Body.
    pub content: &'a str,
    /// Effective parent (already depth-capped).
    pub parent_id: Option<i64>,
    /// Initial status.
    pub status: CommentStatus,
}

impl CommentsRepo {
    /// Whether any comment carries this importer key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn has_import_key(&self, key: &str) -> Result<bool, AppError> {
        Ok(
            sqlx::query_scalar::<_, i32>("SELECT 1 FROM comments WHERE import_key = $1 LIMIT 1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await
                .map_err(|err| AppError::db(format!("comment import lookup failed: {err}")))?
                .is_some(),
        )
    }

    /// Inserts an imported comment as approved, recording its import key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn insert_imported(
        &self,
        post_id: i64,
        parent_id: Option<i64>,
        author_name: &str,
        author_email: &str,
        content_html: &str,
        import_key: &str,
    ) -> Result<i64, AppError> {
        let id = vyasa_common::next_id_i64();
        sqlx::query(
            "INSERT INTO comments (id, post_id, parent_id, author_name, author_email,
                                   content, status, import_key)
             VALUES ($1, $2, $3, $4, $5, $6, 'approved', $7)",
        )
        .bind(id)
        .bind(post_id)
        .bind(parent_id)
        .bind(author_name)
        .bind(author_email)
        .bind(content_html)
        .bind(import_key)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("comment import failed: {err}")))?;
        Ok(id)
    }
}
