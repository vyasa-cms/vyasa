//! Plugin lifecycle REST (ManagePlugins capability).

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use ed25519_dalek::VerifyingKey;
use serde::Serialize;

use vyasa_common::{next_id_i64, AppError};

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::state::AppState;

/// Response shape for a plugin.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PluginResponse {
    /// Row id.
    pub id: i64,
    /// Package name.
    pub name: String,
    /// Active version.
    pub version: String,
    /// Enabled for dispatch.
    pub enabled: bool,
    /// Lifecycle status.
    pub status: String,
    /// Declared capabilities.
    pub capabilities: serde_json::Value,
}

fn response(row: vyasa_db::repo::PluginRow) -> PluginResponse {
    PluginResponse {
        id: row.id,
        name: row.name,
        version: row.version,
        enabled: row.enabled,
        status: row.status,
        capabilities: row.capabilities,
    }
}

/// `POST /api/v1/plugins` — install a signed `.vyplugin`.
///
/// Multipart fields: `file` (package), `trust_confirm` (`true` to accept
/// untrusted signatures; audited).
#[utoipa::path(
    post, path = "/api/v1/plugins", tag = "plugins",
    security(("session_cookie" = [])),
    request_body(content = crate::rest::upload_schema::FileUpload, description = "multipart/form-data with `file` (.vyplugin)", content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Installed", body = PluginResponse),
        (status = 400, description = "Invalid package", body = ApiErrorBody),
        (status = 403, description = "Forbidden / untrusted without confirm", body = ApiErrorBody),
    )
)]
pub async fn install(
    State(state): State<AppState>,
    multipart: Multipart,
) -> ApiResult<(StatusCode, Json<PluginResponse>)> {
    let bytes = package_bytes(multipart).await?;
    let keys = trusted_keys(&state);
    if keys.is_empty() {
        return Err(ApiError(AppError::validation(
            "no trusted signing keys are configured, so no package can be verified; \
             ask your operator to add the author's public key to package_trusted_keys \
             in vyasa.toml (or VYASA_PACKAGE_TRUSTED_KEYS)",
        )));
    }
    let parsed = match vyasa_plugins::package::parse_rpplugin(&bytes, &keys) {
        Ok(p) => p,
        Err(sig_fail) => {
            // Recorded, then refused: there is no override. An operator
            // who trusts the author adds their key and installs again.
            sqlx::query(
                "INSERT INTO plugin_audit (id, plugin_id, kind, capability, detail)
                 VALUES ($1, 0, 'deny', 'install:untrusted', $2)",
            )
            .bind(next_id_i64())
            .bind(sig_fail.to_string())
            .execute(&state.pool)
            .await
            .ok();
            let prefix = vyasa_plugins::package::inspect_rpplugin(&bytes, &keys)
                .map(|i| i.signature_prefix)
                .unwrap_or_default();
            return Err(ApiError(AppError::forbidden(format!(
                "this package is not signed by any trusted key (signature {prefix}…); \
                 ask your operator to add the author's public key to package_trusted_keys \
                 in vyasa.toml (or VYASA_PACKAGE_TRUSTED_KEYS), then install again"
            ))));
        }
    };

    let caps = serde_json::json!(parsed
        .capabilities
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>());
    let sha = vyasa_plugins::host::sha256_hex(&parsed.wasm);
    let row = vyasa_plugins::lifecycle::install(
        &state.plugins_repo,
        &parsed.manifest.name,
        &parsed.manifest.version,
        &parsed.wasm,
        &sha,
        &caps,
    )
    .await
    .map_err(ApiError)?;
    state
        .plugins_repo
        .set_metadata(
            row.id,
            &parsed.manifest.description,
            &parsed.manifest.author,
            &parsed.manifest.homepage,
            &parsed.manifest.license,
        )
        .await
        .ok();
    // A version that clashes with a content type arrives disabled; the
    // list says why.
    let _ = refresh(&state, row.id, crate::plugin_surface::Collect::Boot).await;
    // The row as it is now: an upgrade whose declarations clash with a
    // content type answers `degraded`, not the `loaded` it was installed as.
    let row = state
        .plugins_repo
        .list()
        .await
        .ok()
        .and_then(|rows| rows.into_iter().find(|p| p.id == row.id))
        .unwrap_or(row);
    Ok((StatusCode::CREATED, Json(response(row))))
}

