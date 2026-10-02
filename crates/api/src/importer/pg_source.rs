//! Postgres-backed WordPress source (schema imported via e.g. pgloader).
//!
//! Opens its own connection pool with a read-only session; nothing in this
//! module ever writes to the source.

use vyasa_common::AppError;

use super::wp::{WpComment, WpPost, WpSource, WpTerm, WpUser};

/// A read-only pool over the WP schema.
#[derive(Clone)]
pub struct PgWpSource {
    pool: sqlx::PgPool,
}

impl std::fmt::Debug for PgWpSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PgWpSource")
    }
}

impl PgWpSource {
    /// Connects read-only.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] when the connection fails.
    pub async fn connect(url: &str) -> Result<Self, AppError> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query("SET default_transaction_read_only = on")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .map_err(|e| AppError::db(format!("wp source connect: {e}")))?;
        Ok(Self { pool })
    }

    async fn scalar_rows<T>(&self, sql: &str) -> Result<Vec<T>, AppError>
    where
        T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
    {
        sqlx::query_as::<_, T>(sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("wp source query failed: {e}")))
    }
}

struct WpUserRow {
    id: i64,
    login: String,
    email: String,
    display_name: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for WpUserRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        let raw_id: i64 = row.try_get("ID")?;
        let id = i64::try_from(raw_id.unsigned_abs()).unwrap_or(raw_id);
        Ok(Self {
            id,
            login: row.try_get("user_login")?,
            email: row.try_get("user_email")?,
            display_name: row.try_get("display_name")?,
        })
    }
}

struct WpTermRow {
    term_id: i64,
    taxonomy: String,
    name: String,
    slug: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for WpTermRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            term_id: row.try_get("term_id")?,
            taxonomy: row.try_get("taxonomy")?,
            name: row.try_get("name")?,
            slug: row.try_get("slug")?,
        })
    }
}

struct WpPostRaw {
    id: i64,
    post_type: String,
    post_status: String,
    post_name: String,
    post_title: String,
    post_content: String,
    post_excerpt: String,
    post_author: i64,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for WpPostRaw {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            id: row.try_get("ID")?,
            post_type: row.try_get("post_type")?,
            post_status: row.try_get("post_status")?,
            post_name: row
                .try_get::<Option<String>, _>("post_name")?
                .unwrap_or_default(),
            post_title: row
                .try_get::<Option<String>, _>("post_title")?
                .unwrap_or_default(),
            post_content: row.try_get("post_content")?,
            post_excerpt: row
                .try_get::<Option<String>, _>("post_excerpt")?
                .unwrap_or_default(),
            post_author: row.try_get("post_author")?,
        })
    }
}

struct WpCommentRaw {
    comment_id: i64,
    comment_post_id: i64,
    comment_parent: i64,
    comment_author: String,
    comment_author_email: String,
    comment_content: String,
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for WpCommentRaw {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            comment_id: row.try_get("comment_ID")?,
            comment_post_id: row.try_get("comment_post_ID")?,
            comment_parent: row.try_get("comment_parent")?,
            comment_author: row.try_get("comment_author")?,
            comment_author_email: row
                .try_get::<Option<String>, _>("comment_author_email")?
                .unwrap_or_default(),
            comment_content: row.try_get("comment_content")?,
        })
    }
}

impl WpSource for PgWpSource {
    async fn users(&self) -> Result<Vec<WpUser>, AppError> {
        let rows = self
            .scalar_rows::<WpUserRow>(
                "SELECT ID, user_login, user_email, display_name FROM wp_users ORDER BY ID",
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| WpUser {
                id: r.id,
                login: r.login,
                email: r.email,
                display_name: r.display_name,
            })
            .collect())
    }

    async fn posts(&self) -> Result<Vec<WpPost>, AppError> {
        let rows = self
            .scalar_rows::<WpPostRaw>(
                "SELECT ID, post_type, post_status, post_name, post_title, post_content, \
                        post_excerpt, post_author
                 FROM wp_posts
                 WHERE post_type IN ('post', 'page')
                   AND post_status NOT IN ('trash', 'auto-draft', 'inherit')
                 ORDER BY ID",
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| WpPost {
                id: r.id,
                post_type: r.post_type,
                status: r.post_status,
                slug: if r.post_name.is_empty() {
                    format!("wp-{}", r.id)
                } else {
                    r.post_name
                },
                title: if r.post_title.is_empty() {
                    format!("Untitled {}", r.id)
                } else {
                    r.post_title
                },
                content_html: r.post_content,
                excerpt: r.post_excerpt,
                author_id: r.post_author,
                meta: Vec::new(),
                term_ids: Vec::new(),
            })
            .collect())
    }

    async fn terms(&self) -> Result<Vec<WpTerm>, AppError> {
        let rows = self
            .scalar_rows::<WpTermRow>(
                "SELECT t.term_id, tt.taxonomy, t.name, t.slug
                 FROM wp_terms t JOIN wp_term_taxonomy tt ON tt.term_id = t.term_id
                 WHERE tt.taxonomy IN ('category', 'post_tag')
                 ORDER BY t.term_id",
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| WpTerm {
                id: r.term_id,
                taxonomy: r.taxonomy,
                name: r.name,
                slug: r.slug,
            })
            .collect())
    }

    async fn comments(&self) -> Result<Vec<WpComment>, AppError> {
        let rows = self
            .scalar_rows::<WpCommentRaw>(
                "SELECT comment_ID, comment_post_ID, comment_parent, comment_author, \
                        comment_author_email, comment_content
                 FROM wp_comments
                 WHERE comment_approved NOT IN ('spam', 'trash')
                 ORDER BY comment_ID",
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| WpComment {
                id: r.comment_id,
                post_id: r.comment_post_id,
                parent_id: (r.comment_parent > 0).then_some(r.comment_parent),
                author_name: r.comment_author,
                author_email: r.comment_author_email,
                content_html: r.comment_content,
            })
            .collect())
    }

    async fn attachments(&self) -> Result<Vec<(i64, String, String)>, AppError> {
        self.attachment_rows().await
    }
}

impl PgWpSource {
    async fn attachment_rows(&self) -> Result<Vec<(i64, String, String)>, AppError> {
        let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT p.ID, pm.meta_value, p.post_title
             FROM wp_posts p
             JOIN wp_postmeta pm ON pm.post_id = p.ID AND pm.meta_key = '_wp_attached_file'
             WHERE p.post_type = 'attachment' AND p.post_status != 'trash'",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("wp attachments query failed: {e}")))?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, path, title)| {
                let path = path?;
                let name = path.rsplit('/').next().unwrap_or("file").to_owned();
                Some((id, name, title.filter(|t| !t.is_empty()).unwrap_or(path)))
            })
            .collect())
    }
}
