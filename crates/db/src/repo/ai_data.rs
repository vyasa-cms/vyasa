//! Storage for the AI integrations (migration 0023): post embeddings,
//! comment moderation verdicts, media transcripts and read-aloud audio.

use sqlx::PgPool;
use vyasa_common::AppError;

fn db_err(e: &sqlx::Error) -> AppError {
    AppError::db(format!("ai_data: {e}"))
}

/// A stored embedding.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EmbeddingRow {
    /// Post id.
    pub post_id: i64,
    /// Model that produced it.
    pub model: String,
    /// Hash of the embedded text.
    pub text_hash: String,
    /// Vector length.
    pub dims: i32,
    /// The vector.
    pub vector: Vec<f32>,
}

/// The read-aloud recording for a post.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PostAudioRow {
    /// Post id.
    pub post_id: i64,
    /// Media row holding the MP3.
    pub media_id: i64,
    /// Model that produced it.
    pub model: String,
    /// Hash of the spoken text.
    pub text_hash: String,
}

/// Data access for the AI feature tables.
#[derive(Clone, Debug)]
pub struct AiDataRepo {
    pool: PgPool,
}

impl AiDataRepo {
    /// Creates a repo bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The stored embedding for a post, if any.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn embedding(&self, post_id: i64) -> Result<Option<EmbeddingRow>, AppError> {
        sqlx::query_as::<_, EmbeddingRow>(
            "SELECT post_id, model, text_hash, dims, vector FROM post_embeddings WHERE post_id = $1",
        )
        .bind(post_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// Stores (or replaces) a post's embedding.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn upsert_embedding(
        &self,
        post_id: i64,
        model: &str,
        text_hash: &str,
        vector: &[f32],
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO post_embeddings (post_id, model, text_hash, dims, vector) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (post_id) DO UPDATE SET model = EXCLUDED.model, \
               text_hash = EXCLUDED.text_hash, dims = EXCLUDED.dims, \
               vector = EXCLUDED.vector, updated_at = now()",
        )
        .bind(post_id)
        .bind(model)
        .bind(text_hash)
        .bind(i32::try_from(vector.len()).unwrap_or(i32::MAX))
        .bind(vector)
        .execute(&self.pool)
        .await
        .map_err(|e| db_err(&e))?;
        Ok(())
    }

    /// Every embedding of a published post with the given dimensionality —
    /// the candidate set for similarity. Blog-scale by design.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    ///
    /// Never a reusable block. With `readable` set — the public's view —
    /// only entries the public may open: not password-protected, and of
    /// type `post`, `page` or one of `readable` (the custom types served
    /// publicly). `None` is an editor's view: every other type, protected
    /// entries included.
    pub async fn published_embeddings(
        &self,
        dims: i32,
        readable: Option<&[String]>,
    ) -> Result<Vec<EmbeddingRow>, AppError> {
        sqlx::query_as::<_, EmbeddingRow>(
            "SELECT e.post_id, e.model, e.text_hash, e.dims, e.vector \
             FROM post_embeddings e JOIN posts p ON p.id = e.post_id \
             WHERE p.status = 'published' AND e.dims = $1 AND p.type <> 'block' \
               AND ($2::text[] IS NULL OR (p.password_hash IS NULL \
                    AND (p.type IN ('post', 'page') OR p.type = ANY($2))))",
        )
        .bind(dims)
        .bind(readable)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// Removes a post's embedding.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn delete_embedding(&self, post_id: i64) -> Result<(), AppError> {
        sqlx::query("DELETE FROM post_embeddings WHERE post_id = $1")
            .bind(post_id)
            .execute(&self.pool)
            .await
            .map_err(|e| db_err(&e))?;
        Ok(())
    }

    /// Records a moderation verdict on a comment.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_comment_moderation(
        &self,
        comment_id: i64,
        verdict: &serde_json::Value,
    ) -> Result<(), AppError> {
        sqlx::query("UPDATE comments SET moderation = $2 WHERE id = $1")
            .bind(comment_id)
            .bind(verdict)
            .execute(&self.pool)
            .await
            .map_err(|e| db_err(&e))?;
        Ok(())
    }

    /// Moderation verdicts for a set of comments, keyed by id.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn comment_moderation(
        &self,
        ids: &[i64],
    ) -> Result<Vec<(i64, serde_json::Value)>, AppError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as::<_, (i64, serde_json::Value)>(
            "SELECT id, moderation FROM comments WHERE id = ANY($1) AND moderation IS NOT NULL",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// Stores a transcript on a media row.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_transcript(&self, media_id: i64, transcript: &str) -> Result<(), AppError> {
        sqlx::query("UPDATE media SET transcript = $2 WHERE id = $1")
            .bind(media_id)
            .bind(transcript)
            .execute(&self.pool)
            .await
            .map_err(|e| db_err(&e))?;
        Ok(())
    }

    /// A media row's transcript.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn transcript(&self, media_id: i64) -> Result<Option<String>, AppError> {
        sqlx::query_scalar::<_, Option<String>>("SELECT transcript FROM media WHERE id = $1")
            .bind(media_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| db_err(&e))
            .map(Option::flatten)
    }

    /// The read-aloud recording for a post.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn post_audio(&self, post_id: i64) -> Result<Option<PostAudioRow>, AppError> {
        sqlx::query_as::<_, PostAudioRow>(
            "SELECT post_id, media_id, model, text_hash FROM post_audio WHERE post_id = $1",
        )
        .bind(post_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| db_err(&e))
    }

    /// Records (or replaces) a post's recording.
    ///
    /// # Errors
    /// [`AppError::Db`] on failure.
    pub async fn set_post_audio(
        &self,
        post_id: i64,
        media_id: i64,
        model: &str,
        text_hash: &str,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO post_audio (post_id, media_id, model, text_hash) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (post_id) DO UPDATE SET media_id = EXCLUDED.media_id, \
               model = EXCLUDED.model, text_hash = EXCLUDED.text_hash, created_at = now()",
        )
        .bind(post_id)
        .bind(media_id)
        .bind(model)
        .bind(text_hash)
        .execute(&self.pool)
        .await
        .map_err(|e| db_err(&e))?;
        Ok(())
    }
}
