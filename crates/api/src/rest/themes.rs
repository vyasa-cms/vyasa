//! Theme management endpoints: install, list, activate, rollback.
//!
//! Packages are `.vytheme` zips (see `vyasa_themes::package`); all
//! routes require the `ManageThemes` capability. Activation bumps the
//! render-cache fully (theme_version keys make stale entries unreachable).

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use sha2::Digest as _;

use vyasa_common::AppError;
use vyasa_db::repo::themes::ThemeRow;

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::state::AppState;

/// Maximum accepted upload size (mirrors package cap).
const MAX_UPLOAD_BYTES: usize = vyasa_themes::package::MAX_PACKAGE_BYTES;

/// Public shape of a stored theme version.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ThemeResponse {
    /// Row id.
    pub id: i64,
    /// Package name.
    pub name: String,
    /// Integer version.
    pub version: i64,
    /// Whether this version is active.
    pub is_active: bool,
    /// The package manifest's version this row came from, when it did.
    pub package_version: Option<i64>,
}

impl From<ThemeRow> for ThemeResponse {
    fn from(row: ThemeRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            version: i64::from(row.version),
            is_active: row.is_active,
            package_version: row.package_version.map(i64::from),
        }
    }
}

/// `POST /api/v1/themes` — install a `.vytheme` package (multipart field
/// `file`). Does **not** activate; follow up with `/themes/{id}/activate`.
///
/// 400 on a rejected package (author-friendly message), 403 without
/// `ManageThemes`, 409 when name+version already installed.
#[utoipa::path(
    post, path = "/api/v1/themes",
    tag = "themes",
    security(("session_cookie" = [])),
    request_body(content = crate::rest::upload_schema::FileUpload, description = "multipart/form-data with `file` (.vytheme) and, for a theme carrying script, `signature` (hex ed25519 over the file)", content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Installed", body = ThemeResponse),
        (status = 400, description = "Invalid package", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 409, description = "Version already installed", body = ApiErrorBody),
    )
)]
pub async fn install(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<ThemeResponse>)> {
    let mut bytes: Option<Vec<u8>> = None;
    let mut signature: Option<String> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| ApiError(AppError::validation(format!("multipart error: {err}"))))?
    {
        if field.name() == Some("signature") {
            signature =
                Some(field.text().await.map_err(|err| {
                    ApiError(AppError::validation(format!("read failed: {err}")))
                })?);
            continue;
        }
        if field.name() == Some("file") {
            let data = field
                .bytes()
                .await
                .map_err(|err| ApiError(AppError::validation(format!("read failed: {err}"))))?;
            if data.len() > MAX_UPLOAD_BYTES {
                return Err(ApiError(AppError::validation(format!(
                    "package too large: {} bytes (max {MAX_UPLOAD_BYTES})",
                    data.len()
                ))));
            }
            bytes = Some(data.to_vec());
        }
    }
    let Some(bytes) = bytes else {
        return Err(ApiError(AppError::validation(
            "missing multipart field `file` with a .vytheme package",
        )));
    };

    // Scripted themes need a trusted signature, exactly as from the
    // marketplace; the operator's own keys count here too.
    let mut keys: Vec<String> = crate::official::MARKETPLACE_KEYS
        .iter()
        .map(|k| (*k).to_owned())
        .collect();
    keys.extend(state.config.package_trusted_keys.iter().cloned());
    crate::signing::verify_theme_upload(&bytes, signature.as_deref(), &keys).map_err(ApiError)?;

    let parsed = vyasa_themes::package::parse_vytheme(&bytes)
        .map_err(|e| ApiError(AppError::validation(e.to_string())))?;

    let templates_json =
        if parsed.templates.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&parsed.templates).map_err(|e| {
                ApiError(AppError::internal_msg(format!("serialize templates: {e}")))
            })?)
        };
    let package_version = i32::try_from(parsed.manifest.version).unwrap_or(i32::MAX);
    if state
        .themes
        .has_package_version(&parsed.manifest.name, package_version)
        .await?
    {
        return Err(ApiError(AppError::conflict(format!(
            "theme {} v{package_version} already installed",
            parsed.manifest.name
        ))));
    }
    // The row's `version` is the site's own sequence, shared with studio
    // publishes; the package's number is kept beside it.
    let version = state.themes.latest_version(&parsed.manifest.name).await? + 1;
    let row = state
        .themes
        .insert_version(
            &parsed.manifest.name,
            version,
            parsed.tokens_json,
            parsed.layout_json,
            templates_json,
            parsed.assets_json,
            None,
        )
        .await?;
    state
        .themes
        .set_package_version(row.id, package_version)
        .await?;
    crate::theme_assets::store_files(&state, row.id, &parsed.files).await?;
    let mut row = row;
    row.package_version = Some(package_version);
    Ok((StatusCode::CREATED, Json(ThemeResponse::from(row))))
}

