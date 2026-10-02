//! Terms repository.

use sqlx::PgPool;
use vyasa_common::AppError;

use crate::content_models::{Taxonomy, TermRow};

const TERM_COLUMNS: &str = "id, taxonomy, name, slug, parent_id, meta, created_at";

/// Data access for the `terms` table.
#[derive(Clone, Debug)]
pub struct TermsRepo {
    pool: PgPool,
}

impl TermsRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a term.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure (e.g. duplicate slug).
    pub async fn insert(&self, term: &NewTerm<'_>) -> Result<TermRow, AppError> {
        sqlx::query_as(&format!(
            "INSERT INTO terms (id, taxonomy, name, slug, parent_id, meta)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING {TERM_COLUMNS}"
        ))
        .bind(term.id)
        .bind(term.taxonomy.as_str())
        .bind(term.name)
        .bind(term.slug)
        .bind(term.parent_id)
        .bind(&term.meta)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("term insert failed: {err}")))
    }

    /// Fetches a term by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<TermRow, AppError> {
        sqlx::query_as(&format!("SELECT {TERM_COLUMNS} FROM terms WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("term lookup failed: {err}")))?
            .ok_or_else(|| AppError::not_found("term", id))
    }

    /// Fetches a term by taxonomy + slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get_by_slug(&self, taxonomy: Taxonomy, slug: &str) -> Result<TermRow, AppError> {
        sqlx::query_as(&format!(
            "SELECT {TERM_COLUMNS} FROM terms WHERE taxonomy = $1 AND slug = $2"
        ))
        .bind(taxonomy.as_str())
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("term lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("term", slug))
    }

    /// Lists terms, optionally filtered by taxonomy, with optional post counts.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(
        &self,
        taxonomy: Option<Taxonomy>,
        with_counts: bool,
    ) -> Result<Vec<TermWithCount>, AppError> {
        if with_counts {
            let rows = sqlx::query_as::<_, TermCountRow>(
                "SELECT t.id, t.taxonomy, t.name, t.slug, t.parent_id, t.meta, t.created_at,
                        COUNT(tr.post_id)::bigint AS post_count
                 FROM terms t
                 LEFT JOIN term_relationships tr ON tr.term_id = t.id
                 WHERE ($1::text IS NULL OR t.taxonomy = $1)
                 GROUP BY t.id
                 ORDER BY t.name ASC",
            )
            .bind(taxonomy.map(Taxonomy::as_str))
            .fetch_all(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("term list failed: {err}")))?;
            Ok(rows.into_iter().map(TermWithCount::from).collect())
        } else {
            let terms = sqlx::query_as(&format!(
                "SELECT {TERM_COLUMNS} FROM terms
                 WHERE ($1::text IS NULL OR taxonomy = $1)
                 ORDER BY name ASC"
            ))
            .bind(taxonomy.map(Taxonomy::as_str))
            .fetch_all(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("term list failed: {err}")))?;
            Ok(terms
                .into_iter()
                .map(|term| TermWithCount {
                    term,
                    post_count: 0,
                })
                .collect())
        }
    }

    /// Updates a term's mutable fields (excluding parent, which has cycle checks).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn update(&self, id: i64, update: &TermUpdate<'_>) -> Result<TermRow, AppError> {
        sqlx::query_as(&format!(
            "UPDATE terms SET
                name = COALESCE($2, name),
                slug = COALESCE($3, slug),
                meta = COALESCE($4, meta)
             WHERE id = $1
             RETURNING {TERM_COLUMNS}"
        ))
        .bind(id)
        .bind(update.name)
        .bind(update.slug)
        .bind(update.meta)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("term update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("term", id))
    }

    /// Sets a term's parent (used for cycle checks; separate from generic update).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_parent(&self, id: i64, parent_id: Option<i64>) -> Result<TermRow, AppError> {
        sqlx::query_as(&format!(
            "UPDATE terms SET parent_id = $2 WHERE id = $1 RETURNING {TERM_COLUMNS}"
        ))
        .bind(id)
        .bind(parent_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("term parent update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("term", id))
    }

    /// Updates name/slug/meta and, when `parent` is `Some`, the parent, in
    /// one transaction: a refused slug leaves the parent where it was.
    ///
    /// A new parent is re-checked for cycles inside the transaction.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when the parent would create a
    /// cycle, [`AppError::Conflict`] for a slug taken in the taxonomy,
    /// [`AppError::NotFound`] when missing, [`AppError::Db`] otherwise.
    pub async fn update_with_parent(
        &self,
        id: i64,
        update: &TermUpdate<'_>,
        parent: Option<Option<i64>>,
    ) -> Result<TermRow, AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))?;
        if let Some(parent_id) = parent {
            if let Some(pid) = parent_id {
                sqlx::query("SELECT id FROM terms WHERE id = ANY($1) FOR UPDATE")
                    .bind([id, pid].as_slice())
                    .execute(&mut *tx)
                    .await
                    .map_err(|err| AppError::db(format!("term lock failed: {err}")))?;
                if pid == id || ancestors_of(&mut *tx, pid).await?.contains(&id) {
                    return Err(AppError::validation("term parent would create a cycle"));
                }
            }
            sqlx::query("UPDATE terms SET parent_id = $2 WHERE id = $1")
                .bind(id)
                .bind(parent_id)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("term parent update failed: {err}")))?;
        }
        let row: TermRow = sqlx::query_as(&format!(
            "UPDATE terms SET
                name = COALESCE($2, name),
                slug = COALESCE($3, slug),
                meta = COALESCE($4, meta)
             WHERE id = $1
             RETURNING {TERM_COLUMNS}"
        ))
        .bind(id)
        .bind(update.name)
        .bind(update.slug)
        .bind(update.meta)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| match &err {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                AppError::conflict("term slug is already in use")
            }
            _ => AppError::db(format!("term update failed: {err}")),
        })?
        .ok_or_else(|| AppError::not_found("term", id))?;
        tx.commit()
            .await
            .map_err(|err| AppError::db(format!("tx commit failed: {err}")))?;
        Ok(row)
    }

    /// Deletes a term, optionally reassigning its posts and children.
    ///
    /// When `reassign_to` is `Some`, all post relationships and child
    /// terms are moved to that id before deletion.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete(&self, id: i64, reassign_to: Option<i64>) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))?;
        if let Some(target) = reassign_to {
            // Children are handed to the target below. When the target
            // sits under this term, that hands the target's own ancestors
            // to it as children: a cycle. Checked inside the transaction,
            // with the chain locked, so a concurrent reparent cannot slip
            // one in between the check and the write.
            sqlx::query("SELECT id FROM terms WHERE id = ANY($1) FOR UPDATE")
                .bind([id, target].as_slice())
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("term lock failed: {err}")))?;
            if target == id || ancestors_of(&mut *tx, target).await?.contains(&id) {
                return Err(AppError::validation(
                    "cannot move a term's posts and children into itself or one of its descendants",
                ));
            }
            // Move post relationships (avoid duplicate PK errors via ON CONFLICT DO NOTHING).
            sqlx::query(
                "INSERT INTO term_relationships (post_id, term_id)
                 SELECT post_id, $1 FROM term_relationships WHERE term_id = $2
                 ON CONFLICT DO NOTHING",
            )
            .bind(target)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("reassign relationships failed: {err}")))?;
            sqlx::query("DELETE FROM term_relationships WHERE term_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("delete relationships failed: {err}")))?;
            // Reparent children.
            sqlx::query("UPDATE terms SET parent_id = $1 WHERE parent_id = $2")
                .bind(target)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("reparent children failed: {err}")))?;
        } else {
            // Just delete relationships and null out children.
            sqlx::query("DELETE FROM term_relationships WHERE term_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("delete relationships failed: {err}")))?;
            sqlx::query("UPDATE terms SET parent_id = NULL WHERE parent_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("null children failed: {err}")))?;
        }
        sqlx::query("DELETE FROM terms WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("term delete failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::db(format!("tx commit failed: {err}")))?;
        Ok(())
    }

    /// Merges `from` into `into` by moving relationships and deleting `from`.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn merge(&self, from: i64, into: i64) -> Result<(), AppError> {
        self.delete(from, Some(into)).await
    }

    /// Replaces a post's terms with `term_ids` (replace-set semantics).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn set_post_terms(&self, post_id: i64, term_ids: &[i64]) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))?;
        replace_post_terms(&mut tx, post_id, term_ids).await?;
        tx.commit()
            .await
            .map_err(|err| AppError::db(format!("tx commit failed: {err}")))?;
        Ok(())
    }

    /// Lists term ids for a post.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_for_post(&self, post_id: i64) -> Result<Vec<i64>, AppError> {
        sqlx::query_scalar("SELECT term_id FROM term_relationships WHERE post_id = $1")
            .bind(post_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("list post terms failed: {err}")))
    }

    /// Checks if a term id exists.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn exists(&self, id: i64) -> Result<bool, AppError> {
        let found: Option<i64> = sqlx::query_scalar("SELECT id FROM terms WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("term exists check failed: {err}")))?;
        Ok(found.is_some())
    }

    /// Returns the ancestor ids of a term via recursive CTE, nearest
    /// first.
    ///
    /// Walks at most [`MAX_TERM_DEPTH`] levels and never revisits a term,
    /// so a cycle that got into the table (by hand, or before the guards
    /// existed) ends the walk instead of looping until the server runs
    /// out of memory.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn ancestors(&self, id: i64) -> Result<Vec<i64>, AppError> {
        ancestors_of(&self.pool, id).await
    }
}

