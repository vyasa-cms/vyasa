//! Media library endpoints.

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use vyasa_common::AppError;
use vyasa_core::media::MediaService;
use vyasa_db::content_models::MediaRow;

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::policy;
use crate::state::AppState;

/// Response shape for a media item.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MediaResponse {
    /// Media id.
    pub id: i64,
    /// Owner user id.
    pub owner_id: i64,
    /// Original file name.
    pub file_name: String,
    /// MIME type.
    pub mime: String,
    /// Size in bytes.
    pub byte_size: i64,
    /// Storage backend.
    pub storage: String,
    /// Storage-relative path.
    pub path: String,
    /// Pixel width.
    pub width: Option<i32>,
    /// Pixel height.
    pub height: Option<i32>,
    /// Blurhash.
    pub blurhash: Option<String>,
    /// Alt text.
    pub alt: Option<String>,
    /// Caption.
    pub caption: Option<String>,
    /// Derivatives.
    pub derivatives: serde_json::Value,
    /// Creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Hex SHA-256 of the original bytes.
    pub sha256: Option<String>,
    /// Focal point, 0..1 of width.
    pub focal_x: Option<f32>,
    /// Focal point, 0..1 of height.
    pub focal_y: Option<f32>,
    /// In the trash since; absent in the library.
    pub trashed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<MediaRow> for MediaResponse {
    fn from(row: MediaRow) -> Self {
        Self {
            id: row.id,
            owner_id: row.owner_id,
            file_name: row.file_name,
            mime: row.mime,
            byte_size: row.byte_size,
            storage: row.storage.as_str().to_string(),
            path: row.path,
            width: row.width,
            height: row.height,
            blurhash: row.blurhash,
            alt: row.alt,
            caption: row.caption,
            derivatives: row.derivatives,
            created_at: row.created_at,
            sha256: row.sha256,
            focal_x: row.focal_x,
            focal_y: row.focal_y,
            trashed_at: row.trashed_at,
        }
    }
}

/// Library totals for the page header and Site health.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MediaStatsResponse {
    /// Files in the library.
    pub count: i64,
    /// Bytes of originals.
    pub bytes: i64,
    /// Images with no alt text.
    pub missing_alt: i64,
    /// The configured storage cap in bytes, when there is one.
    pub cap_bytes: Option<i64>,
}

/// Query for `GET /api/v1/media/stats`.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct MediaStatsQuery {
    /// Only this uploader's files (forced to your own without
    /// `edit_others`).
    pub owner_id: Option<i64>,
}

/// Library totals for one uploader. The repository's `stats` has no owner
/// filter, so the scoped form is asked here; it mirrors that query with
/// one more predicate.
async fn owner_stats(
    state: &AppState,
    owner_id: i64,
) -> Result<vyasa_db::repo::MediaStats, AppError> {
    sqlx::query_as::<_, vyasa_db::repo::MediaStats>(
        "SELECT count(*) AS count, COALESCE(sum(byte_size), 0)::bigint AS bytes,
                count(*) FILTER (WHERE mime LIKE 'image/%' AND COALESCE(alt, '') = '') AS missing_alt
         FROM media WHERE trashed_at IS NULL AND owner_id = $1",
    )
    .bind(owner_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|err| AppError::db(format!("media stats failed: {err}")))
}

/// `GET /api/v1/media/stats` — how big the library is.
#[utoipa::path(get, path = "/api/v1/media/stats", tag = "media",
    security(("session_cookie" = [])),
    params(MediaStatsQuery),
    responses((status = 200, description = "Totals", body = MediaStatsResponse)))]
pub async fn stats(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<MediaStatsQuery>,
) -> ApiResult<Json<MediaStatsResponse>> {
    policy::media_reader(&principal)?;
    // Same scoping as the list: without `EditOthers` the totals are your
    // own uploads, whatever `owner_id` asks for.
    let owner_id = policy::media_owner_scope(&principal, query.owner_id);
    let s = match owner_id {
        None => state.media.stats().await?,
        Some(owner) => owner_stats(&state, owner).await?,
    };
    Ok(Json(MediaStatsResponse {
        count: s.count,
        bytes: s.bytes,
        missing_alt: s.missing_alt,
        cap_bytes: storage_cap_bytes(&state).await,
    }))
}

/// The `media_storage_cap_mb` site option as bytes, if set and positive.
pub async fn storage_cap_bytes(state: &AppState) -> Option<i64> {
    let value = state.options.get("media_storage_cap_mb").await.ok()?;
    let mb = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))?;
    (mb > 0).then(|| mb.saturating_mul(1024 * 1024))
}

