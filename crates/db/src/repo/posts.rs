//! Posts repository.

use sqlx::PgPool;
use vyasa_common::AppError;

use crate::content_models::{PostRow, PostStatus, PostType};

const POST_COLUMNS: &str = "id, type AS post_type, status, slug, title, content, layout, excerpt,
                            author_id, parent_id, meta, published_at, scheduled_for,
                            password_hash, created_at, updated_at, sticky, lang, translation_group";

/// Filter + pagination for post listing.
#[derive(Clone, Debug, Default)]
pub struct PostFilter {
    /// Filter by status.
    pub status: Option<PostStatus>,
    /// Row order; [`PostSort::Newest`] is what every existing caller got.
    pub sort: PostSort,
    /// Filter by type.
    pub post_type: Option<PostType>,
    /// Filter by author.
    pub author_id: Option<i64>,
    /// Filter by term (any taxonomy).
    pub term_id: Option<i64>,
    /// Full-text-ish title/substring match.
    pub search: Option<String>,
    /// Restrict to entries published in `(year, month)`.
    pub published_month: Option<(i32, u32)>,
    /// Page size (default 20, max 100 enforced by callers).
    pub limit: u32,
    /// Row offset.
    pub offset: u32,
    /// Pinned posts first (the home listing); other sorts unchanged.
    pub sticky_first: bool,
    /// Restrict to `post`, `page` and these custom types: what a reader who
    /// does not edit content may see (custom types served publicly).
    /// `None` leaves every type in.
    pub readable_types: Option<Vec<String>>,
}

/// What a bound section lists: published entries of one type, filtered
/// and ordered by custom field values (`meta.fields`) as well as the
/// usual ways.
#[derive(Debug, Clone)]
pub struct BoundQuery<'a> {
    /// The type listed.
    pub post_type: PostType,
    /// Only entries carrying this term.
    pub term_id: Option<i64>,
    /// Order when `field_sort` is not set.
    pub sort: PostSort,
    /// Order by this field's value (`true`: highest first); entries
    /// without a value come last.
    pub field_sort: Option<(&'a str, bool)>,
    /// Field key and value each entry's field must equal (or, holding
    /// several values, include).
    pub filters: &'a [(String, serde_json::Value)],
    /// At most this many.
    pub limit: u32,
}

/// How a listing orders its rows.
///
/// A closed set compiled into the query — never request text — so the
/// ORDER BY can be interpolated without carrying author input into SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PostSort {
    /// Most recently created first (the historical order).
    #[default]
    Newest,
    /// Oldest first.
    Oldest,
    /// Alphabetical by title.
    Title,
    /// Most recently updated first.
    Updated,
}

impl PostSort {
    /// The ORDER BY fragment for this sort. `p.id` breaks every tie so
    /// pagination never sees the same row twice.
    #[must_use]
    pub const fn order_sql(self) -> &'static str {
        match self {
            Self::Newest => "p.created_at DESC, p.id DESC",
            Self::Oldest => "p.created_at ASC, p.id ASC",
            Self::Title => "p.title ASC, p.id ASC",
            Self::Updated => "p.updated_at DESC, p.id DESC",
        }
    }
}

/// Data access for the `posts` table.
#[derive(Clone, Debug)]
pub struct PostsRepo {
    pool: PgPool,
}