/// One bundled file of a theme version, as listed.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ThemeFileResponse {
    /// Path under `assets/`, e.g. `images/hero.jpg`.
    pub path: String,
    /// MIME type the file is served with.
    pub content_type: String,
    /// Hex SHA-256 of the bytes; the ETag it serves under.
    pub sha256: String,
    /// Size in bytes.
    pub size: i64,
    /// Where the site serves it while this version is active.
    pub url: String,
}

/// `GET /api/v1/themes/{id}/files` — the pictures and fonts this version
/// bundles.
#[utoipa::path(
    get, path = "/api/v1/themes/{id}/files",
    tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Theme row id")),
    responses(
        (status = 200, description = "Bundled files", body = [ThemeFileResponse]),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Unknown theme id", body = ApiErrorBody),
    )
)]
pub async fn files(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<ThemeFileResponse>>> {
    state.themes.get(id).await?;
    let list = state
        .themes
        .files_meta(id)
        .await?
        .into_iter()
        .map(|f| ThemeFileResponse {
            url: format!("/theme-assets/{}", f.path),
            path: f.path,
            content_type: f.content_type,
            sha256: f.sha256,
            size: i64::from(f.size),
        })
        .collect();
    Ok(Json(list))
}

/// `PUT /api/v1/themes/{id}/files/{path}` — add or replace one bundled
/// file. The body is the raw bytes; the path decides the type under the
/// same rules a package is held to, so what an author uploads here is
/// exactly what a package could have carried.
///
/// 400 for a path or type a package would refuse, 413 past the per-file
/// or per-theme caps.
#[utoipa::path(
    put, path = "/api/v1/themes/{id}/files/{path}",
    tag = "themes",
    security(("session_cookie" = [])),
    request_body(content = crate::rest::upload_schema::RawBytes, content_type = "application/octet-stream"),
    params(
        ("id" = i64, Path, description = "Theme row id"),
        ("path" = String, Path, description = "Path under assets/, e.g. images/hero.jpg"),
    ),
    responses(
        (status = 200, description = "Stored", body = ThemeFileResponse),
        (status = 400, description = "Not a file a theme may bundle", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Unknown theme id", body = ApiErrorBody),
        (status = 413, description = "Too large", body = ApiErrorBody),
    )
)]
pub async fn put_file(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, path)): Path<(i64, String)>,
    body: axum::body::Bytes,
) -> ApiResult<Json<ThemeFileResponse>> {
    use vyasa_themes::package as pkg;
    let row = state.themes.get(id).await?;
    if !pkg::valid_bundled_path(&path) {
        return Err(ApiError(AppError::validation(
            "the path must be images/… or fonts/…, lowercase letters, digits, dots, \
             dashes and underscores, at most one directory deep",
        )));
    }
    let Some(content_type) = pkg::bundled_content_type(&path) else {
        return Err(ApiError(AppError::validation(
            "images may be png, jpg, gif, webp, avif or svg; fonts may be woff2, woff, ttf or otf",
        )));
    };
    if body.is_empty() {
        return Err(ApiError(AppError::validation("the file is empty")));
    }
    if body.len() > pkg::MAX_ENTRY_BYTES {
        return Err(ApiError(AppError::too_large(format!(
            "the file is {} bytes; the maximum is {}",
            body.len(),
            pkg::MAX_ENTRY_BYTES
        ))));
    }
    if content_type == "image/svg+xml" && !pkg::looks_like_svg(&body) {
        return Err(ApiError(AppError::validation(
            "that does not look like an SVG document",
        )));
    }
    // One listing answers both caps; the bytes themselves stay in the
    // database.
    let listed = state.themes.files_meta(id).await?;
    let existing = listed.iter().find(|f| f.path == path);
    let total: usize = listed
        .iter()
        .map(|f| usize::try_from(f.size).unwrap_or(0))
        .sum();
    let replaced = existing.map_or(0, |f| usize::try_from(f.size).unwrap_or(0));
    if total.saturating_sub(replaced).saturating_add(body.len()) > pkg::MAX_FILES_TOTAL_BYTES {
        return Err(ApiError(AppError::too_large(format!(
            "this theme's files would pass {} bytes in total",
            pkg::MAX_FILES_TOTAL_BYTES
        ))));
    }
    if existing.is_none() && listed.len() >= pkg::MAX_FILES {
        return Err(ApiError(AppError::validation(format!(
            "a theme bundles at most {} files",
            pkg::MAX_FILES
        ))));
    }
    let sha256 = hex::encode(sha2::Sha256::digest(&body));
    state
        .themes
        .upsert_file(
            id,
            &vyasa_db::repo::ThemeFileInput {
                path: path.clone(),
                content_type: content_type.to_owned(),
                sha256: sha256.clone(),
                bytes: body.to_vec(),
            },
        )
        .await?;
    crate::audit::record(
        &state,
        principal.user(),
        "theme.file.put",
        format!("theme:{}", row.name),
        serde_json::json!({ "version": row.version, "path": path, "bytes": body.len() }),
    );
    Ok(Json(ThemeFileResponse {
        url: format!("/theme-assets/{path}"),
        path,
        content_type: content_type.to_owned(),
        sha256,
        size: i64::try_from(body.len()).unwrap_or(i64::MAX),
    }))
}

