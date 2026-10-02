//! Media repository.

use sqlx::PgPool;
use vyasa_common::AppError;

use crate::content_models::{MediaRow, MediaStorage};

const MEDIA_COLUMNS: &str =
    "id, owner_id, file_name, mime, byte_size, storage, path, width, height, blurhash, alt, caption, derivatives, created_at, sha256, focal_x, focal_y";

/// How a library listing is filtered and ordered.
#[derive(Debug, Default, Clone)]
pub struct MediaFilter {
    /// Case-insensitive match on file name, alt or caption.
    pub search: Option<String>,
    /// `image`, `video`, `audio`, `document`, or `other`.
    pub kind: Option<String>,
    /// Only files this user uploaded.
    pub owner_id: Option<i64>,
    /// `newest` (default), `oldest`, `largest`, `smallest`, `name`.
    pub sort: Option<String>,
    /// List what is in the trash instead of the library.
    pub trashed: bool,
}

impl MediaFilter {
    fn kind_clause(&self) -> &'static str {
        match self.kind.as_deref() {
            Some("image") => " AND mime LIKE 'image/%'",
            Some("video") => " AND mime LIKE 'video/%'",
            Some("audio") => " AND mime LIKE 'audio/%'",
            Some("document") => " AND mime = 'application/pdf'",
            Some("other") => " AND mime NOT LIKE 'image/%' AND mime NOT LIKE 'video/%' AND mime NOT LIKE 'audio/%' AND mime <> 'application/pdf'",
            _ => "",
        }
    }
    fn order(&self) -> &'static str {
        match self.sort.as_deref() {
            Some("oldest") => "created_at ASC",
            Some("largest") => "byte_size DESC",
            Some("smallest") => "byte_size ASC",
            Some("name") => "file_name ASC",
            _ => "created_at DESC",
        }
    }
}

/// Library totals for the page header and Site health.
#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct MediaStats {
    /// Files in the library.
    pub count: i64,
    /// Bytes of originals; derivatives are not counted.
    pub bytes: i64,
    /// Images with no alt text.
    pub missing_alt: i64,
}

/// Data access for the `media` table.
#[derive(Clone, Debug)]
pub struct MediaRepo {
    pool: PgPool,
}

impl MediaRepo {
    /// Returns the underlying pool.
    #[must_use]
    pub fn pool(&self) -> sqlx::PgPool {
        self.pool.clone()
    }

    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a media row.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn insert(&self, media: &NewMedia<'_>) -> Result<MediaRow, AppError> {
        sqlx::query_as(&format!(
            "INSERT INTO media (id, owner_id, file_name, mime, byte_size, storage, path, width, height, blurhash, alt, caption, derivatives, sha256)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
             RETURNING {MEDIA_COLUMNS}"
        ))
        .bind(media.id)
        .bind(media.owner_id)
        .bind(media.file_name)
        .bind(media.mime)
        .bind(media.byte_size)
        .bind(media.storage.as_str())
        .bind(media.path)
        .bind(media.width)
        .bind(media.height)
        .bind(media.blurhash)
        .bind(media.alt)
        .bind(media.caption)
        .bind(&media.derivatives)
        .bind(media.sha256)
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media insert failed: {err}")))
    }