/// Where a file is used: posts that embed it, and site identity slots.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MediaUsage {
    /// Entries whose content or featured image references the file.
    pub posts: Vec<MediaUsagePost>,
    /// True when it is the site logo.
    pub site_logo: bool,
    /// True when it is the site favicon.
    pub site_favicon: bool,
}

/// One entry using a file.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MediaUsagePost {
    /// Entry id, as a string.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Its status.
    pub status: String,
}

/// A crop as fractions of the picture, top-left origin.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct CropBody {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Crop first, then rotation (0, 90, 180 or 270 clockwise), then flips.
#[derive(serde::Deserialize, utoipa::ToSchema, Default)]
pub struct ImageEditBody {
    #[serde(default)]
    pub rotate: u32,
    #[serde(default)]
    pub flip_h: bool,
    #[serde(default)]
    pub flip_v: bool,
    #[serde(default)]
    pub crop: Option<CropBody>,
}

/// `POST /api/v1/media/{id}/restore` — back from the trash.
#[utoipa::path(post, path = "/api/v1/media/{id}/restore", tag = "media",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Restored", body = MediaResponse)))]
pub async fn restore(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<MediaResponse>> {
    policy::media_for_edit(&state, &principal, id).await?;
    Ok(Json(MediaResponse::from(state.media.restore(id).await?)))
}

/// `POST /api/v1/media/trash/empty` — purge everything in the trash.
#[utoipa::path(post, path = "/api/v1/media/trash/empty", tag = "media",
    security(("session_cookie" = [])),
    responses((status = 200, description = "How many were removed", body = serde_json::Value)))]
pub async fn empty_trash(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let n = state.media.empty_trash().await?;
    Ok(Json(serde_json::json!({ "purged": n })))
}

/// `POST /api/v1/media/{id}/edit` — rotate, flip, crop in place.
#[utoipa::path(post, path = "/api/v1/media/{id}/edit", tag = "media",
    security(("session_cookie" = [])),
    request_body = ImageEditBody,
    responses((status = 200, description = "The updated row", body = MediaResponse),
              (status = 400, description = "Not a raster image, or an empty crop")))]
pub async fn edit(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<ImageEditBody>,
) -> ApiResult<Json<MediaResponse>> {
    policy::media_for_edit(&state, &principal, id).await?;
    let edit = vyasa_core::media::ImageEdit {
        rotate: body.rotate,
        flip_h: body.flip_h,
        flip_v: body.flip_v,
        crop: body.crop.map(|c| vyasa_core::media::CropBox {
            x: c.x,
            y: c.y,
            w: c.w,
            h: c.h,
        }),
    };
    let updated = state.media.edit_image(id, &edit).await?;
    Ok(Json(MediaResponse::from(updated)))
}

/// `GET /api/v1/media/{id}/usage` — what would break if this went.
#[utoipa::path(get, path = "/api/v1/media/{id}/usage", tag = "media",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Media id")),
    responses((status = 200, description = "Usage", body = MediaUsage)))]
pub async fn usage(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<MediaUsage>> {
    policy::media_for_usage(&state, &principal, id).await?;
    Ok(Json(media_usage(&state, id).await?))
}

/// Everything that references media `id`.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn media_usage(state: &AppState, id: i64) -> Result<MediaUsage, AppError> {
    let needle = format!("%/api/v1/media/{id}/raw%");
    let posts = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, title, status FROM posts WHERE status <> 'trash'
         AND (content::text LIKE $1 OR meta::text LIKE $1 OR meta->>'featured_media_id' = $2)
         ORDER BY updated_at DESC LIMIT 50",
    )
    .bind(&needle)
    .bind(id.to_string())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("media usage: {e}")))?
    .into_iter()
    .map(|(id, title, status)| MediaUsagePost {
        id: id.to_string(),
        title,
        status,
    })
    .collect();
    let option_is = |key: &'static str| async move {
        state.options.get(key).await.ok().and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        }) == Some(id)
    };
    Ok(MediaUsage {
        posts,
        site_logo: option_is("site_logo_media_id").await,
        site_favicon: option_is("site_favicon_media_id").await,
    })
}

/// Ids to delete together.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct BatchDeleteBody {
    /// Media ids.
    #[serde(deserialize_with = "crate::flexible_id::vec")]
    pub ids: Vec<i64>,
}