impl PostsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns the underlying pool.
    #[must_use]
    pub fn pool(&self) -> PgPool {
        self.pool.clone()
    }

    /// Inserts a post row.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Conflict`] when the slug is taken for the type
    /// (the partial unique index decides, so a racing insert is caught
    /// too), [`AppError::Db`] on other database failures.
    pub async fn insert(&self, post: &NewPost) -> Result<PostRow, AppError> {
        self.insert_with_terms(post, None).await
    }

    /// Inserts a post row and, when `term_ids` is given, its term
    /// assignments, in one transaction: either both land or neither does.
    ///
    /// # Errors
    ///
    /// As [`Self::insert`]; an unknown term id is a database error (the
    /// foreign key refuses it) and rolls the post back with it.
    pub async fn insert_with_terms(
        &self,
        post: &NewPost,
        term_ids: Option<&[i64]>,
    ) -> Result<PostRow, AppError> {
        let mut tx = self.begin().await?;
        let row = insert_in(&mut tx, post, &post.slug, term_ids).await?;
        commit(tx).await?;
        Ok(row)
    }

    /// Inserts a post under the first free slug derived from `post.slug`
    /// (the base, suffixed `-2`, `-3`, ... as [`vyasa_common::unique_slug`]
    /// does), plus its term assignments, in one transaction.
    ///
    /// The probe and the insert run under a transaction-scoped advisory
    /// lock keyed on `(type, base)`, so concurrent creates from one base
    /// slug take turns instead of all probing the same free suffix.
    /// Trashed posts do not hold their slug, matching the partial unique
    /// index. The probe is one index lookup when the base is free, and one
    /// `slug = ANY(candidates)` index scan otherwise (see `free_slug_in`),
    /// not a scan of every post of the type (the planner may still pick a
    /// scan while the table is small enough for that to be cheaper). If
    /// all 999 candidates are taken, the slug is `base-<post id>`, which
    /// the snowflake id keeps unique.
    ///
    /// # Errors
    ///
    /// As [`Self::insert_with_terms`]. A Conflict is still possible when a
    /// writer outside this lock (an update renaming a slug, a create from
    /// another base) takes the chosen slug first.
    pub async fn insert_with_unique_slug(
        &self,
        post: &NewPost,
        term_ids: Option<&[i64]>,
    ) -> Result<PostRow, AppError> {
        let base = post.slug.as_str();
        let mut tx = self.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1 || ':' || $2, 0))")
            .bind(post.post_type.as_str())
            .bind(base)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("slug lock failed: {err}")))?;
        let slug = free_slug_in(&mut tx, post).await?;
        let row = insert_in(&mut tx, post, &slug, term_ids).await?;
        commit(tx).await?;
        Ok(row)
    }

    async fn begin(&self) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, AppError> {
        self.pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))
    }

    /// Fetches a post by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<PostRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {POST_COLUMNS} FROM posts WHERE id = $1"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("post lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("post", id))
    }

    /// Fetches a post by type + slug (trash excluded).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get_by_slug(&self, post_type: PostType, slug: &str) -> Result<PostRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {POST_COLUMNS} FROM posts
             WHERE type = $1 AND slug = $2 AND status <> 'trash'"
        )))
        .bind(post_type.as_str())
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("post lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("post", slug))
    }

    /// Whether a slug is taken for `post_type` (trash excluded).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn slug_taken(
        &self,
        post_type: PostType,
        slug: &str,
        exclude_id: Option<i64>,
    ) -> Result<bool, AppError> {
        let taken: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM posts
             WHERE type = $1 AND slug = $2 AND status <> 'trash'
               AND ($3::bigint IS NULL OR id <> $3)
             LIMIT 1",
        )
        .bind(post_type.as_str())
        .bind(slug)
        .bind(exclude_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("slug check failed: {err}")))?;
        Ok(taken.is_some())
    }

    /// Published-entry counts per post type, for the content-types listing.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_by_type(&self) -> Result<Vec<(String, i64)>, AppError> {
        sqlx::query_as(
            "SELECT type, COUNT(*) FROM posts WHERE status = 'published' \
             GROUP BY type ORDER BY type",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("type counts failed: {err}")))
    }

    /// Lists posts matching a filter, in the filter's order.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self, filter: &PostFilter) -> Result<Vec<PostRow>, AppError> {
        self.list_visible(filter, true, None).await
    }

    /// Lists matching posts, restricted to unprotected published rows and
    /// the supplied author unless `all` is true. Visibility precedes pagination.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_visible(
        &self,
        filter: &PostFilter,
        all: bool,
        author: Option<i64>,
    ) -> Result<Vec<PostRow>, AppError> {
        let order = filter.sort.order_sql();
        let sticky = if filter.sticky_first {
            "p.sticky DESC, "
        } else {
            ""
        };
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {POST_COLUMNS} FROM posts p
             WHERE ($1::text IS NULL OR p.status = $1)
               AND ($2::text IS NULL OR p.type = $2)
               AND ($3::bigint IS NULL OR p.author_id = $3)
               AND ($4::bigint IS NULL OR EXISTS (
                    SELECT 1 FROM term_relationships tr
                    WHERE tr.post_id = p.id AND tr.term_id = $4))
               AND ($5::text IS NULL OR p.title ILIKE '%' || $5 || '%')
               AND ($8::int IS NULL OR (
                    EXTRACT(YEAR FROM COALESCE(p.published_at, p.created_at)) = $8
                    AND EXTRACT(MONTH FROM COALESCE(p.published_at, p.created_at)) = $9))
               AND ($10 OR (p.status = 'published' AND p.password_hash IS NULL)
                    OR p.author_id = $11)
               AND ($12::text[] IS NULL OR p.type IN ('post', 'page') OR p.type = ANY($12))
             ORDER BY {sticky}{order}
             LIMIT $6 OFFSET $7"
        )))
        .bind(filter.status.map(PostStatus::as_str))
        .bind(filter.post_type.map(PostType::as_str))
        .bind(filter.author_id)
        .bind(filter.term_id)
        .bind(filter.search.as_deref())
        .bind(i64::from(filter.limit))
        .bind(i64::from(filter.offset))
        .bind(filter.published_month.map(|(y, _)| y))
        .bind(
            filter
                .published_month
                .map(|(_, m)| i32::try_from(m).unwrap_or(1)),
        )
        .bind(all)
        .bind(author)
        .bind(filter.readable_types.as_deref())
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("post list failed: {err}")))
    }

    /// Replaces an entry's field values (`meta.fields`) with `values`,
    /// already validated by the caller; `{}` removes them. Leaves
    /// `updated_at` alone: an import writing the values it just inserted
    /// the entry without is not an edit.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_field_values(
        &self,
        id: i64,
        values: &serde_json::Value,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE posts SET meta = CASE
                 WHEN $2::jsonb = '{}'::jsonb THEN COALESCE(meta, '{}'::jsonb) - 'fields'
                 ELSE jsonb_set(COALESCE(meta, '{}'::jsonb), '{fields}', $2::jsonb)
             END
             WHERE id = $1",
        )
        .bind(id)
        .bind(values)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("field values update failed: {err}")))?;
        Ok(())
    }

    /// The entries among `ids` that exist, in any status (one query, for
    /// resolving many references at once).
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn get_many(&self, ids: &[i64]) -> Result<Vec<PostRow>, AppError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {POST_COLUMNS} FROM posts WHERE id = ANY($1)"
        )))
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("post lookup failed: {err}")))
    }

    /// Published entries for a bound section ([`BoundQuery`]).
    ///
    /// Password-protected entries are left out: a binding's field filter
    /// must not become an oracle on what a protected entry holds.
    ///
    /// Field keys and values are bound parameters, never SQL text; the
    /// only interpolated pieces are fixed ORDER BY fragments.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_bound(&self, q: &BoundQuery<'_>) -> Result<Vec<PostRow>, AppError> {
        let conditions: Vec<serde_json::Value> = q
            .filters
            .iter()
            .map(|(k, v)| serde_json::json!([k, v]))
            .collect();
        let order = match q.field_sort {
            Some((_, true)) => "(p.meta->'fields'->$5) DESC NULLS LAST, p.id DESC",
            Some((_, false)) => "(p.meta->'fields'->$5) ASC NULLS LAST, p.id ASC",
            None => q.sort.order_sql(),
        };
        let sql = format!(
            "SELECT {POST_COLUMNS} FROM posts p
             WHERE p.status = 'published' AND p.password_hash IS NULL AND p.type = $1
               AND ($2::bigint IS NULL OR EXISTS (
                    SELECT 1 FROM term_relationships tr
                    WHERE tr.post_id = p.id AND tr.term_id = $2))
               AND NOT EXISTS (
                    SELECT 1 FROM jsonb_array_elements($3::jsonb) c
                    WHERE NOT COALESCE((p.meta->'fields'->(c->>0)) @> (c->1), false))
             ORDER BY {order}
             LIMIT $4"
        );
        let mut query = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(q.post_type.as_str())
            .bind(q.term_id)
            .bind(serde_json::Value::Array(conditions))
            .bind(i64::from(q.limit));
        if let Some((key, _)) = q.field_sort {
            query = query.bind(key);
        }
        query
            .fetch_all(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("bound list failed: {err}")))
    }

    /// Counts posts matching a filter (same criteria, ignoring paging).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count(&self, filter: &PostFilter) -> Result<i64, AppError> {
        self.count_visible(filter, true, None).await
    }

    /// Counts the same visible rows as [`Self::list_visible`], before pagination.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_visible(
        &self,
        filter: &PostFilter,
        all: bool,
        author: Option<i64>,
    ) -> Result<i64, AppError> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM posts p
             WHERE ($1::text IS NULL OR p.status = $1)
               AND ($2::text IS NULL OR p.type = $2)
               AND ($3::bigint IS NULL OR p.author_id = $3)
               AND ($4::bigint IS NULL OR EXISTS (
                    SELECT 1 FROM term_relationships tr
                    WHERE tr.post_id = p.id AND tr.term_id = $4))
               AND ($5::text IS NULL OR p.title ILIKE '%' || $5 || '%')
               AND ($6 OR (p.status = 'published' AND p.password_hash IS NULL)
                    OR p.author_id = $7)
               AND ($8::int IS NULL OR (
                    EXTRACT(YEAR FROM COALESCE(p.published_at, p.created_at)) = $8
                    AND EXTRACT(MONTH FROM COALESCE(p.published_at, p.created_at)) = $9))
               AND ($10::text[] IS NULL OR p.type IN ('post', 'page') OR p.type = ANY($10))",
        )
        .bind(filter.status.map(PostStatus::as_str))
        .bind(filter.post_type.map(PostType::as_str))
        .bind(filter.author_id)
        .bind(filter.term_id)
        .bind(filter.search.as_deref())
        .bind(all)
        .bind(author)
        .bind(filter.published_month.map(|(y, _)| y))
        .bind(
            filter
                .published_month
                .map(|(_, m)| i32::try_from(m).unwrap_or(1)),
        )
        .bind(filter.readable_types.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("post count failed: {err}")))
    }

    /// Lists due scheduled posts (for the publisher worker).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_scheduled_due(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<Vec<PostRow>, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {POST_COLUMNS} FROM posts
             WHERE status = 'scheduled' AND scheduled_for <= $1
             ORDER BY scheduled_for ASC
             LIMIT $2"
        )))
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("scheduled due list failed: {err}")))
    }

    /// Publishes due scheduled posts and returns the rows it published.
    ///
    /// One statement claims and flips them: the inner select locks due
    /// rows with `SKIP LOCKED`, so concurrent publishers split the batch
    /// instead of sharing it, and the outer `status = 'scheduled'` guard
    /// re-checks each row after the lock -- a post someone moved back to
    /// draft a moment earlier is left alone. Each post comes back from
    /// exactly one call, so the caller announces it exactly once.
    ///
    /// `updated_at` is not touched: going live on schedule is not an edit.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn publish_scheduled_due(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Result<Vec<PostRow>, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE posts SET
                status = 'published',
                published_at = COALESCE(published_at, now())
             WHERE id IN (
                 SELECT id FROM posts
                 WHERE status = 'scheduled' AND scheduled_for <= $1
                 ORDER BY scheduled_for ASC
                 LIMIT $2
                 FOR UPDATE SKIP LOCKED)
               AND status = 'scheduled'
             RETURNING {POST_COLUMNS}"
        )))
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("scheduled publish failed: {err}")))
    }

    /// Sets the password hash for a post (private posts).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_password_hash(
        &self,
        id: i64,
        password_hash: Option<&str>,
    ) -> Result<(), AppError> {
        sqlx::query("UPDATE posts SET password_hash = $1 WHERE id = $2")
            .bind(password_hash)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("password update failed: {err}")))?;
        Ok(())
    }

    /// Updates the mutable fields of a post, and its password and terms
    /// when asked, in one transaction.
    ///
    /// With [`PostUpdate::expect_status`] set, the row is only written
    /// while it still has that status; a post that changed underneath the
    /// caller is a conflict rather than a silent overwrite of the newer
    /// state (the scheduled publisher racing an editor, say).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing, [`AppError::Conflict`]
    /// for a taken slug or a status that moved, [`AppError::Db`] otherwise.
    pub async fn update(&self, post: &PostUpdate) -> Result<PostRow, AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))?;
        let row: Option<PostRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE posts SET
                status = COALESCE($2, status),
                slug = COALESCE($3, slug),
                title = COALESCE($4, title),
                content = COALESCE($5, content),
                excerpt = COALESCE($6, excerpt),
                parent_id = COALESCE($7, parent_id),
                meta = {META_WITH_FIELDS},
                scheduled_for = CASE WHEN $13 THEN NULL ELSE COALESCE($9, scheduled_for) END,
                password_hash = CASE WHEN $14 THEN $10 ELSE password_hash END,
                layout = COALESCE($12, layout),
                published_at = CASE
                    WHEN $2 = 'published' AND published_at IS NULL THEN now()
                    ELSE published_at END,
                updated_at = CASE WHEN $11 THEN now() ELSE updated_at END
             WHERE id = $1 AND ($15::text IS NULL OR status = $15)
             RETURNING {POST_COLUMNS}"
        )))
        .bind(post.id)
        .bind(post.status.map(PostStatus::as_str))
        .bind(&post.slug)
        .bind(&post.title)
        .bind(&post.content)
        .bind(&post.excerpt)
        .bind(post.parent_id)
        .bind(&post.meta)
        .bind(post.scheduled_for)
        .bind(post.password_hash.as_ref().and_then(Option::as_deref))
        .bind(post.touch_updated_at)
        .bind(&post.layout)
        .bind(post.clear_schedule)
        .bind(post.password_hash.is_some())
        .bind(post.expect_status.map(PostStatus::as_str))
        .bind(&post.fields)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| {
            write_err(
                &err,
                "post update failed",
                post.slug.as_deref().unwrap_or("(current)"),
            )
        })?;
        let Some(row) = row else {
            let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM posts WHERE id = $1")
                .bind(post.id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("post lookup failed: {err}")))?;
            return Err(if exists.is_some() {
                AppError::conflict("the post changed while saving; reload and try again")
            } else {
                AppError::not_found("post", post.id)
            });
        };
        if let Some(ids) = &post.term_ids {
            crate::repo::terms::replace_post_terms(&mut tx, post.id, ids).await?;
        }
        tx.commit()
            .await
            .map_err(|err| AppError::db(format!("tx commit failed: {err}")))?;
        Ok(row)
    }

    /// Deletes a post permanently.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn hard_delete(&self, id: i64) -> Result<(), AppError> {
        sqlx::query("DELETE FROM posts WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("post delete failed: {err}")))?;
        Ok(())
    }
}