/// Trusted verifying keys from config env (hex), plus optional test key.
/// Puts the running process back in step with the database after a
/// lifecycle change, without a restart.
///
/// The linked component and the capability grants were also cached for
/// the life of the process, so an install, enable, disable or rollback
/// kept the old behaviour until one.
///
/// Everything the plugin contributed is withdrawn first, then — if it is
/// still enabled — collected again from scratch. Withdrawing alone was
/// what the old version did: disabling took effect at once while enabling
/// recorded a flag and changed nothing until the next boot, and a disabled
/// plugin stayed registered on every hook, so each page render called it,
/// failed to load it, and marked it degraded. Turning a plugin off made
/// the admin report that it had crashed.
///
/// Returns the refusal when the plugin, still enabled, declares a post
/// type or taxonomy an administrator's content type holds: it is then
/// turned off (and marked errored with the reason), and nothing it
/// declares stays registered.
///
/// `mode` says how a declaration clashing with an administrator's content
/// type is met: [`Collect::Enable`] (turning a plugin on, rolling it back)
/// refuses the plugin; [`Collect::Boot`] (refreshing one already on: a
/// settings save, an upgrade) skips the declaration and marks the plugin
/// degraded, as a restart would — saving a setting never turns a plugin
/// off.
pub(crate) async fn refresh(
    state: &AppState,
    plugin_id: i64,
    mode: crate::plugin_surface::Collect,
) -> Option<AppError> {
    state.plugin_host.evict(plugin_id).await;
    state.broker.evict(plugin_id).await;
    // Its declarations go too: block kinds it can no longer render, routes
    // that would answer 502, assets linked from a page it no longer
    // styles, and its place in the hook order. Its post types keep their
    // place in the registry until it has been collected again, so no
    // content type can take one of their slugs in between.
    let kept_types = state.plugin_surface.forget_for_refresh(plugin_id).await;
    state.plugin_blocks.forget(plugin_id).await;
    state.hook_registry.forget(plugin_id).await;

    // Still enabled? Then it was an install, an upgrade or a rollback, and
    // the new version's declarations replace what was just withdrawn.
    let still_on = state
        .plugins_repo
        .list()
        .await
        .ok()
        .and_then(|rows| rows.into_iter().find(|p| p.id == plugin_id))
        .filter(|row| row.enabled);
    let mut refused = None;
    if let Some(row) = still_on {
        if let Err(e) = crate::plugins_boot::activate(state, &row, mode).await {
            state.plugins_repo.set_enabled(plugin_id, false).await.ok();
            refused = Some(e);
        }
    }
    state.plugin_surface.release_stale_types(&kept_types).await;

    // Rendered pages carry a plugin's block output and its asset links, so
    // a lifecycle change makes every cached page stale at once. Bumping
    // the epoch changes every key rather than emptying the cache: entries
    // for the old epoch are unreachable and age out on their own, and a
    // site that toggles a plugin no longer re-renders from cold.
    state
        .surface_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    refused
}

/// Keys a hand-uploaded plugin may be signed by: the operator's
/// `package_trusted_keys`. Marketplace installs check the listing's
/// author key instead (`registry::install`).
pub(crate) fn trusted_keys(state: &AppState) -> Vec<VerifyingKey> {
    crate::signing::parse_keys(&state.config.package_trusted_keys)
}

/// Reads the package out of a multipart upload.
async fn package_bytes(mut multipart: Multipart) -> Result<Vec<u8>, ApiError> {
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError(AppError::validation(format!("multipart: {e}"))))?
    {
        if field.name() == Some("file") {
            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError(AppError::validation(format!("read: {e}"))))?;
            bytes = Some(data.to_vec());
        }
    }
    bytes.ok_or_else(|| ApiError(AppError::validation("missing `file` field")))
}