/// What a batch delete did, file by file.
#[derive(Serialize, utoipa::ToSchema)]
pub struct BatchDeleteResult {
    /// Ids removed.
    pub deleted: Vec<String>,
    /// Ids that could not be removed, with why.
    pub failed: Vec<BatchDeleteFailure>,
}

/// One file a batch delete could not remove.
#[derive(Serialize, utoipa::ToSchema)]
pub struct BatchDeleteFailure {
    /// Media id.
    pub id: String,
    /// The reason.
    pub message: String,
}

/// `POST /api/v1/media/batch-delete` — remove several files, reporting
/// each outcome rather than stopping at the first failure.
#[utoipa::path(post, path = "/api/v1/media/batch-delete", tag = "media",
    security(("session_cookie" = [])), request_body = BatchDeleteBody,
    responses((status = 200, description = "Per-file outcome", body = BatchDeleteResult)))]
pub async fn batch_delete(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<BatchDeleteBody>,
) -> ApiResult<Json<BatchDeleteResult>> {
    let mut out = BatchDeleteResult {
        deleted: Vec::new(),
        failed: Vec::new(),
    };
    for id in body.ids.into_iter().take(200) {
        let outcome = async {
            policy::media_for_edit(&state, &principal, id)
                .await
                .map_err(|ApiError(err)| err)?;
            state.media.delete(id).await
        }
        .await;
        match outcome {
            Ok(()) => out.deleted.push(id.to_string()),
            Err(e) => out.failed.push(BatchDeleteFailure {
                id: id.to_string(),
                message: e.client_message(),
            }),
        }
    }
    Ok(Json(out))
}

/// `POST /api/v1/media/{id}/replace` — new bytes under the same id, so
/// every post that embeds the file keeps working.
#[utoipa::path(post, path = "/api/v1/media/{id}/replace", tag = "media",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Media id")),
    request_body(content = String, content_type = "multipart/form-data"),
    responses((status = 200, description = "The row, now pointing at the new file", body = MediaResponse),
              (status = 403, description = "Forbidden", body = ApiErrorBody),
              (status = 404, description = "Not found", body = ApiErrorBody)))]
pub async fn replace(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> ApiResult<Json<MediaResponse>> {
    policy::media_for_edit(&state, &principal, id).await?;
    let mut file_name: Option<String> = None;
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::validation(format!("multipart error: {err}")))?
    {
        if field.name() == Some("file") {
            file_name = field.file_name().map(ToString::to_string);
            let data = field.bytes().await.map_err(|err| {
                if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    AppError::too_large(format!(
                        "The file is larger than the {} MB limit.",
                        vyasa_core::media::MAX_BYTES / (1024 * 1024)
                    ))
                } else {
                    AppError::validation(format!("failed to read file: {err}"))
                }
            })?;
            bytes = Some(data.to_vec());
        } else {
            let _ = field.bytes().await;
        }
    }
    let file_name = file_name.ok_or_else(|| AppError::validation("missing file field"))?;
    let bytes = bytes.ok_or_else(|| AppError::validation("missing file data"))?;
    let updated = state.media.replace(id, &file_name, bytes).await?;
    if let Some(cache) = &state.render_cache {
        cache.invalidate_theme();
    }
    Ok(Json(updated.into()))
}

/// Query for listing media.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct ListMediaQuery {
    /// Page size (default 20, max 100).
    pub limit: Option<i64>,
    /// Offset.
    pub offset: Option<i64>,
    /// Case-insensitive match on file name, alt or caption.
    pub search: Option<String>,
    /// `image`, `video`, `audio`, `document` or `other`.
    pub kind: Option<String>,
    /// `newest` (default), `oldest`, `largest`, `smallest`, `name`.
    pub sort: Option<String>,
    /// Only this uploader's files.
    pub owner_id: Option<i64>,
    /// List the trash instead of the library.
    pub trashed: Option<bool>,
}