/// Picks the first free slug for `post` (base, `base-2` ... `base-999`,
/// then `base-<id>`), inside the caller's transaction.
///
/// Both probes match `(type, slug)` by equality so they use the partial
/// unique index `posts_type_slug_uniq` (whose predicate, `status <>
/// 'trash'`, they repeat). A prefix match (`left(slug, ...)`, `LIKE`) or a
/// string range would not: the first cannot use the index, and a range
/// is collation-dependent.
async fn free_slug_in(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    post: &NewPost,
) -> Result<String, AppError> {
    let base = post.slug.as_str();
    let base_taken: bool = sqlx::query_scalar(BASE_SLUG_TAKEN_SQL)
        .bind(post.post_type.as_str())
        .bind(base)
        .fetch_one(&mut **tx)
        .await
        .map_err(|err| AppError::db(format!("slug check failed: {err}")))?;
    if !base_taken {
        return Ok(base.to_string());
    }
    let candidates: Vec<String> = vyasa_common::slug_candidates(base).collect();
    let taken: std::collections::HashSet<String> = sqlx::query_scalar(TAKEN_SLUGS_SQL)
        .bind(post.post_type.as_str())
        .bind(&candidates)
        .fetch_all(&mut **tx)
        .await
        .map_err(|err| AppError::db(format!("slug check failed: {err}")))?
        .into_iter()
        .collect();
    Ok(candidates
        .into_iter()
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| format!("{base}-{}", post.id)))
}