/// How deep a term hierarchy is walked before giving up.
pub const MAX_TERM_DEPTH: i32 = 100;

async fn ancestors_of<'e, E>(executor: E, id: i64) -> Result<Vec<i64>, AppError>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_scalar(
        "WITH RECURSIVE up(id, depth, path) AS (
            SELECT parent_id, 1, ARRAY[id] FROM terms
             WHERE id = $1 AND parent_id IS NOT NULL
            UNION ALL
            SELECT t.parent_id, up.depth + 1, up.path || up.id
              FROM terms t JOIN up ON t.id = up.id
             WHERE t.parent_id IS NOT NULL
               AND NOT (up.id = ANY(up.path))
               AND up.depth < $2
         ) SELECT id FROM up WHERE NOT (id = ANY(path)) ORDER BY depth",
    )
    .bind(id)
    .bind(MAX_TERM_DEPTH)
    .fetch_all(executor)
    .await
    .map_err(|err| AppError::db(format!("ancestors query failed: {err}")))
}

/// Replaces a post's term assignments inside the caller's transaction.
///
/// # Errors
///
/// Returns [`AppError::Db`] on database failure (an unknown term id is
/// refused by the foreign key).
pub(crate) async fn replace_post_terms(
    conn: &mut sqlx::PgConnection,
    post_id: i64,
    term_ids: &[i64],
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM term_relationships WHERE post_id = $1")
        .bind(post_id)
        .execute(&mut *conn)
        .await
        .map_err(|err| AppError::db(format!("clear post terms failed: {err}")))?;
    sqlx::query(
        "INSERT INTO term_relationships (post_id, term_id)
         SELECT $1, t FROM UNNEST($2::bigint[]) AS t
         ON CONFLICT DO NOTHING",
    )
    .bind(post_id)
    .bind(term_ids)
    .execute(&mut *conn)
    .await
    .map_err(|err| AppError::db(format!("insert post terms failed: {err}")))?;
    Ok(())
}

