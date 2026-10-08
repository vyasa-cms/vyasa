//! Media storage chosen from the admin: S3-compatible object storage
//! configured at runtime, saved in site options with the keys sealed by
//! the server secret, and applied to the running router. The environment
//! (`VYASA_STORAGE__*`) always wins over what the admin saved, so an
//! operator keeps control of where bytes go.
//!
//! Files uploaded before a switch stay where they are and keep serving
//! (the router reads from both stores); "Move existing files" copies them
//! across as a resumable job.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use vyasa_common::{AppError, Secret, StorageConfig};
use vyasa_core::ai_models::KeyVault;
use vyasa_core::media::StorageBackend;
use vyasa_db::content_models::MediaStorage;
use vyasa_db::repo::MediaRepo;

use crate::media_s3::S3Backend;
use crate::state::AppState;

/// Progress of the move job, as stored in `storage_migration`.
pub const MIGRATION_KEY: &str = "storage_migration";

/// The job kind that moves files between stores.
pub const MIGRATE_KIND: &str = "media_migrate";

fn vault(state: &AppState) -> KeyVault {
    KeyVault::new(
        state
            .config
            .secret_key
            .as_ref()
            .map(|k| k.expose().as_bytes()),
    )
}

async fn option_string(state: &AppState, key: &str) -> String {
    state
        .options
        .get(key)
        .await
        .ok()
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Where the active configuration comes from.
#[derive(Serialize, utoipa::ToSchema, PartialEq, Eq, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Saved from the admin.
    Options,
    /// `VYASA_STORAGE__*`; the admin page is read-only.
    Environment,
    /// Local disk.
    None,
}

/// How far the move job got.
#[derive(Serialize, Deserialize, utoipa::ToSchema, Debug, Clone, PartialEq, Eq)]
pub struct MigrationProgress {
    /// `running`, `done` or `failed`.
    pub state: String,
    /// Rows to move when the job started.
    pub total: i64,
    /// Rows moved so far.
    pub done: i64,
    /// Rows that could not be moved; they are retried on the next run.
    pub failed: i64,
    /// Where the bytes are going: `local` or `s3`.
    #[schema(value_type = String)]
    pub to: MediaStorage,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub last_error: Option<String>,
}

/// Files per store.
#[derive(Serialize, utoipa::ToSchema, Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageCounts {
    pub local: i64,
    pub s3: i64,
}

/// The settings, without the secret and with the key id masked.
#[derive(Serialize, utoipa::ToSchema, Debug, Clone)]
// Each flag answers a different question on the page.
#[allow(clippy::struct_excessive_bools)]
pub struct StorageSettings {
    /// `local` or `s3`: where new uploads go.
    pub provider: String,
    pub bucket: String,
    pub region: String,
    pub endpoint: String,
    pub path_style: bool,
    /// The last four characters of the access key id, or empty.
    pub access_key_id_hint: String,
    /// Whether a secret is stored and readable.
    pub has_secret: bool,
    /// Settings say object storage but the stored keys cannot be opened
    /// (the server secret changed): uploads go to local disk until the
    /// keys are entered again.
    pub keys_unreadable: bool,
    pub source: Source,
    /// Whether stored keys are encrypted at rest.
    pub encrypted: bool,
    pub counts: StorageCounts,
    /// Local disk does not survive a restart here (a container platform).
    pub ephemeral_disk: bool,
    pub migration: Option<MigrationProgress>,
}

async fn put_option(state: &AppState, key: &str, value: String) -> Result<(), AppError> {
    state
        .options
        .set(key, &serde_json::Value::String(value))
        .await
}

/// What the admin sends.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct StorageInput {
    /// `local` or `s3`.
    pub provider: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default = "default_path_style")]
    pub path_style: bool,
    /// Omitted: keep the stored one.
    pub access_key_id: Option<String>,
    /// Omitted: keep the stored one.
    pub secret_access_key: Option<String>,
    /// Allow changing bucket or endpoint while files sit in the old one.
    #[serde(default)]
    pub forget_existing: bool,
}

fn default_path_style() -> bool {
    true
}

