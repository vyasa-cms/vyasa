//! Entry languages and translation groups.

use sqlx::PgPool;
use vyasa_common::AppError;

/// One entry's language + which group of translations it belongs to.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TranslationRow {
    /// The entry.
    pub post_id: i64,
    /// BCP-47-ish language tag as the author wrote it.
    pub lang: String,
    /// Entries sharing a `group_id` are translations of each other.
    pub group_id: i64,
}

/// A published group member with what a hreflang link needs.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlternateRow {
    /// The entry.
    pub post_id: i64,
    /// Its language.
    pub lang: String,
    /// Its slug.
    pub slug: String,
    /// Its post type (for the permalink rule).
    pub post_type: String,
    /// Its title (for admin display).
    pub title: String,
}

/// Repository over `post_translations`.
#[derive(Clone, Debug)]
pub struct TranslationsRepo {
    pool: PgPool,
}

impl TranslationsRepo {
    /// Binds to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The entry's row, when it has declared a language.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn get(&self, post_id: i64) -> Result<Option<TranslationRow>, AppError> {
        sqlx::query_as("SELECT post_id, lang, group_id FROM post_translations WHERE post_id = $1")
            .bind(post_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("translation get: {e}")))
    }

    /// Declares (or changes) an entry's language, keeping its group.
    /// A fresh entry starts a group of its own (`group_id = post_id`).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_lang(&self, post_id: i64, lang: &str) -> Result<TranslationRow, AppError> {
        sqlx::query_as(
            "INSERT INTO post_translations (post_id, lang, group_id)
             VALUES ($1, $2, $1)
             ON CONFLICT (post_id) DO UPDATE SET lang = EXCLUDED.lang
             RETURNING post_id, lang, group_id",
        )
        .bind(post_id)
        .bind(lang)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("translation set: {e}")))
    }

    /// Moves the entry into `group_id` (its language must already be
    /// declared).
    ///
    /// # Errors
    /// [`AppError::NotFound`] when the entry has no language row.
    pub async fn join_group(&self, post_id: i64, group_id: i64) -> Result<(), AppError> {
        let done = sqlx::query("UPDATE post_translations SET group_id = $2 WHERE post_id = $1")
            .bind(post_id)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("translation join: {e}")))?;
        if done.rows_affected() == 0 {
            return Err(AppError::not_found("post_translation", post_id));
        }
        Ok(())
    }

    /// Forgets the entry's language (and group membership).
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn clear(&self, post_id: i64) -> Result<(), AppError> {
        sqlx::query("DELETE FROM post_translations WHERE post_id = $1")
            .bind(post_id)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("translation clear: {e}")))?;
        Ok(())
    }

    /// The published members of the entry's group (itself included),
    /// ready for hreflang links. Empty when the entry has no language.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn alternates(&self, post_id: i64) -> Result<Vec<AlternateRow>, AppError> {
        sqlx::query_as(
            "SELECT t.post_id, t.lang, p.slug, p.type AS post_type, p.title
             FROM post_translations t
             JOIN posts p ON p.id = t.post_id
             WHERE t.group_id = (SELECT group_id FROM post_translations WHERE post_id = $1)
               AND p.status = 'published' AND p.password_hash IS NULL
             ORDER BY t.lang",
        )
        .bind(post_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("translation alternates: {e}")))
    }

    /// Every row, for the sitemap's alternate links.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn all(&self) -> Result<Vec<TranslationRow>, AppError> {
        sqlx::query_as("SELECT post_id, lang, group_id FROM post_translations")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::db(format!("translation all: {e}")))
    }
}
