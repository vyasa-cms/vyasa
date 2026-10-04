#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! Dataloaders for GraphQL — batched author/terms/media fetches.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_graphql::dataloader::Loader;
use sqlx::PgPool;

use vyasa_db::content_models::{MediaRow, TermRow};
use vyasa_db::models::UserRow;

// Global batch counters for tests — reset before each test.
static AUTHOR_BATCHES: AtomicUsize = AtomicUsize::new(0);
static TERMS_BATCHES: AtomicUsize = AtomicUsize::new(0);
static MEDIA_BATCHES: AtomicUsize = AtomicUsize::new(0);

/// Resets batch counters (test helper).
#[allow(dead_code)]
pub fn reset_counters() {
    AUTHOR_BATCHES.store(0, Ordering::SeqCst);
    TERMS_BATCHES.store(0, Ordering::SeqCst);
    MEDIA_BATCHES.store(0, Ordering::SeqCst);
}

/// Returns current counter values `(author, terms, media)`.
#[allow(dead_code)]
pub fn counters() -> (usize, usize, usize) {
    (
        AUTHOR_BATCHES.load(Ordering::SeqCst),
        TERMS_BATCHES.load(Ordering::SeqCst),
        MEDIA_BATCHES.load(Ordering::SeqCst),
    )
}

/// Human names for counters — for diagnostics.
#[allow(dead_code)]
pub fn counters_snapshot() -> String {
    let (a, t, m) = counters();
    format!("author={a} terms={t} media={m}")
}

/// Batches user loads by id.
#[derive(Clone)]
pub struct AuthorLoader {
    pool: PgPool,
}

impl AuthorLoader {
    /// Creates a loader bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl Loader<i64> for AuthorLoader {
    type Value = UserRow;
    type Error = Arc<str>;

    async fn load(&self, keys: &[i64]) -> Result<HashMap<i64, Self::Value>, Self::Error> {
        AUTHOR_BATCHES.fetch_add(1, Ordering::SeqCst);
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, UserRow>(sqlx::AssertSqlSafe(format!(
            // An account that has not confirmed its address is nobody's
            // public author: it loads as nothing, and `author` is null.
            "SELECT {} FROM users WHERE id = ANY($1) AND email_verified_at IS NOT NULL",
            vyasa_db::repo::users::USER_COLUMNS
        )))
        .bind(keys)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Arc::<str>::from(format!("author load failed: {e}")))?;
        let mut map = HashMap::new();
        for row in rows {
            map.insert(row.id, row);
        }
        Ok(map)
    }
}

/// Batches term loads for posts: key is post_id, value is Vec<TermRow>.
#[derive(Clone)]
pub struct TermsForPostLoader {
    pool: PgPool,
}

impl TermsForPostLoader {
    /// Creates a loader bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl Loader<i64> for TermsForPostLoader {
    type Value = Vec<TermRow>;
    type Error = Arc<str>;

    async fn load(&self, keys: &[i64]) -> Result<HashMap<i64, Self::Value>, Self::Error> {
        TERMS_BATCHES.fetch_add(1, Ordering::SeqCst);
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        // Join to get terms with post_id.
        let rows = sqlx::query_as::<_, TermWithPost>(
            "SELECT t.id, t.taxonomy, t.name, t.slug, t.parent_id, t.meta, t.created_at, tr.post_id as post_id \
             FROM terms t JOIN term_relationships tr ON t.id = tr.term_id \
             WHERE tr.post_id = ANY($1)",
        )
        .bind(keys)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Arc::<str>::from(format!("terms load failed: {e}")))?;
        let mut map: HashMap<i64, Vec<TermRow>> = HashMap::new();
        for r in rows {
            map.entry(r.post_id).or_default().push(TermRow {
                id: r.id,
                taxonomy: r.taxonomy,
                name: r.name,
                slug: r.slug,
                parent_id: r.parent_id,
                meta: r.meta,
                created_at: r.created_at,
            });
        }
        // Ensure missing posts get empty vec (loader expects missing keys omitted; resolver will handle missing)
        for k in keys {
            map.entry(*k).or_default();
        }
        Ok(map)
    }
}

#[derive(sqlx::FromRow)]
struct TermWithPost {
    id: i64,
    taxonomy: vyasa_db::content_models::Taxonomy,
    name: String,
    slug: String,
    parent_id: Option<i64>,
    meta: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
    post_id: i64,
}

/// Batches media loads by id.
#[derive(Clone)]
pub struct MediaLoader {
    pool: PgPool,
}

impl MediaLoader {
    /// Creates a loader bound to `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl Loader<i64> for MediaLoader {
    type Value = MediaRow;
    type Error = Arc<str>;

    async fn load(&self, keys: &[i64]) -> Result<HashMap<i64, Self::Value>, Self::Error> {
        MEDIA_BATCHES.fetch_add(1, Ordering::SeqCst);
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, MediaRow>(
            "SELECT id, owner_id, file_name, mime, byte_size, storage, path, width, height, blurhash, alt, caption, derivatives, created_at FROM media WHERE id = ANY($1)",
        )
        .bind(keys)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Arc::<str>::from(format!("media load failed: {e}")))?;
        let mut map = HashMap::new();
        for row in rows {
            map.insert(row.id, row);
        }
        Ok(map)
    }
}