impl std::fmt::Debug for StorageInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageInput")
            .field("provider", &self.provider)
            .field("bucket", &self.bucket)
            .field("endpoint", &self.endpoint)
            .field("has_access_key_id", &self.access_key_id.is_some())
            .field("has_secret_access_key", &self.secret_access_key.is_some())
            .finish_non_exhaustive()
    }
}

/// The saved object store, with the secret opened, whether or not it is
/// where uploads go now; `None` when nothing usable is saved (including
/// sealed keys the current secret cannot open).
pub async fn saved(state: &AppState) -> Option<StorageConfig> {
    let vault = vault(state);
    let key_id = vault
        .open(&option_string(state, "storage_access_key_id").await)
        .ok()?;
    let secret = vault
        .open(&option_string(state, "storage_secret_access_key").await)
        .ok()?;
    let config = StorageConfig {
        provider: String::from("s3"),
        bucket: option_string(state, "storage_bucket").await,
        region: option_string(state, "storage_region").await,
        endpoint: option_string(state, "storage_endpoint").await,
        access_key_id: key_id,
        secret_access_key: Some(Secret::new(secret)),
        path_style: option_string(state, "storage_path_style").await != "false",
    };
    config.is_s3().then_some(config)
}

/// Applies the saved settings to the router at boot. The environment wins:
/// when it configures object storage the saved settings are left alone.
pub async fn apply_saved(state: &AppState) {
    if state
        .config
        .storage
        .as_ref()
        .is_some_and(StorageConfig::is_s3)
    {
        return;
    }
    if let Some(config) = saved(state).await {
        if let Some(backend) = S3Backend::new(&config) {
            state.media_storage.set_object(Some(Arc::new(backend)));
            if option_string(state, "storage_provider").await == "s3" {
                tracing::info!(bucket = %config.bucket, "media: object storage (admin settings)");
            } else {
                state.media_storage.set_active(MediaStorage::Local);
                tracing::info!(bucket = %config.bucket, "media: local disk; object storage kept for reading (admin settings)");
            }
        }
    }
}

fn hint(key_id: &str) -> String {
    let n = key_id.chars().count();
    if n < 4 {
        return String::new();
    }
    key_id.chars().skip(n - 4).collect()
}

async fn counts(state: &AppState) -> StorageCounts {
    let mut counts = StorageCounts::default();
    if let Ok(rows) = MediaRepo::new(state.pool.clone()).count_by_storage().await {
        for (kind, n) in rows {
            match kind {
                MediaStorage::Local => counts.local = n,
                MediaStorage::S3 => counts.s3 = n,
            }
        }
    }
    counts
}

/// The stored progress, if any.
pub async fn migration(state: &AppState) -> Option<MigrationProgress> {
    let value = state.options.get(MIGRATION_KEY).await.ok()?;
    serde_json::from_value(value).ok()
}

fn ephemeral_disk(state: &AppState) -> bool {
    state.config.run_dir.starts_with("/tmp") || state.config.media_dir.starts_with("/tmp")
}

/// What the admin page shows.
pub async fn settings(state: &AppState) -> StorageSettings {
    let encrypted = vault(state).encrypting();
    let counts = counts(state).await;
    let ephemeral_disk = ephemeral_disk(state);
    let migration = migration(state).await;
    if let Some(env) = state.config.storage.as_ref().filter(|s| s.is_s3()) {
        return StorageSettings {
            provider: String::from("s3"),
            bucket: env.bucket.clone(),
            region: env.region.clone(),
            endpoint: env.endpoint.clone(),
            path_style: env.path_style,
            access_key_id_hint: hint(&env.access_key_id),
            has_secret: env.secret_access_key.is_some(),
            keys_unreadable: false,
            source: Source::Environment,
            encrypted,
            counts,
            ephemeral_disk,
            migration,
        };
    }
    let provider = option_string(state, "storage_provider").await;
    if !option_string(state, "storage_bucket").await.is_empty() {
        let vault = vault(state);
        let key_id = vault
            .open(&option_string(state, "storage_access_key_id").await)
            .unwrap_or_default();
        let has_secret = vault
            .open(&option_string(state, "storage_secret_access_key").await)
            .is_ok_and(|s| !s.is_empty());
        let wants_s3 = provider == "s3";
        let active_s3 = state.media_storage.active_kind() == MediaStorage::S3;
        return StorageSettings {
            // What the router does, not what was asked for: with
            // unreadable keys uploads are on local disk whatever the row says.
            provider: String::from(if active_s3 { "s3" } else { "local" }),
            bucket: option_string(state, "storage_bucket").await,
            region: option_string(state, "storage_region").await,
            endpoint: option_string(state, "storage_endpoint").await,
            path_style: option_string(state, "storage_path_style").await != "false",
            access_key_id_hint: hint(&key_id),
            has_secret,
            keys_unreadable: wants_s3 && !active_s3,
            source: Source::Options,
            encrypted,
            counts,
            ephemeral_disk,
            migration,
        };
    }
    StorageSettings {
        provider: String::from("local"),
        bucket: String::new(),
        region: String::from("auto"),
        endpoint: String::new(),
        path_style: true,
        access_key_id_hint: String::new(),
        has_secret: false,
        keys_unreadable: false,
        source: Source::None,
        encrypted,
        counts,
        ephemeral_disk,
        migration,
    }
}