/// `DELETE /api/v1/themes/{id}/files/{path}` — remove one bundled file.
#[utoipa::path(
    delete, path = "/api/v1/themes/{id}/files/{path}",
    tag = "themes",
    security(("session_cookie" = [])),
    params(
        ("id" = i64, Path, description = "Theme row id"),
        ("path" = String, Path, description = "Path under assets/"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Unknown theme id or file", body = ApiErrorBody),
    )
)]
pub async fn delete_file(
    State(state): State<AppState>,
    principal: Principal,
    Path((id, path)): Path<(i64, String)>,
) -> ApiResult<StatusCode> {
    let row = state.themes.get(id).await?;
    if !state.themes.delete_file(id, &path).await? {
        return Err(ApiError(AppError::not_found(
            "theme_file",
            format!("{} v{} {path}", row.name, row.version),
        )));
    }
    crate::audit::record(
        &state,
        principal.user(),
        "theme.file.delete",
        format!("theme:{}", row.name),
        serde_json::json!({ "version": row.version, "path": path }),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/themes` — list installed versions.
#[utoipa::path(
    get, path = "/api/v1/themes",
    tag = "themes",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Installed themes", body = [ThemeResponse]),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<ThemeResponse>>> {
    let rows = state.themes.list().await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// `GET /api/v1/themes/{id}/tokens` — the stored design tokens for one
/// version.
///
/// The admin theme studio needs the token set to show colour roles,
/// typography and spacing; `list` deliberately stays lightweight, so the
/// tokens live behind their own route.
#[utoipa::path(
    get, path = "/api/v1/themes/{id}/tokens",
    tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Theme row id")),
    responses(
        (status = 200, description = "Design tokens", body = serde_json::Value),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Unknown theme id", body = ApiErrorBody),
    )
)]
pub async fn tokens(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = state.themes.list().await?;
    let row = rows
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| AppError::not_found("theme", id))?;
    Ok(Json(serde_json::json!({
        "id": row.id,
        "name": row.name,
        "version": row.version,
        "is_active": row.is_active,
        "tokens": row.tokens,
        "layout": row.layout,
    })))
}

fn purge_theme_cache(state: &AppState) {
    // Theme identity changed: every cached page may render differently.
    if let Some(cache) = &state.render_cache {
        cache.invalidate_theme();
    }
}

/// What every activation path owes the rest of the system: drop cached
/// pages and tell webhook subscribers. Used by the activate and rollback
/// routes and by the studio's publish-and-activate.
pub fn after_activate(state: &AppState, name: &str, version: i32) {
    purge_theme_cache(state);
    announce_theme_change(state, name, &version.to_string());
}

/// `DELETE /api/v1/themes/{id}` — remove an installed version. The live
/// theme cannot be deleted; activate another one first.
#[utoipa::path(
    delete, path = "/api/v1/themes/{id}",
    tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Theme row id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Unknown theme id", body = ApiErrorBody),
        (status = 409, description = "Theme is live", body = ApiErrorBody),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let row = state.themes.get(id).await?;
    state.themes.delete(id).await?;
    crate::audit::record(
        &state,
        principal.user(),
        "theme.delete",
        format!("theme:{}", row.name),
        serde_json::json!({ "version": row.version }),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/themes/{id}/activate` — make this version live.
#[utoipa::path(
    post, path = "/api/v1/themes/{id}/activate",
    tag = "themes",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Theme row id")),
    responses(
        (status = 200, description = "Activated", body = ThemeResponse),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Unknown theme id", body = ApiErrorBody),
    )
)]
pub async fn activate(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<ThemeResponse>> {
    let rows = state.themes.list().await?;
    let row = rows
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError(AppError::not_found("theme", id)))?;
    state.themes.set_active(&row.name, row.version).await?;
    after_activate(&state, &row.name, row.version);
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::THEME_ACTIVATED,
        serde_json::json!({ "theme_id": id, "name": row.name }),
    );
    crate::audit::record(
        &state,
        principal.user(),
        "theme.activate",
        format!("theme:{}", row.name),
        serde_json::json!({ "version": row.version }),
    );
    let mut out = row.clone();
    out.is_active = true;
    Ok(Json(ThemeResponse::from(out)))
}