/// `POST /api/v1/media` — upload a file (multipart).
///
/// # Errors
///
/// 400 on validation, 403 without `upload_media`, 413 when too large.
#[utoipa::path(
    post, path = "/api/v1/media",
    tag = "media",
    security(("session_cookie" = [])),
    request_body(content = String, description = "multipart/form-data with `file` field", content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Media created", body = MediaResponse),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 413, description = "Payload too large", body = ApiErrorBody),
    )
)]
pub async fn upload(
    State(state): State<AppState>,
    principal: Principal,
    mut multipart: Multipart,
) -> ApiResult<(StatusCode, Json<MediaResponse>)> {
    let mut file_name: Option<String> = None;
    let mut bytes: Option<Vec<u8>> = None;
    let mut alt: Option<String> = None;
    let mut caption: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::validation(format!("multipart error: {err}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                file_name = field.file_name().map(ToString::to_string);
                let data = field.bytes().await.map_err(|err| {
                    // The extractor reports its own limit as a status; that
                    // is the one case that is not the client's formatting.
                    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
                        AppError::too_large(format!(
                            "The file is larger than the {} MB limit.",
                            vyasa_core::media::MAX_BYTES / (1024 * 1024)
                        ))
                    } else {
                        AppError::validation(format!("failed to read file: {err}"))
                    }
                })?;
                if data.len() > vyasa_core::media::MAX_BYTES {
                    return Err(ApiError(AppError::too_large(format!(
                        "The file is {} MB; the limit is {} MB.",
                        data.len() / (1024 * 1024),
                        vyasa_core::media::MAX_BYTES / (1024 * 1024)
                    ))));
                }
                bytes = Some(data.to_vec());
            }
            "alt" => {
                let text = field
                    .text()
                    .await
                    .map_err(|err| AppError::validation(format!("failed to read alt: {err}")))?;
                alt = Some(text);
            }
            "caption" => {
                let text = field.text().await.map_err(|err| {
                    AppError::validation(format!("failed to read caption: {err}"))
                })?;
                caption = Some(text);
            }
            _ => {
                // Ignore unknown fields.
                let _ = field.bytes().await;
            }
        }
    }

    let file_name = file_name.ok_or_else(|| AppError::validation("missing file field"))?;
    let bytes = bytes.ok_or_else(|| AppError::validation("missing file data"))?;

    // The same bytes again are not a second file: the library's copy is
    // returned as it is (200, not 201) so the caller can say so.
    if let Some(existing) = state.media.duplicate_of(&bytes).await? {
        return Ok((StatusCode::OK, Json(MediaResponse::from(existing))));
    }
    if let Some(cap) = storage_cap_bytes(&state).await {
        let used = state.media.stats().await?.bytes;
        if used.saturating_add(i64::try_from(bytes.len()).unwrap_or(i64::MAX)) > cap {
            return Err(ApiError(AppError::too_large(format!(
                "The library is at its {} MB cap; delete something or raise the cap under Settings.",
                cap / (1024 * 1024)
            ))));
        }
    }
    let row = state
        .media
        .upload(principal.user().id, &file_name, bytes, alt, caption)
        .await?;
    // An image with no alt text gets one written by the vision model when
    // the site has that switched on; the upload does not wait for it.
    if row.mime.starts_with("image/")
        && row.alt.as_deref().is_none_or(|a| a.trim().is_empty())
        && crate::ai_features::AiSettings::load(&state).await.alt_text
    {
        crate::ai_jobs::enqueue(
            &state,
            "ai_alt_text",
            serde_json::json!({ "media_id": row.id }),
        )
        .await;
    }
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::MEDIA_UPLOADED,
        serde_json::json!({
            "media_id": row.id,
            "mime": row.mime,
            "bytes": row.byte_size,
        }),
    );
    Ok((StatusCode::CREATED, Json(MediaResponse::from(row))))
}

/// `GET /api/v1/media` — list media, newest first.
///
/// # Errors
///
/// 401 when unauthenticated.
#[utoipa::path(
    get, path = "/api/v1/media",
    tag = "media",
    security(("session_cookie" = [])),
    params(ListMediaQuery),
    responses(
        (status = 200, description = "Media list", body = [MediaResponse]),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<ListMediaQuery>,
) -> ApiResult<(axum::http::HeaderMap, Json<Vec<MediaResponse>>)> {
    policy::media_reader(&principal)?;
    // Without `EditOthers` the library is your own uploads, whatever
    // `owner_id` asks for (the admin Media page already passes your own
    // id for authors; that keeps working, anything else is narrowed).
    let owner_id = policy::media_owner_scope(&principal, query.owner_id);
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let offset = query.offset.unwrap_or(0).max(0);
    let filter = vyasa_db::repo::MediaFilter {
        search: query.search,
        kind: query.kind,
        owner_id,
        sort: query.sort,
        trashed: query.trashed.unwrap_or(false),
    };
    let (rows, total) = state
        .media
        .repo()
        .list_filtered(&filter, limit, offset)
        .await?;
    let body = Json(
        rows.into_iter()
            .map(MediaResponse::from)
            .collect::<Vec<_>>(),
    );
    // The total rides in a header so the array shape every picker reads
    // stays as it was.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::HeaderName::from_static("x-total-count"),
        axum::http::HeaderValue::from(total),
    );
    Ok((headers, body))
}

