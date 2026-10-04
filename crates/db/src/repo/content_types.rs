//! Content types repository: administrator-created post types in the
//! `content_types` table. Slug rules, the registry and conflict checks
//! live in `vyasa-core`; the table's own constraints are the backstop.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use vyasa_common::AppError;

const TYPE_COLUMNS: &str =
    "slug, singular, plural, description, public, has_archive, created_at, updated_at";

/// One administrator-created content type.
#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow, serde::Serialize)]
pub struct ContentTypeRow {
    /// The `posts.type` value and URL segment; immutable.
    pub slug: String,
    /// Singular label ("Product").
    pub singular: String,
    /// Plural label ("Products").
    pub plural: String,
    /// What the type is for.
    pub description: String,
    /// Whether entries get public URLs.
    pub public: bool,
    /// Whether `/{slug}/` lists the entries.
    pub has_archive: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last change.
    pub updated_at: DateTime<Utc>,
}

/// Parameters for inserting a content type.
#[derive(Clone, Copy, Debug)]
pub struct NewContentType<'a> {
    /// Slug (primary key).
    pub slug: &'a str,
    /// Singular label.
    pub singular: &'a str,
    /// Plural label.
    pub plural: &'a str,
    /// Description.
    pub description: &'a str,
    /// Whether entries get public URLs.
    pub public: bool,
    /// Whether the type has an archive.
    pub has_archive: bool,
}

/// Fields of a content type to change; `None` leaves one as it is. The
/// slug is not here: it is in URLs and templates, so it never changes.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContentTypeUpdate<'a> {
    /// New singular label.
    pub singular: Option<&'a str>,
    /// New plural label.
    pub plural: Option<&'a str>,
    /// New description.
    pub description: Option<&'a str>,
    /// New public flag.
    pub public: Option<bool>,
    /// New archive flag.
    pub has_archive: Option<bool>,
}

/// What [`ContentTypesRepo::delete_if_unused`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeDeletion {
    /// The type and its field definitions are gone.
    Deleted,
    /// Entries of the type exist (any status); nothing was deleted.
    InUse(i64),
    /// There was no such type.
    Missing,
}

/// Data access for the `content_types` table.
#[derive(Clone, Debug)]
pub struct ContentTypesRepo {
    pool: PgPool,
}

impl ContentTypesRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Every content type, by slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self) -> Result<Vec<ContentTypeRow>, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {TYPE_COLUMNS} FROM content_types ORDER BY slug"
        )))
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("content type list failed: {err}")))
    }

    /// Fetches a content type by slug.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such type, or
    /// [`AppError::Db`] on database failure.
    pub async fn get(&self, slug: &str) -> Result<ContentTypeRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {TYPE_COLUMNS} FROM content_types WHERE slug = $1"
        )))
        .bind(slug)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("content type lookup failed: {err}")))?
        .ok_or_else(|| AppError::not_found("content_type", slug))
    }

    /// Inserts a content type.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Conflict`] when the slug is taken — also when
    /// two inserts of one slug race, where the primary key lets exactly
    /// one through — or [`AppError::Db`] on database failure (including a
    /// value the table's constraints refuse).
    pub async fn insert(&self, row: &NewContentType<'_>) -> Result<ContentTypeRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "INSERT INTO content_types (slug, singular, plural, description, public, has_archive)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING {TYPE_COLUMNS}"
        )))
        .bind(row.slug)
        .bind(row.singular)
        .bind(row.plural)
        .bind(row.description)
        .bind(row.public)
        .bind(row.has_archive)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| match err {
            sqlx::Error::Database(db) if db.is_unique_violation() => AppError::conflict(format!(
                "a content type with the slug {:?} already exists",
                row.slug
            )),
            err => AppError::db(format!("content type insert failed: {err}")),
        })
    }

    /// Updates a content type's labels and flags; each is optional.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when there is no such type, or
    /// [`AppError::Db`] on database failure.
    pub async fn update(
        &self,
        slug: &str,
        update: &ContentTypeUpdate<'_>,
    ) -> Result<ContentTypeRow, AppError> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "UPDATE content_types SET
                singular = COALESCE($2, singular),
                plural = COALESCE($3, plural),
                description = COALESCE($4, description),
                public = COALESCE($5, public),
                has_archive = COALESCE($6, has_archive),
                updated_at = now()
             WHERE slug = $1
             RETURNING {TYPE_COLUMNS}"
        )))
        .bind(slug)
        .bind(update.singular)
        .bind(update.plural)
        .bind(update.description)
        .bind(update.public)
        .bind(update.has_archive)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("content type update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("content_type", slug))
    }

    /// Entries of the type in any status, trash included.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count_entries(&self, slug: &str) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE type = $1")
            .bind(slug)
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("content type entry count failed: {err}")))
    }

    /// Deletes a type and its field definitions, in one transaction, when
    /// no entry of it exists in any status. The type's row is locked
    /// while the entries are counted.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete_if_unused(&self, slug: &str) -> Result<TypeDeletion, AppError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| AppError::db(format!("tx begin failed: {err}")))?;
        let found: Option<String> =
            sqlx::query_scalar("SELECT slug FROM content_types WHERE slug = $1 FOR UPDATE")
                .bind(slug)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|err| AppError::db(format!("content type lock failed: {err}")))?;
        if found.is_none() {
            return Ok(TypeDeletion::Missing);
        }
        let entries: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts WHERE type = $1")
            .bind(slug)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("content type entry count failed: {err}")))?;
        if entries > 0 {
            return Ok(TypeDeletion::InUse(entries));
        }
        sqlx::query("DELETE FROM content_fields WHERE type_slug = $1")
            .bind(slug)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("content field delete failed: {err}")))?;
        sqlx::query("DELETE FROM content_types WHERE slug = $1")
            .bind(slug)
            .execute(&mut *tx)
            .await
            .map_err(|err| AppError::db(format!("content type delete failed: {err}")))?;
        tx.commit()
            .await
            .map_err(|err| AppError::db(format!("tx commit failed: {err}")))?;
        Ok(TypeDeletion::Deleted)
    }
}