/// Notifies webhook subscribers that the active theme changed.
///
/// Best-effort and non-blocking: a webhook problem must not fail the
/// activation the admin just asked for.
fn announce_theme_change(state: &AppState, name: &str, version: &str) {
    let state = state.clone();
    let data = serde_json::json!({ "theme": name, "version": version });
    tokio::spawn(async move {
        if let Err(err) = crate::webhook_dispatcher::deliver_now(
            &state,
            vyasa_core::webhooks::service::WebhookEvent::ThemeChanged,
            data,
        )
        .await
        {
            tracing::warn!("theme.changed webhook fan-out failed: {err}");
        }
    });
}

/// `POST /api/v1/themes/{name}/rollback` — activate the newest installed
/// version of `name` that is older than the currently active one.
#[utoipa::path(
    post, path = "/api/v1/themes/{name}/rollback",
    tag = "themes",
    security(("session_cookie" = [])),
    params(("name" = String, Path, description = "Theme package name")),
    responses(
        (status = 200, description = "Rolled back", body = ThemeResponse),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "No earlier version", body = ApiErrorBody),
    )
)]
pub async fn rollback(
    State(state): State<AppState>,
    principal: Principal,
    Path(name): Path<String>,
) -> ApiResult<Json<ThemeResponse>> {
    let current = state
        .themes
        .get_active()
        .await?
        .ok_or_else(|| ApiError(AppError::conflict("no active theme to roll back from")))?;
    if current.name != name {
        return Err(ApiError(AppError::validation(format!(
            "active theme is \"{}\", not \"{name}\"",
            current.name
        ))));
    }
    let target = state
        .themes
        .latest_below(&name, current.version)
        .await
        .map_err(ApiError)?;
    state
        .themes
        .set_active(&target.name, target.version)
        .await?;
    after_activate(&state, &target.name, target.version);
    crate::audit::record(
        &state,
        principal.user(),
        "theme.rollback",
        format!("theme:{}", target.name),
        serde_json::json!({ "version": target.version }),
    );
    Ok(Json(ThemeResponse::from(target)))
}