/// `GET /api/v1/media/{id}` — fetch a media item.
///
/// # Errors
///
/// 404 when missing.
#[utoipa::path(
    get, path = "/api/v1/media/{id}",
    tag = "media",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Media id")),
    responses(
        (status = 200, description = "Media", body = MediaResponse),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<MediaResponse>> {
    let row = policy::media_for_view(&state, &principal, id).await?;
    Ok(Json(MediaResponse::from(row)))
}

/// Query for raw variant.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct RawQuery {
    /// Variant (`thumb`, `medium`, `large`, `webp`, `avif`).
    pub variant: Option<String>,
}

/// The derivative to serve for this request: the asked-for variant,
/// upgraded to its WebP twin when the client's `Accept` allows — the
/// negotiation is what makes image optimization automatic, with zero
/// markup changes and graceful service of uploads that predate real
/// derivatives (no twin recorded → no upgrade).
fn negotiated_variant(
    derivatives: &serde_json::Value,
    requested: Option<&str>,
    accept: Option<&str>,
) -> Option<String> {
    let wants_webp = accept.is_some_and(|a| a.contains("image/webp"));
    let webp_key = match requested {
        // Already asking for a webp flavour: no upgrade to invent.
        Some(v) if v.starts_with("webp") => None,
        Some(v) => Some(format!("webp_{v}")),
        None => Some("webp".to_owned()),
    };
    if wants_webp {
        if let Some(key) = webp_key {
            if derivatives.get(&key).is_some() {
                return Some(key);
            }
        }
    }
    requested.map(str::to_owned)
}

/// Content type for the bytes actually served: the variant path's own
/// extension wins (a WebP twin must never travel as `image/png` — the
/// `nosniff` header would make browsers drop it), the row's mime
/// otherwise.
fn served_mime(row_mime: &str, variant_path: Option<&str>) -> String {
    variant_path.map_or_else(
        || row_mime.to_owned(),
        |p| mime_guess::from_path(p).first_or_octet_stream().to_string(),
    )
}

/// `GET /api/v1/media/{id}/raw` — serve raw bytes with ETag/Cache-Control.
/// Public (no auth) for published use; still checks If-None-Match.
///
/// # Errors
///
/// 404 when missing.
#[utoipa::path(
    get, path = "/api/v1/media/{id}/raw",
    tag = "media",
    params(
        ("id" = i64, Path, description = "Media id"),
        RawQuery,
    ),
    responses(
        (status = 200, description = "Raw bytes", content_type = "application/octet-stream"),
        (status = 304, description = "Not modified"),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn raw(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<RawQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let row = state.media.repo().get(id).await.map_err(ApiError)?;
    let accept = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok());
    let variant = negotiated_variant(&row.derivatives, query.variant.as_deref(), accept);
    let mut served_path: Option<String> = None;
    let bytes = if let Some(variant) = variant.as_deref() {
        // Look up variant path in derivatives JSON.
        if let Some(path) = row
            .derivatives
            .get(variant)
            .and_then(|v| v.get("path"))
            .and_then(|p| p.as_str())
        {
            // Try to serve variant; fall back to original if missing (job not yet done).
            match state.media.get_bytes_for_path(path).await {
                Ok(b) => {
                    served_path = Some(path.to_owned());
                    b
                }
                Err(_) => state.media.get_bytes(&row).await.map_err(ApiError)?,
            }
        } else {
            // Variant not yet generated; fall back to original.
            state.media.get_bytes(&row).await.map_err(ApiError)?
        }
    } else {
        state.media.get_bytes(&row).await.map_err(ApiError)?
    };
    let etag = MediaService::etag_for(&bytes);
    // Check If-None-Match
    if let Some(if_none_match) = headers.get(axum::http::header::IF_NONE_MATCH) {
        if let Ok(value) = if_none_match.to_str() {
            if value == etag {
                return Ok(StatusCode::NOT_MODIFIED.into_response());
            }
        }
    }
    let mime = served_mime(&row.mime, served_path.as_deref());
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        HeaderName::from_static("etag"),
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"\"")),
    );
    response_headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    if row.mime.starts_with("image/") {
        // The WebP upgrade depends on the Accept header; caches must key
        // on it or a webp body could be handed to a client that asked
        // for the original.
        response_headers.insert(
            HeaderName::from_static("vary"),
            HeaderValue::from_static("Accept"),
        );
    }
    let mut response = (StatusCode::OK, bytes).into_response();
    // Override content-type from `bytes` default.
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_str(&mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    for (k, v) in response_headers {
        if let Some(k) = k {
            response.headers_mut().insert(k, v);
        }
    }
    Ok(response)
}

