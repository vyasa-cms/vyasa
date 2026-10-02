#![allow(clippy::pedantic, clippy::missing_errors_doc)]
#![allow(missing_docs)]
//! Media derivatives worker.

use sqlx::PgPool;
use vyasa_db::repo::MediaRepo;

/// Processes a `media_derivatives` job: generates blurhash + derivatives.
///
/// `storage` is the backend the bytes actually live in. The worker used to
/// build its own `LocalFsBackend` from `VYASA_MEDIA_DIR`, which was wrong
/// on every object-storage deployment (nothing on disk, every job failed,
/// every upload left with no blurhash and no dimensions) and wrong on a
/// local one configured through `vyasa.toml` rather than the environment
/// (derivatives written under a directory the server never serves).
pub async fn handle(
    pool: &PgPool,
    storage: &dyn vyasa_core::media::StorageBackend,
    payload: serde_json::Value,
) -> Result<(), String> {
    let media_id = payload
        .get("media_id")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| "missing media_id".to_string())?;
    let repo = MediaRepo::new(pool.clone());
    let row = repo
        .get(media_id)
        .await
        .map_err(|e| format!("media get failed: {e}"))?;
    let bytes = storage
        .get(&row.path)
        .await
        .map_err(|e| format!("storage get failed: {e}"))?;
    // Generate blurhash and derivatives.
    let blurhash = vyasa_core::media::derivatives::blurhash_for(&bytes);
    let derivatives = vyasa_core::media::derivatives::generate_derivatives(
        storage,
        &row.path,
        &row.file_name,
        &bytes,
    )
    .await
    .map_err(|e| format!("derivatives failed: {e}"))?;
    // Update DB: blurhash + derivatives + dimensions if image.
    let dims = vyasa_core::media::derivatives::dimensions_of(&bytes);
    let (w, h) = dims.unzip();
    // Update via direct SQL (avoid repo round-trip for now).
    sqlx::query("UPDATE media SET blurhash = $1, derivatives = $2, width = COALESCE($3, width), height = COALESCE($4, height) WHERE id = $5")
        .bind(&blurhash)
        .bind(&derivatives)
        .bind(w.map(|v| i32::try_from(v).unwrap_or(i32::MAX)))
        .bind(h.map(|v| i32::try_from(v).unwrap_or(i32::MAX)))
        .bind(media_id)
        .execute(pool)
        .await
        .map_err(|e| format!("media update failed: {e}"))?;
    Ok(())
}