/// What inspect returns: enough to decide.
#[derive(Serialize, utoipa::ToSchema)]
pub struct Inspection {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub homepage: String,
    pub license: String,
    pub capabilities: Vec<String>,
    /// A trusted key vouches for the signature.
    pub trusted: bool,
    /// The first characters of the signature, to talk about it.
    pub signature_prefix: String,
    pub wasm_bytes: usize,
    /// Already installed at this version, if any.
    pub installed_version: Option<String>,
    /// Capabilities the installed version does not have.
    pub new_capabilities: Vec<String>,
}

/// `POST /api/v1/plugins/inspect` — parse a package and report what it
/// is and whether it is trusted, without installing anything.
#[utoipa::path(
    post, path = "/api/v1/plugins/inspect", tag = "plugins",
    security(("session_cookie" = [])),
    request_body(content = crate::rest::upload_schema::FileUpload, description = "multipart/form-data with `file` (.vyplugin)", content_type = "multipart/form-data"),
    responses((status = 200, description = "What the package is", body = Inspection),
              (status = 400, description = "Malformed package", body = ApiErrorBody))
)]
pub async fn inspect(
    State(state): State<AppState>,
    multipart: Multipart,
) -> ApiResult<Json<Inspection>> {
    let bytes = package_bytes(multipart).await?;
    let keys = trusted_keys(&state);
    let seen = vyasa_plugins::package::inspect_rpplugin(&bytes, &keys).map_err(ApiError)?;
    let caps: Vec<String> = seen.capabilities.iter().map(ToString::to_string).collect();
    let existing = state
        .plugins_repo
        .list()
        .await
        .ok()
        .and_then(|rows| rows.into_iter().find(|p| p.name == seen.manifest.name));
    let had: std::collections::HashSet<String> = existing
        .as_ref()
        .and_then(|p| p.capabilities.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    Ok(Json(Inspection {
        name: seen.manifest.name.clone(),
        version: seen.manifest.version.clone(),
        description: seen.manifest.description.clone(),
        author: seen.manifest.author.clone(),
        homepage: seen.manifest.homepage.clone(),
        license: seen.manifest.license.clone(),
        new_capabilities: caps.iter().filter(|c| !had.contains(*c)).cloned().collect(),
        capabilities: caps,
        trusted: seen.trusted,
        signature_prefix: seen.signature_prefix,
        wasm_bytes: seen.wasm_bytes,
        installed_version: existing.map(|p| p.version),
    }))
}

/// `GET /api/v1/plugins/{id}/audit` — the newest denials, fetches and
/// quota hits.
#[utoipa::path(
    get, path = "/api/v1/plugins/{id}/audit", tag = "plugins",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Audit rows", body = serde_json::Value))
)]
pub async fn audit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = state
        .plugins_repo
        .audit_tail(id, 50)
        .await
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!(rows)))
}

/// `GET /api/v1/plugins` — list with capabilities + audit summary.
#[utoipa::path(
    get, path = "/api/v1/plugins", tag = "plugins",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Installed plugins", body = [serde_json::Value]))
)]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let rows = state.plugins_repo.list().await.map_err(ApiError)?;
    let mut out = Vec::new();
    for row in rows {
        let audit = state
            .plugins_repo
            .audit_summary(&state.pool, row.id)
            .await
            .unwrap_or_default();
        let versions = state
            .plugins_repo
            .versions(row.id)
            .await
            .unwrap_or_default();
        out.push(serde_json::json!({
            "id": row.id, "name": row.name, "version": row.version,
            "enabled": row.enabled, "status": row.status,
            "status_reason": row.status_reason,
            "capabilities": row.capabilities,
            "description": row.description, "author": row.author,
            "homepage": row.homepage, "license": row.license,
            "versions": versions,
            "audit": audit.into_iter().collect::<std::collections::BTreeMap<_,_>>(),
        }));
    }
    Ok(Json(serde_json::Value::Array(out)))
}