/// `DELETE /api/v1/media/{id}` — delete a media item.
///
/// # Errors
///
/// 403 without `upload_media` or not owner, 404 when missing.
#
[utoipa::path(
    delete, path = "/api/v1/media/{id}",
    tag = "media",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Media id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let row = policy::media_for_edit(&state, &principal, id).await?;
    if row.trashed_at.is_some() {
        state.media.purge(id).await?;
    } else {
        state.media.delete(id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Editable media metadata. Absent fields are left alone.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateMediaBody {
    /// Accessibility description.
    pub alt: Option<String>,
    /// Caption shown with the file.
    pub caption: Option<String>,
    /// A new file name (sanitised like an upload's).
    pub file_name: Option<String>,
    /// Focal point, 0..1 of width.
    pub focal_x: Option<f32>,
    /// Focal point, 0..1 of height.
    pub focal_y: Option<f32>,
}

/// `PATCH /api/v1/media/{id}` — update alt text and caption.
///
/// Alt text is the one piece of media metadata an author must be able to fix
/// after upload, so the library can be made accessible without re-uploading.
#[utoipa::path(
    patch, path = "/api/v1/media/{id}",
    tag = "media",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Media id")),
    request_body = UpdateMediaBody,
    responses(
        (status = 200, description = "Updated", body = MediaResponse),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<UpdateMediaBody>,
) -> ApiResult<Json<MediaResponse>> {
    policy::media_for_edit(&state, &principal, id).await?;
    let updated = state
        .media
        .repo()
        .update_meta(id, body.alt.as_deref(), body.caption.as_deref())
        .await?;
    let file_name = body
        .file_name
        .as_deref()
        .map(vyasa_core::media::sanitize_file_name)
        .filter(|n| !n.is_empty());
    let focal = match (body.focal_x, body.focal_y) {
        (Some(x), Some(y)) if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) => Some((x, y)),
        (Some(_), Some(_)) => {
            return Err(ApiError(AppError::validation(
                "the focal point is a fraction of width and height, 0 to 1",
            )))
        }
        _ => None,
    };
    let updated = if file_name.is_some() || focal.is_some() {
        state
            .media
            .repo()
            .update_details(id, file_name.as_deref(), focal)
            .await?
    } else {
        updated
    };
    Ok(Json(updated.into()))
}

#[cfg(test)]
mod raw_tests {
    use super::{negotiated_variant, served_mime};

    fn derivs() -> serde_json::Value {
        serde_json::json!({
            "medium": {"path": "1/1/medium_a.jpg", "width": 768},
            "webp_medium": {"path": "1/1/webp_medium_a.webp", "width": 768},
            "webp": {"path": "1/1/webp_a.webp", "width": 1600},
        })
    }

    #[test]
    fn accept_webp_upgrades_to_the_twin_when_it_exists() {
        let d = derivs();
        let webp = Some("text/html,image/webp,*/*");
        assert_eq!(
            negotiated_variant(&d, Some("medium"), webp).as_deref(),
            Some("webp_medium")
        );
        assert_eq!(negotiated_variant(&d, None, webp).as_deref(), Some("webp"));
        // thumb has no twin recorded (pre-fix upload): no upgrade invented.
        assert_eq!(
            negotiated_variant(&d, Some("thumb"), webp).as_deref(),
            Some("thumb")
        );
        // A client that never said webp gets exactly what it asked for.
        assert_eq!(
            negotiated_variant(&d, Some("medium"), Some("image/avif,image/*")).as_deref(),
            Some("medium")
        );
        assert_eq!(negotiated_variant(&d, None, None), None);
        // An explicit webp ask is never double-prefixed.
        assert_eq!(
            negotiated_variant(&d, Some("webp"), webp).as_deref(),
            Some("webp")
        );
    }

    #[test]
    fn the_content_type_follows_the_bytes_actually_served() {
        assert_eq!(
            served_mime("image/png", Some("1/1/webp_a.webp")),
            "image/webp"
        );
        assert_eq!(
            served_mime("image/png", Some("1/1/thumb_a.jpg")),
            "image/jpeg"
        );
        assert_eq!(served_mime("image/png", None), "image/png");
    }
}