/// Inserts `post` and, when given, its term assignments inside `tx`.
async fn insert_in(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    post: &NewPost,
    slug: &str,
    term_ids: Option<&[i64]>,
) -> Result<PostRow, AppError> {
    let row: PostRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO posts (id, type, status, slug, title, content, excerpt,
                            author_id, parent_id, meta, published_at, scheduled_for,
                            password_hash, layout)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         RETURNING {POST_COLUMNS}"
    )))
    .bind(post.id)
    .bind(post.post_type.as_str())
    .bind(post.status.as_str())
    .bind(slug)
    .bind(&post.title)
    .bind(&post.content)
    .bind(&post.excerpt)
    .bind(post.author_id)
    .bind(post.parent_id)
    .bind(&post.meta)
    .bind(post.published_at)
    .bind(post.scheduled_for)
    .bind(&post.password_hash)
    .bind(&post.layout)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| write_err(&err, "post insert failed", slug))?;
    if let Some(ids) = term_ids {
        crate::repo::terms::replace_post_terms(tx, row.id, ids).await?;
    }
    Ok(row)
}

async fn commit(tx: sqlx::Transaction<'static, sqlx::Postgres>) -> Result<(), AppError> {
    tx.commit()
        .await
        .map_err(|err| AppError::db(format!("tx commit failed: {err}")))
}