fn env_locked(state: &AppState) -> Result<(), AppError> {
    if state
        .config
        .storage
        .as_ref()
        .is_some_and(StorageConfig::is_s3)
    {
        return Err(AppError::conflict(
            "media storage is set by the operator in the environment (VYASA_STORAGE__*); change it there",
        ));
    }
    Ok(())
}

/// Builds the backend the input describes, filling omitted keys from the
/// saved ones.
async fn backend_from_input(
    state: &AppState,
    input: &StorageInput,
) -> Result<(StorageConfig, S3Backend), AppError> {
    let saved = saved(state).await;
    let bucket = input.bucket.trim().to_owned();
    let endpoint = input.endpoint.trim().trim_end_matches('/').to_owned();
    if bucket.is_empty() || bucket.contains('/') || bucket.contains(char::is_whitespace) {
        return Err(AppError::validation("bucket is a bucket name"));
    }
    if !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
        return Err(AppError::validation(
            "endpoint is the service URL, e.g. https://<account>.r2.cloudflarestorage.com",
        ));
    }
    let access_key_id = match input.access_key_id.as_deref().map(str::trim) {
        Some(k) if !k.is_empty() => k.to_owned(),
        _ => saved
            .as_ref()
            .map(|s| s.access_key_id.clone())
            .unwrap_or_default(),
    };
    let secret = match input.secret_access_key.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Some(Secret::new(s.to_owned())),
        _ => saved.and_then(|s| s.secret_access_key),
    };
    if access_key_id.is_empty() || secret.is_none() {
        return Err(AppError::validation(
            "access key id and secret access key are required",
        ));
    }
    let region = input.region.trim().to_owned();
    let config = StorageConfig {
        provider: String::from("s3"),
        bucket,
        region: if region.is_empty() {
            String::from("auto")
        } else {
            region
        },
        endpoint,
        access_key_id,
        secret_access_key: secret,
        path_style: input.path_style,
    };
    let backend = S3Backend::new(&config)
        .ok_or_else(|| AppError::validation("incomplete object storage settings"))?;
    Ok((config, backend))
}

/// Writes, reads back and deletes one probe object.
///
/// # Errors
/// [`AppError::Validation`] with the store's message.
pub async fn probe(backend: &dyn StorageBackend) -> Result<(), AppError> {
    let path = format!(".vyasa-storage-probe/{}", vyasa_common::next_id_i64());
    // The store refusing is the admin's settings being wrong, not an
    // upstream outage: a 400 with the store's words.
    let step = |what: &'static str, err: AppError| {
        AppError::validation(format!("object storage {what} failed: {err}"))
    };
    backend
        .put(&path, b"vyasa")
        .await
        .map_err(|e| step("write", e))?;
    let back = backend.get(&path).await.map_err(|e| step("read back", e))?;
    backend.delete(&path).await.map_err(|e| step("delete", e))?;
    if back != b"vyasa" {
        return Err(AppError::validation(
            "object storage read back different bytes",
        ));
    }
    Ok(())
}

/// Probes the store the input describes without saving anything.
///
/// # Errors
/// Validation of the input, or the store's refusal.
pub async fn test(state: &AppState, input: &StorageInput) -> Result<(), AppError> {
    if input.provider != "s3" {
        return Err(AppError::validation("nothing to test for local disk"));
    }
    let (_, backend) = backend_from_input(state, input).await?;
    probe(&backend).await
}

