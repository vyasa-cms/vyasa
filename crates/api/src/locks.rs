//! Who has an entry open in the editor.
//!
//! Not live co-editing: a lock names the person editing, the editor
//! heartbeats it while open, and a second person sees who is there and
//! may take over. Together with the save conflict check this is what
//! stops two people overwriting each other without either knowing.

use serde::Serialize;
use sqlx::PgPool;
use vyasa_common::AppError;

/// How long a heartbeat keeps a lock alive.
pub const LOCK_TTL_SECS: i64 = 90;

/// The current state of an entry's lock, from the asker's point of view.
#[derive(Serialize, utoipa::ToSchema)]
pub struct LockState {
    /// True when the asker holds it (or just took it).
    pub mine: bool,
    /// Who holds it when it is not the asker's.
    pub holder_name: Option<String>,
    /// Seconds since the holder was last seen.
    pub seen_ago_secs: Option<i64>,
}

/// Takes or refreshes the lock for `user`, unless someone else holds a
/// live one; then reports them.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn acquire(
    pool: &PgPool,
    post_id: i64,
    user_id: i64,
    display_name: &str,
    force: bool,
) -> Result<LockState, AppError> {
    let holder = sqlx::query_as::<_, (i64, String, chrono::DateTime<chrono::Utc>)>(
        "SELECT user_id, display_name, seen_at FROM post_locks WHERE post_id = $1",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::db(format!("lock: {e}")))?;
    if let Some((uid, name, seen)) = holder {
        let ago = (chrono::Utc::now() - seen).num_seconds();
        if uid != user_id && ago < LOCK_TTL_SECS && !force {
            return Ok(LockState {
                mine: false,
                holder_name: Some(name),
                seen_ago_secs: Some(ago),
            });
        }
    }
    sqlx::query(
        "INSERT INTO post_locks (post_id, user_id, display_name, seen_at) VALUES ($1, $2, $3, now())
         ON CONFLICT (post_id) DO UPDATE SET user_id = EXCLUDED.user_id,
             display_name = EXCLUDED.display_name, seen_at = now()",
    )
    .bind(post_id)
    .bind(user_id)
    .bind(display_name)
    .execute(pool)
    .await
    .map_err(|e| AppError::db(format!("lock: {e}")))?;
    Ok(LockState {
        mine: true,
        holder_name: None,
        seen_ago_secs: None,
    })
}

/// Lets go of the lock if `user` holds it.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn release(pool: &PgPool, post_id: i64, user_id: i64) -> Result<(), AppError> {
    sqlx::query("DELETE FROM post_locks WHERE post_id = $1 AND user_id = $2")
        .bind(post_id)
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::db(format!("lock: {e}")))?;
    Ok(())
}