/// Maps a write failure: the per-type slug index is a conflict the caller
/// can act on (pick another slug, retry); anything else is a database
/// fault.
fn write_err(err: &sqlx::Error, what: &str, slug: &str) -> AppError {
    if let sqlx::Error::Database(db) = err {
        if db.is_unique_violation() && db.constraint() == Some(SLUG_INDEX) {
            return AppError::conflict(format!("slug {slug:?} is already in use"));
        }
    }
    AppError::db(format!("{what}: {err}"))
}

/// The new `meta` of [`PostsRepo::update`], with `$8` the given meta and
/// `$16` the given field values.
///
/// `meta.fields` belongs to the field values alone: with `$16` set they
/// replace the stored ones (an empty object removes the key); without it
/// the stored values are carried over whatever `$8` says, so a meta write
/// (SEO fields and the like) neither drops them nor smuggles in values
/// that never went through validation. One statement, so a concurrent
/// fields write is not undone by a meta write that read the row earlier.
const META_WITH_FIELDS: &str = "CASE
        WHEN $16::jsonb IS NOT NULL THEN
            (CASE WHEN jsonb_typeof(COALESCE($8::jsonb, meta)) = 'object'
                  THEN COALESCE($8::jsonb, meta) ELSE '{}'::jsonb END - 'fields')
            || CASE WHEN $16::jsonb = '{}'::jsonb THEN '{}'::jsonb
                    ELSE jsonb_build_object('fields', $16::jsonb) END
        WHEN $8::jsonb IS NOT NULL AND jsonb_typeof($8::jsonb) = 'object' THEN
            ($8::jsonb - 'fields')
            || CASE WHEN jsonb_typeof(meta) = 'object' AND jsonb_typeof(meta->'fields') = 'object'
                    THEN jsonb_build_object('fields', meta->'fields') ELSE '{}'::jsonb END
        ELSE COALESCE($8::jsonb, meta)
    END";