    /// A library listing with search, kind, owner and order, plus the
    /// total the filter matches so a page can say where it is.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_filtered(
        &self,
        filter: &MediaFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<MediaRow>, i64), AppError> {
        let search = filter
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("%{}%", s.replace('%', "\\%").replace('_', "\\_")));
        let where_clause = format!(
            "WHERE ($1::text IS NULL OR file_name ILIKE $1 OR alt ILIKE $1 OR caption ILIKE $1)
             AND ($2::bigint IS NULL OR owner_id = $2) AND trashed_at IS {}{}",
            if filter.trashed { "NOT NULL" } else { "NULL" },
            filter.kind_clause()
        );
        let rows = sqlx::query_as::<_, MediaRow>(&format!(
            "SELECT {MEDIA_COLUMNS} FROM media {where_clause} ORDER BY {} LIMIT $3 OFFSET $4",
            filter.order()
        ))
        .bind(&search)
        .bind(filter.owner_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media list failed: {err}")))?;
        let total =
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM media {where_clause}"))
                .bind(&search)
                .bind(filter.owner_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|err| AppError::db(format!("media count failed: {err}")))?;
        Ok((rows, total))
    }

    /// Library totals.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn stats(&self) -> Result<MediaStats, AppError> {
        sqlx::query_as::<_, MediaStats>(
            "SELECT count(*) AS count, COALESCE(sum(byte_size), 0)::bigint AS bytes,
                    count(*) FILTER (WHERE mime LIKE 'image/%' AND COALESCE(alt, '') = '') AS missing_alt
             FROM media WHERE trashed_at IS NULL",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media stats failed: {err}")))
    }

    /// The row holding these exact bytes, if the library has one.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn find_by_sha256(&self, sha256: &str) -> Result<Option<MediaRow>, AppError> {
        sqlx::query_as::<_, MediaRow>(&format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE sha256 = $1 ORDER BY created_at ASC LIMIT 1"
        ))
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media lookup failed: {err}")))
    }

    /// Renames the file and sets the focal point; absent fields are kept.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure, `NotFound` when missing.
    pub async fn update_details(
        &self,
        id: i64,
        file_name: Option<&str>,
        focal: Option<(f32, f32)>,
    ) -> Result<MediaRow, AppError> {
        sqlx::query_as::<_, MediaRow>(&format!(
            "UPDATE media SET file_name = COALESCE($2, file_name),
                 focal_x = COALESCE($3, focal_x), focal_y = COALESCE($4, focal_y)
             WHERE id = $1 RETURNING {MEDIA_COLUMNS}"
        ))
        .bind(id)
        .bind(file_name)
        .bind(focal.map(|f| f.0))
        .bind(focal.map(|f| f.1))
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media details update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("media", id))
    }

    /// Points the row at new bytes: a replacement keeps the id, so every
    /// post that embeds it keeps working.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure, `NotFound` when missing.
    pub async fn replace_file(
        &self,
        id: i64,
        file_name: &str,
        mime: &str,
        byte_size: i64,
        path: &str,
        sha256: &str,
    ) -> Result<MediaRow, AppError> {
        sqlx::query_as::<_, MediaRow>(&format!(
            "UPDATE media SET file_name = $2, mime = $3, byte_size = $4, path = $5, sha256 = $6,
                 width = NULL, height = NULL, blurhash = NULL, derivatives = '{{}}'::jsonb
             WHERE id = $1 RETURNING {MEDIA_COLUMNS}"
        ))
        .bind(id)
        .bind(file_name)
        .bind(mime)
        .bind(byte_size)
        .bind(path)
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media replace failed: {err}")))?
        .ok_or_else(|| AppError::not_found("media", id))
    }

    /// Fetches a media row by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<MediaRow, AppError> {
        sqlx::query_as(&format!("SELECT {MEDIA_COLUMNS} FROM media WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("media lookup failed: {err}")))?
            .ok_or_else(|| AppError::not_found("media", id))
    }

    /// Whether a media row exists and is in the trash: `None` when there
    /// is no such row, `Some(true)` when it is trashed. (`get` does not
    /// read `trashed_at`.)
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn trash_state(&self, id: i64) -> Result<Option<bool>, AppError> {
        sqlx::query_scalar("SELECT trashed_at IS NOT NULL FROM media WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("media lookup failed: {err}")))
    }

    /// The media items among `ids` that exist outside the trash (one
    /// query, for resolving many references at once).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn get_many_live(&self, ids: &[i64]) -> Result<Vec<MediaRow>, AppError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as(&format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE id = ANY($1) AND trashed_at IS NULL"
        ))
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media lookup failed: {err}")))
    }

    /// Lists media, newest first, paginated.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<MediaRow>, AppError> {
        sqlx::query_as(&format!(
            "SELECT {MEDIA_COLUMNS} FROM media ORDER BY created_at DESC LIMIT $1 OFFSET $2"
        ))
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media list failed: {err}")))
    }

    /// Lists media for an owner, newest first, paginated.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_for_owner(
        &self,
        owner_id: i64,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<MediaRow>, AppError> {
        sqlx::query_as(&format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE owner_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3"
        ))
        .bind(owner_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media list for owner failed: {err}")))
    }

    /// Updates the author-supplied metadata (alt text, caption).
    ///
    /// `None` leaves a field untouched, so a caller can set alt text without
    /// clearing a caption it never loaded.
    ///
    /// # Errors
    /// [`AppError::NotFound`] when the row is missing, [`AppError::Db`] on
    /// failure.
    pub async fn update_meta(
        &self,
        id: i64,
        alt: Option<&str>,
        caption: Option<&str>,
    ) -> Result<MediaRow, AppError> {
        sqlx::query_as::<_, MediaRow>(&format!(
            "UPDATE media
                SET alt = COALESCE($2, alt),
                    caption = COALESCE($3, caption)
              WHERE id = $1
              RETURNING {MEDIA_COLUMNS}"
        ))
        .bind(id)
        .bind(alt)
        .bind(caption)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media metadata update failed: {err}")))?
        .ok_or_else(|| AppError::not_found("media", id))
    }

    /// Moves a row to the trash; the file stays until purged.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn trash(&self, id: i64) -> Result<(), AppError> {
        let n =
            sqlx::query("UPDATE media SET trashed_at = now() WHERE id = $1 AND trashed_at IS NULL")
                .bind(id)
                .execute(&self.pool)
                .await
                .map_err(|err| AppError::db(format!("media trash failed: {err}")))?
                .rows_affected();
        if n == 0 {
            return Err(AppError::not_found("media", id));
        }
        Ok(())
    }

    /// Brings a row back from the trash.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn restore(&self, id: i64) -> Result<(), AppError> {
        let n = sqlx::query("UPDATE media SET trashed_at = NULL WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("media restore failed: {err}")))?
            .rows_affected();
        if n == 0 {
            return Err(AppError::not_found("media", id));
        }
        Ok(())
    }

    /// Everything in the trash, for purging.
    ///
    /// # Errors
    /// Returns [`AppError::Db`] on database failure.
    pub async fn trashed(&self) -> Result<Vec<MediaRow>, AppError> {
        sqlx::query_as::<_, MediaRow>(&format!(
            "SELECT {MEDIA_COLUMNS} FROM media WHERE trashed_at IS NOT NULL"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media trashed failed: {err}")))
    }

    /// Deletes a media row.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let result = sqlx::query("DELETE FROM media WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("media delete failed: {err}")))?;
        if result.rows_affected() == 0 {
            return Err(AppError::not_found("media", id));
        }
        Ok(())
    }

    /// Updates derivatives and blurhash for a media row.
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn update_derivatives(
        &self,
        id: i64,
        derivatives: &serde_json::Value,
        blurhash: &str,
        width: Option<i32>,
        height: Option<i32>,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE media SET derivatives = $1, blurhash = $2, width = COALESCE($3, width), height = COALESCE($4, height) WHERE id = $5",
        )
        .bind(derivatives)
        .bind(blurhash)
        .bind(width)
        .bind(height)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|err| AppError::db(format!("media update derivatives failed: {err}")))?;
        Ok(())
    }

    /// Counts media rows.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn count(&self) -> Result<i64, AppError> {
        sqlx::query_scalar("SELECT COUNT(*) FROM media")
            .fetch_one(&self.pool)
            .await
            .map_err(|err| AppError::db(format!("media count failed: {err}")))
    }
}

/// Parameters for inserting a media row.
pub struct NewMedia<'a> {
    /// Snowflake id.
    pub id: i64,
    /// Owner user id.
    pub owner_id: i64,
    /// Original file name.
    pub file_name: &'a str,
    /// MIME type (sniffed).
    pub mime: &'a str,
    /// Size in bytes.
    pub byte_size: i64,
    /// Storage backend.
    pub storage: MediaStorage,
    /// Storage-relative path.
    pub path: &'a str,
    /// Pixel width, if image.
    pub width: Option<i32>,
    /// Pixel height, if image.
    pub height: Option<i32>,
    /// Blurhash placeholder.
    pub blurhash: Option<&'a str>,
    /// Alt text.
    pub alt: Option<&'a str>,
    /// Caption.
    pub caption: Option<&'a str>,
    /// Derivatives JSON (empty for now).
    pub derivatives: serde_json::Value,
    /// Hex SHA-256 of the bytes.
    pub sha256: Option<&'a str>,
}