/// Saves the settings and switches the router.
///
/// # Errors
/// 409 when the environment owns the configuration or files sit in a
/// bucket that is about to be forgotten; 400 when the store refuses.
pub async fn save(state: &AppState, input: StorageInput) -> Result<StorageSettings, AppError> {
    env_locked(state)?;
    if migration(state).await.is_some_and(|m| m.state == "running") {
        return Err(AppError::conflict(
            "files are being moved; change the store when the move has finished",
        ));
    }
    match input.provider.as_str() {
        "local" => {
            // The saved object store is kept: files in it must stay
            // readable, and "Move existing files" can bring them back.
            put_option(state, "storage_provider", String::from("local")).await?;
            state.media_storage.set_active(MediaStorage::Local);
            tracing::info!("media: local disk for new uploads (admin settings)");
            Ok(settings(state).await)
        }
        "s3" => {
            let (config, backend) = backend_from_input(state, &input).await?;
            let previous = saved(state).await;
            let in_bucket = counts(state).await.s3;
            let location_changed = previous
                .as_ref()
                .is_some_and(|p| p.bucket != config.bucket || p.endpoint != config.endpoint);
            if location_changed && in_bucket > 0 && !input.forget_existing {
                return Err(AppError::conflict(format!(
                    "{in_bucket} files are in the current bucket; move them first, or confirm they should be forgotten"
                )));
            }
            probe(&backend).await?;
            let vault = vault(state);
            let secret = config.secret_access_key.as_ref().map_or("", Secret::expose);
            // Keys first, location last: a boot between two writes finds
            // either the old location with the old keys or the new with the new.
            put_option(
                state,
                "storage_access_key_id",
                vault.seal(&config.access_key_id)?,
            )
            .await?;
            put_option(state, "storage_secret_access_key", vault.seal(secret)?).await?;
            put_option(state, "storage_region", config.region.clone()).await?;
            put_option(state, "storage_path_style", config.path_style.to_string()).await?;
            put_option(state, "storage_endpoint", config.endpoint.clone()).await?;
            put_option(state, "storage_bucket", config.bucket.clone()).await?;
            put_option(state, "storage_provider", String::from("s3")).await?;
            state.media_storage.set_object(Some(Arc::new(backend)));
            tracing::info!(bucket = %config.bucket, "media: object storage (admin settings)");
            Ok(settings(state).await)
        }
        other => Err(AppError::validation(format!("unknown provider {other:?}"))),
    }
}

/// Starts the move job unless one is running.
///
/// # Errors
/// 409 while a run is in progress; validation when nothing needs moving.
pub async fn start_migration(state: &AppState) -> Result<MigrationProgress, AppError> {
    if let Some(current) = migration(state).await {
        if current.state == "running" {
            return Err(AppError::conflict("a move is already running"));
        }
    }
    let to = state.media_storage.active_kind();
    let from = other(to);
    if state.media_storage.object().is_none() {
        return Err(AppError::validation("no object storage is configured"));
    }
    let total = counts(state).await;
    let total = match from {
        MediaStorage::Local => total.local,
        MediaStorage::S3 => total.s3,
    };
    let progress = MigrationProgress {
        state: String::from("running"),
        total,
        done: 0,
        failed: 0,
        to,
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
        last_error: None,
    };
    write_progress(state, &progress).await?;
    vyasa_jobs::queue::enqueue(&state.pool, MIGRATE_KIND, serde_json::json!({}), None).await?;
    Ok(progress)
}

fn other(kind: MediaStorage) -> MediaStorage {
    match kind {
        MediaStorage::Local => MediaStorage::S3,
        MediaStorage::S3 => MediaStorage::Local,
    }
}

async fn write_progress(state: &AppState, progress: &MigrationProgress) -> Result<(), AppError> {
    state
        .options
        .set(
            MIGRATION_KEY,
            &serde_json::to_value(progress).map_err(|e| AppError::internal_msg(e.to_string()))?,
        )
        .await
}