/// The partial unique index on `(type, slug)` for non-trashed posts.
pub const SLUG_INDEX: &str = "posts_type_slug_uniq";

/// Whether slug `$2` is held by a non-trashed post of type `$1`: the
/// first probe of [`PostsRepo::insert_with_unique_slug`]. Public so a test
/// can check its plan uses [`SLUG_INDEX`].
pub const BASE_SLUG_TAKEN_SQL: &str = "SELECT EXISTS(SELECT 1 FROM posts
                   WHERE type = $1 AND slug = $2 AND status <> 'trash')";

/// Which of the slugs in `$2` are held by non-trashed posts of type `$1`:
/// the second probe of [`PostsRepo::insert_with_unique_slug`]. Public so a
/// test can check its plan uses [`SLUG_INDEX`].
pub const TAKEN_SLUGS_SQL: &str = "SELECT slug FROM posts
     WHERE type = $1 AND slug = ANY($2) AND status <> 'trash'";

/// Parameters for inserting a post.
pub struct NewPost {
    /// Snowflake id.
    pub id: i64,
    /// Content type.
    pub post_type: PostType,
    /// Initial status.
    pub status: PostStatus,
    /// Slug (caller ensures uniqueness; the base to suffix for
    /// [`PostsRepo::insert_with_unique_slug`]).
    pub slug: String,
    /// Title.
    pub title: String,
    /// Block document JSON.
    pub content: serde_json::Value,
    /// Optional excerpt.
    pub excerpt: Option<String>,
    /// Author.
    pub author_id: i64,
    /// Optional parent.
    pub parent_id: Option<i64>,
    /// Extension metadata.
    pub meta: serde_json::Value,
    /// Set when inserting directly as published.
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Set when inserting as scheduled.
    pub scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional access password hash.
    pub password_hash: Option<String>,
    /// Section tree composing this entry, when it has one.
    pub layout: Option<serde_json::Value>,
}