/// `POST /api/v1/plugins/{id}/enable|disable`.
macro_rules! toggle {
    ($name:ident, $flag:expr) => {
        pub async fn $name(
            State(state): State<AppState>,
            principal: Principal,
            Path(id): Path<i64>,
        ) -> ApiResult<Json<serde_json::Value>> {
            state.plugins_repo.set_enabled(id, $flag).await.map_err(ApiError)?;
            state.plugins_repo.set_status(id, if $flag { "loaded" } else { "disabled" }).await.ok();
            // An enable refused because the plugin declares an
            // administrator's content type leaves it off, with nothing
            // registered, and says why.
            if let Some(refused) = refresh(&state, id, crate::plugin_surface::Collect::Enable).await {
                return Err(ApiError(refused));
            }
            crate::audit::record(
                &state,
                principal.user(),
                if $flag { "plugin.enable" } else { "plugin.disable" },
                format!("plugin:{id}"),
                serde_json::json!({}),
            );
            Ok(Json(serde_json::json!({"id": id, "enabled": $flag})))
        }
    };
}
toggle!(enable, true);
toggle!(disable, false);

/// `POST /api/v1/plugins/{id}/rollback` — activate previous version.
#[utoipa::path(
    post, path = "/api/v1/plugins/{id}/rollback", tag = "plugins",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Rolled back", body = serde_json::Value))
)]
pub async fn rollback(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<serde_json::Value>>,
) -> ApiResult<Json<serde_json::Value>> {
    let wanted = body
        .as_ref()
        .and_then(|b| b.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let version = match wanted {
        Some(v) => {
            let known = state.plugins_repo.versions(id).await.map_err(ApiError)?;
            if !known.iter().any(|k| k == &v) {
                return Err(ApiError(AppError::not_found("plugin version", v)));
            }
            state
                .plugins_repo
                .set_version(id, &v)
                .await
                .map_err(ApiError)?;
            v
        }
        None => vyasa_plugins::lifecycle::rollback(&state.plugins_repo, id)
            .await
            .map_err(ApiError)?,
    };
    if let Some(refused) = refresh(&state, id, crate::plugin_surface::Collect::Enable).await {
        return Err(ApiError(refused));
    }
    Ok(Json(serde_json::json!({"id": id, "version": version})))
}

/// `DELETE /api/v1/plugins/{id}` — uninstall (cascades versions + settings).
#[utoipa::path(
    delete, path = "/api/v1/plugins/{id}", tag = "plugins",
    security(("session_cookie" = [])),
    responses((status = 204, description = "Uninstalled"))
)]
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    state
        .plugins_repo
        .delete_plugin(id)
        .await
        .map_err(ApiError)?;
    let _ = refresh(&state, id, crate::plugin_surface::Collect::Boot).await;
    crate::audit::record(
        &state,
        principal.user(),
        "plugin.delete",
        format!("plugin:{id}"),
        serde_json::json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/plugins/{id}/settings/{key}` — set one setting.
#[utoipa::path(
    put, path = "/api/v1/plugins/{id}/settings/{key}", tag = "plugins",
    security(("session_cookie" = [])),
    responses((status = 204, description = "Stored"))
)]
pub async fn setting_put(
    State(state): State<AppState>,
    Path((id, key)): Path<(i64, String)>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<StatusCode> {
    state
        .plugins_repo
        .setting_put(id, &key, &value)
        .await
        .map_err(ApiError)?;
    // A plugin reads its settings once, at `init`, so a saved setting did
    // nothing until the next restart. Re-activating hands it the new
    // configuration now.
    let _ = refresh(&state, id, crate::plugin_surface::Collect::Boot).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/plugins/{id}/settings/{key}` — read one setting.
#[utoipa::path(
    get, path = "/api/v1/plugins/{id}/settings/{key}", tag = "plugins",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Setting value", body = serde_json::Value))
)]
pub async fn setting_get(
    State(state): State<AppState>,
    Path((id, key)): Path<(i64, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let value = state
        .plugins_repo
        .setting_get(id, &key)
        .await?
        .unwrap_or(serde_json::Value::Null);
    Ok(Json(value))
}

/// `GET /api/v1/plugins/surface` — everything plugins contribute to the
/// admin: block kinds for the inserter, settings forms to render, custom
/// post types, declared routes and the state of scheduled tasks.
///
/// One endpoint rather than five: the admin needs all of it to draw a
/// plugin's page, and each part is a handful of rows.
#[utoipa::path(
    get, path = "/api/v1/plugins/surface", tag = "plugins",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Contributed surface", body = serde_json::Value))
)]
pub async fn surface(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let blocks: Vec<serde_json::Value> = state
        .plugin_blocks
        .all()
        .await
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "kind": d.kind, "title": d.title, "icon": d.icon,
                "pluginId": d.plugin_id.to_string(), "pluginName": d.plugin_name,
            })
        })
        .collect();
    let routes: Vec<serde_json::Value> = state
        .plugin_surface
        .routes()
        .await
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "method": r.method,
                "path": format!("/api/v1/plugin/{}/{}", r.plugin_name, r.path),
                "pluginId": r.plugin_id.to_string(),
            })
        })
        .collect();
    let forms: Vec<serde_json::Value> = state
        .plugin_surface
        .admin_forms()
        .await
        .into_iter()
        .map(|f| {
            serde_json::json!({
                "pluginId": f.plugin_id.to_string(), "pluginName": f.plugin_name,
                "title": f.title, "description": f.description, "fields": f.fields,
            })
        })
        .collect();
    let post_types: Vec<serde_json::Value> = state
        .plugin_surface
        .post_types()
        .await
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "slug": t.slug, "singular": t.singular, "plural": t.plural,
                "public": t.public, "hasArchive": t.has_archive,
                "pluginId": t.plugin_id.to_string(),
            })
        })
        .collect();
    let taxonomies: Vec<serde_json::Value> = state
        .plugin_surface
        .taxonomies()
        .await
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "slug": t.slug, "singular": t.singular, "plural": t.plural,
                "hierarchical": t.hierarchical, "public": t.public,
                "pluginId": t.plugin_id.to_string(),
            })
        })
        .collect();
    let tasks = task_status(&state).await;
    Ok(Json(serde_json::json!({
        "blocks": blocks, "routes": routes, "forms": forms,
        "postTypes": post_types, "taxonomies": taxonomies, "tasks": tasks,
        // The vocabulary itself, so the admin can show an author what a
        // plugin is able to hook without them reading the docs.
        "filterPoints": crate::plugin_hooks::FILTER_POINTS,
        "eventNames": crate::plugin_hooks::EVENT_NAMES,
    })))
}