/// Moves every row that sits in the store that is not active: original
/// and derivatives are copied to the destination, the row is updated, then
/// the source copies are removed. Nothing is deleted before the row says
/// the bytes are in their new place.
pub async fn run_migration(state: &AppState) -> Result<(), String> {
    let mut progress = migration(state).await.unwrap_or(MigrationProgress {
        state: String::from("running"),
        total: 0,
        done: 0,
        failed: 0,
        to: state.media_storage.active_kind(),
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
        last_error: None,
    });
    // The direction is the one the admin asked for, not whatever is
    // active now: a switch made while the job waited must not reverse it.
    let to = progress.to;
    let from = other(to);
    let outcome = run_migration_inner(state, &mut progress, from, to).await;
    progress.state = String::from(match &outcome {
        Ok(()) if progress.failed == 0 => "done",
        _ => "failed",
    });
    if let Err(err) = &outcome {
        progress.last_error = Some(err.clone());
    }
    progress.finished_at = Some(chrono::Utc::now().to_rfc3339());
    write_progress(state, &progress)
        .await
        .map_err(|e| e.to_string())?;
    outcome
}

async fn run_migration_inner(
    state: &AppState,
    progress: &mut MigrationProgress,
    from: MediaStorage,
    to: MediaStorage,
) -> Result<(), String> {
    let (Some(src), Some(dst)) = (
        state.media_storage.backend_for(from),
        state.media_storage.backend_for(to),
    ) else {
        return Err("no destination store".to_owned());
    };
    let repo = MediaRepo::new(state.pool.clone());
    let mut after_id = 0;
    loop {
        let rows = repo
            .list_by_storage(from, after_id, 50)
            .await
            .map_err(|e| e.to_string())?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after_id = row.id;
            let mut paths = vec![row.path.clone()];
            paths.extend(
                vyasa_core::media::derivative_paths(&row.derivatives)
                    .into_iter()
                    .map(str::to_owned),
            );
            match move_paths(src.as_ref(), dst.as_ref(), &paths).await {
                Ok(()) => match repo.update_storage(row.id, from, to).await {
                    Ok(true) => {
                        for path in &paths {
                            if let Err(err) = src.delete(path).await {
                                tracing::warn!(media_id = row.id, path, %err, "media move: source copy left behind");
                            }
                        }
                        progress.done += 1;
                    }
                    Ok(false) => {
                        // Deleted, replaced or moved meanwhile: the copy just
                        // written is an orphan, not a file anyone asked for.
                        for path in &paths {
                            let _ = dst.delete(path).await;
                        }
                        tracing::info!(
                            media_id = row.id,
                            "media move: row changed under the job, skipped"
                        );
                    }
                    Err(err) => {
                        progress.failed += 1;
                        progress.last_error = Some(format!("media {}: {err}", row.id));
                    }
                },
                Err(err) => {
                    tracing::warn!(media_id = row.id, %err, "media move failed");
                    progress.failed += 1;
                    progress.last_error = Some(format!("media {}: {err}", row.id));
                }
            }
            if (progress.done + progress.failed) % 10 == 0 {
                let _ = write_progress(state, progress).await;
            }
        }
    }
    Ok(())
}

async fn move_paths(
    src: &dyn StorageBackend,
    dst: &dyn StorageBackend,
    paths: &[String],
) -> Result<(), AppError> {
    for path in paths {
        let bytes = match src.get(path).await {
            Ok(bytes) => bytes,
            // A derivative that was never written: nothing to carry over.
            Err(AppError::NotFound { .. }) if Some(path) != paths.first() => continue,
            Err(err) => return Err(err),
        };
        dst.put(path, &bytes).await?;
    }
    Ok(())
}

/// The queue handler for `media_migrate`.
pub struct MigrateJobs(pub AppState);

impl vyasa_jobs::JobHandler for MigrateJobs {
    fn handle<'a>(
        &'a self,
        kind: &'a str,
        _payload: serde_json::Value,
    ) -> vyasa_jobs::HandlerFuture<'a> {
        Box::pin(async move {
            if kind != MIGRATE_KIND {
                return None;
            }
            Some(run_migration(&self.0).await)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::hint;

    #[test]
    fn the_hint_is_the_tail_of_the_key_id() {
        assert_eq!(hint("AKIAEXAMPLE1234"), "1234");
        assert_eq!(hint("abc"), "");
    }
}