/// Partial update for a post; `None` fields keep their current values.
pub struct PostUpdate {
    /// Post id.
    pub id: i64,
    /// New status.
    pub status: Option<PostStatus>,
    /// New slug.
    pub slug: Option<String>,
    /// New title.
    pub title: Option<String>,
    /// New content.
    pub content: Option<serde_json::Value>,
    /// New excerpt.
    pub excerpt: Option<String>,
    /// New parent.
    pub parent_id: Option<i64>,
    /// New meta (a JSON object). Its own `fields` key is ignored: the
    /// stored field values are kept unless [`PostUpdate::fields`] is set.
    pub meta: Option<serde_json::Value>,
    /// New field values (`meta.fields`, a JSON object of already
    /// validated values) replacing the stored ones; an empty object
    /// removes them, `None` keeps them. Applied in the same statement as
    /// `meta`, so a meta write can never drop values it did not mean to.
    pub fields: Option<serde_json::Value>,
    /// Remove the schedule outright.
    ///
    /// `scheduled_for: None` means "leave it", as every other field's
    /// `None` does, so there was no way to say "no schedule": a post moved
    /// back to draft kept its old date, and on the next tick or the next
    /// re-schedule that stale date published it.
    pub clear_schedule: bool,
    /// New scheduled time.
    pub scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
    /// New password hash: `None` keeps, `Some(None)` clears,
    /// `Some(Some(hash))` sets. Written in the same transaction as the
    /// other fields.
    pub password_hash: Option<Option<String>>,
    /// Replacement term ids (`None` keeps). Written in the same
    /// transaction; callers validate the ids first for a clean error.
    pub term_ids: Option<Vec<i64>>,
    /// Only write while the row still has this status.
    pub expect_status: Option<PostStatus>,
    /// Whether updated_at should be bumped (content actually changed).
    pub touch_updated_at: bool,
    /// New section tree. An empty array clears the composition, which is
    /// why this is not modelled as "`None` means clear" like the others —
    /// `COALESCE` cannot express both "leave alone" and "set to null".
    pub layout: Option<serde_json::Value>,
}

impl PostsRepo {
    /// Published-post counts per calendar month, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn month_counts(&self, months: u32) -> Result<Vec<(i16, u8, i64)>, AppError> {
        sqlx::query_as::<_, (sqlx::types::chrono::NaiveDate, i64)>(
            "SELECT date_trunc('month', COALESCE(p.published_at, p.created_at))::date, COUNT(*)::bigint
             FROM posts p WHERE p.status = 'published'
             GROUP BY 1 ORDER BY 1 DESC LIMIT $1",
        )
        .bind(i64::from(months))
        .fetch_all(&self.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|(d, c)| {
                    use chrono::Datelike as _;
                    (
                        i16::try_from(d.year()).unwrap_or(i16::MAX),
                        u8::try_from(d.month()).unwrap_or(12),
                        c,
                    )
                })
                .collect()
        })
        .map_err(|err| AppError::db(format!("month counts failed: {err}")))
    }
}

impl PostsRepo {
    /// Finds a post id whose meta records the given importer key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn find_id_by_import_key(&self, key: &str) -> Result<Option<i64>, AppError> {
        sqlx::query_scalar::<_, i64>("SELECT id FROM posts WHERE meta ? $1 LIMIT 1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("post import lookup failed: {err}")))
    }

    /// Sets the language and, optionally, joins a translation group.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_language(
        &self,
        id: i64,
        lang: &str,
        translation_group: Option<i64>,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE posts SET lang = $2, translation_group = COALESCE($3, translation_group) WHERE id = $1",
        )
        .bind(id)
        .bind(lang)
        .bind(translation_group)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("language update failed: {err}")))?;
        Ok(())
    }

    /// Every member of a translation group, in any status, by language.
    /// Callers decide which of them their reader may see.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn translations(&self, group: i64) -> Result<Vec<PostRow>, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {POST_COLUMNS} FROM posts WHERE translation_group = $1 ORDER BY lang, id"
        )))
        .bind(group)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("translations failed: {err}")))
    }

    /// Pins or unpins a post on the home listing.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_sticky(&self, id: i64, sticky: bool) -> Result<(), AppError> {
        sqlx::query("UPDATE posts SET sticky = $2 WHERE id = $1")
            .bind(id)
            .bind(sticky)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("sticky update failed: {err}")))?;
        Ok(())
    }
}