/// Declared tasks joined with what actually happened when they last ran.
/// One `plugin_tasks` row as the query returns it.
type TaskRow = (
    i64,
    String,
    i32,
    Option<chrono::DateTime<chrono::Utc>>,
    String,
    Option<String>,
);

async fn task_status(state: &AppState) -> Vec<serde_json::Value> {
    let rows: Vec<TaskRow> = sqlx::query_as(
        "SELECT plugin_id, name, every_seconds, last_run_at, last_status, last_error
         FROM plugin_tasks ORDER BY plugin_id, name",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(plugin_id, name, every, last_run, status, error)| {
            serde_json::json!({
                "pluginId": plugin_id.to_string(), "name": name,
                "everySeconds": every,
                "lastRunAt": last_run.map(|t| t.to_rfc3339()),
                "lastStatus": status, "lastError": error,
            })
        })
        .collect()
}

/// `GET /api/v1/plugins/settings/{id}` — every setting for one plugin.
///
/// The per-key endpoints stay for scripts; a settings form needs the whole
/// object to render, and asking for it one key at a time would make the
/// number of requests depend on how many fields a plugin declares.
#[utoipa::path(
    get, path = "/api/v1/plugins/settings/{id}", tag = "plugins",
    security(("session_cookie" = [])),
    responses((status = 200, description = "All settings", body = serde_json::Value))
)]
pub async fn settings_all(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let pairs = state.plugins_repo.settings(id).await.map_err(ApiError)?;
    Ok(Json(serde_json::Value::Object(pairs.into_iter().collect())))
}
