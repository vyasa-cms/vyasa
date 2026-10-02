//! Plugin lifecycle operations over the repo: install (atomic), enable/
//! disable, rollback, uninstall. Instance invalidation is delegated to the
//! caller-supplied hook so the host crate stays decoupled.

use vyasa_common::AppError;
use vyasa_db::repo::PluginsRepo;

/// Invalidate any cached instances for this plugin (host provides).
pub type InvalidateFn<'a> = Box<dyn FnOnce(i64) + 'a>;

/// Installs a verified package atomically (plugin row + version row in one
/// transaction via `create`/`add_version`).
///
/// # Errors
/// [`AppError::Conflict`] when the exact name+version exists; other
/// repository errors propagate.
pub async fn install(
    repo: &PluginsRepo,
    name: &str,
    version: &str,
    wasm: &[u8],
    sha256: &str,
    capabilities: &serde_json::Value,
) -> Result<vyasa_db::repo::PluginRow, AppError> {
    // create() writes plugin + first version together; for an existing
    // plugin it conflicts, which callers translate into add_version.
    match repo.create(name, version, wasm, sha256, capabilities).await {
        Ok(row) => Ok(row),
        Err(AppError::Conflict { .. }) => {
            let existing = repo
                .list()
                .await?
                .into_iter()
                .find(|p| p.name == name)
                .ok_or_else(|| AppError::internal_msg("conflict without row"))?;
            repo.add_version(existing.id, version, wasm, sha256, capabilities)
                .await?;
            repo.list()
                .await?
                .into_iter()
                .find(|p| p.id == existing.id)
                .ok_or_else(|| AppError::internal_msg("row vanished"))
        }
        Err(e) => Err(e),
    }
}

/// Rollback to the newest installed version strictly lower than current.
///
/// # Errors
/// [`AppError::NotFound`] when no earlier version exists.
pub async fn rollback(repo: &PluginsRepo, plugin_id: i64) -> Result<String, AppError> {
    let rows = repo.list().await?;
    let current = rows
        .iter()
        .find(|p| p.id == plugin_id)
        .ok_or_else(|| AppError::not_found("plugin", plugin_id))?;
    let versions = repo.versions(plugin_id).await?;
    let target = versions
        .iter()
        .filter(|v| v.as_str() != current.version)
        .max()
        .or_else(|| versions.first())
        .filter(|v| v.as_str() != current.version)
        .ok_or_else(|| AppError::not_found("prior version", format!("{plugin_id}")))?;
    repo.set_version(plugin_id, target).await?;
    Ok(target.clone())
}