/// Parameters for inserting a term.
pub struct NewTerm<'a> {
    /// Snowflake id.
    pub id: i64,
    /// Taxonomy (category/tag).
    pub taxonomy: Taxonomy,
    /// Human name.
    pub name: &'a str,
    /// Slug (unique per taxonomy).
    pub slug: &'a str,
    /// Optional parent for hierarchy.
    pub parent_id: Option<i64>,
    /// Extension metadata.
    pub meta: serde_json::Value,
}

/// Partial update for a term.
pub struct TermUpdate<'a> {
    /// New name.
    pub name: Option<&'a str>,
    /// New slug.
    pub slug: Option<&'a str>,
    /// New meta.
    pub meta: Option<&'a serde_json::Value>,
}

/// Term with post count (for listing).
#[derive(Clone, Debug)]
pub struct TermWithCount {
    /// Term row.
    pub term: TermRow,
    /// Number of posts assigned to this term.
    pub post_count: i64,
}

#[derive(sqlx::FromRow)]
struct TermCountRow {
    id: i64,
    taxonomy: Taxonomy,
    name: String,
    slug: String,
    parent_id: Option<i64>,
    meta: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
    post_count: i64,
}

impl From<TermCountRow> for TermWithCount {
    fn from(row: TermCountRow) -> Self {
        Self {
            term: TermRow {
                id: row.id,
                taxonomy: row.taxonomy,
                name: row.name,
                slug: row.slug,
                parent_id: row.parent_id,
                meta: row.meta,
                created_at: row.created_at,
            },
            post_count: row.post_count,
        }
    }
}

impl TermsRepo {
    /// Finds a term id whose meta records the given importer key.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn find_id_by_import_key(&self, key: &str) -> Result<Option<i64>, AppError> {
        sqlx::query_scalar::<_, i64>("SELECT id FROM terms WHERE meta ? $1 LIMIT 1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("term import lookup failed: {err}")))
    }
}
