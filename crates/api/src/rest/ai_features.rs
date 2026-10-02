//! REST surface of the AI integrations: the on-demand versions of what the
//! event-driven jobs do, plus image generation and the read-aloud/related
//! lookups the admin uses. ManageOptions is not required — an author may
//! ask for alt text on their own upload — but every call is logged and
//! budgeted like any other AI use.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use vyasa_common::AppError;
use vyasa_core::user::Capability;

use crate::ai_features::{self, AiSettings, Screening};
use crate::ai_jobs::enqueue;
use crate::error::{ApiError, ApiResult};
use crate::middleware::Principal;
use crate::policy;
use crate::rest::media::MediaResponse;
use crate::state::AppState;

/// Site-wide cap on generated images per day; a runaway loop or a curious
/// user should not empty the budget in an afternoon.
const IMAGES_PER_DAY: i64 = 50;

/// Every on-demand call honours the same switch as its automatic
/// counterpart: a feature an operator turned off stays off.
fn require(on: bool, feature: &str) -> Result<(), ApiError> {
    if on {
        Ok(())
    } else {
        Err(ApiError(AppError::validation(format!(
            "{feature} is turned off in Settings → AI features"
        ))))
    }
}

/// `POST /api/v1/media/{id}/alt-text` — write alt text for an image now.
#[utoipa::path(
    post, path = "/api/v1/media/{id}/alt-text", tag = "media",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Media id")),
    responses(
        (status = 200, description = "Alt text written", body = serde_json::Value),
        (status = 403, description = "Someone else's file without edit_others"),
        (status = 404, description = "Not found"),
    )
)]
pub async fn alt_text(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    policy::media_for_edit(&state, &principal, id).await?;
    require(AiSettings::load(&state).await.alt_text, "Alt text")?;
    let alt = ai_features::alt_text(&state, id, true).await?;
    Ok(Json(json!({ "alt": alt })))
}

/// `POST /api/v1/media/{id}/transcribe` — queue a transcription.
#[utoipa::path(
    post, path = "/api/v1/media/{id}/transcribe", tag = "media",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Media id")),
    responses(
        (status = 202, description = "Queued", body = serde_json::Value),
        (status = 403, description = "Someone else's file without edit_others"),
        (status = 404, description = "Not found"),
    )
)]
pub async fn transcribe(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let row = policy::media_for_edit(&state, &principal, id).await?;
    require(
        AiSettings::load(&state).await.transcription,
        "Transcription",
    )?;
    if !(row.mime.starts_with("audio/") || row.mime.starts_with("video/")) {
        return Err(ApiError(AppError::validation(
            "only audio and video files can be transcribed",
        )));
    }
    // Fail fast when no model is registered rather than dead-lettering a job.
    crate::ai_registry::first(&state, vyasa_core::ai_models::ModelKind::Transcription).await?;
    enqueue(&state, "ai_transcribe_media", json!({ "media_id": id })).await;
    Ok((StatusCode::ACCEPTED, Json(json!({ "queued": true }))))
}

/// `GET /api/v1/media/{id}/transcript` — the stored transcript, if any.
#[utoipa::path(
    get, path = "/api/v1/media/{id}/transcript", tag = "media",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Media id")),
    responses(
        (status = 200, description = "Transcript", body = serde_json::Value),
        (status = 403, description = "Not a library reader (upload_media or edit_posts)"),
        (status = 404, description = "Not found"),
    )
)]
pub async fn transcript(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    // Not the route's access (a library reader): a key scoped to the
    // library alone still needs `view_admin` here, as it always did.
    principal.ensure(Capability::ViewAdmin)?;
    // A transcript is part of the file's metadata: for library readers.
    policy::media_for_view(&state, &principal, id).await?;
    let text = vyasa_db::repo::AiDataRepo::new(state.pool.clone())
        .transcript(id)
        .await?;
    Ok(Json(json!({ "transcript": text })))
}

/// `POST /api/v1/posts/{id}/read-aloud` — queue the audio version.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/read-aloud", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses((status = 202, description = "Queued", body = serde_json::Value))
)]
pub async fn read_aloud(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    policy::post_for_edit(&state, &principal, id).await?;
    require(AiSettings::load(&state).await.read_aloud, "Read aloud")?;
    crate::ai_registry::first(&state, vyasa_core::ai_models::ModelKind::Speech).await?;
    enqueue(&state, "ai_read_aloud", json!({ "post_id": id })).await;
    Ok((StatusCode::ACCEPTED, Json(json!({ "queued": true }))))
}

/// `GET /api/v1/posts/{id}/audio` — the read-aloud recording, if any.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}/audio", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses(
        (status = 200, description = "Recording", body = serde_json::Value),
        (status = 403, description = "Entry not visible"),
        (status = 404, description = "Not found"),
    )
)]
pub async fn audio(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    // The recording is the entry read aloud: for whoever may read it.
    policy::post_for_view(&state, &principal, id, None).await?;
    let row = vyasa_db::repo::AiDataRepo::new(state.pool.clone())
        .post_audio(id)
        .await?;
    Ok(Json(match row {
        Some(a) => json!({
            "media_id": a.media_id.to_string(),
            "url": format!("/api/v1/media/{}/raw", a.media_id),
            "model": a.model,
        }),
        None => json!({ "media_id": null }),
    }))
}

/// Related-posts query.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct RelatedQuery {
    /// How many, at most 10.
    pub limit: Option<usize>,
}

/// `GET /api/v1/posts/{id}/related` — nearest published posts by meaning.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}/related", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id"), RelatedQuery),
    responses((status = 200, description = "Related posts", body = serde_json::Value))
)]
pub async fn related(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<RelatedQuery>,
) -> ApiResult<Json<Value>> {
    let pattern = crate::permalinks::pattern(&state).await;
    require(
        AiSettings::load(&state).await.related_posts,
        "Related posts",
    )?;
    let posts =
        ai_features::related_posts(&state, id, query.limit.unwrap_or(5).clamp(1, 10)).await?;
    Ok(Json(json!({
        "posts": posts.iter().map(|p| json!({
            "id": p.id.to_string(),
            "title": p.title,
            "slug": p.slug,
            "url": pattern.path_for(p),
        })).collect::<Vec<_>>()
    })))
}

/// `POST /api/v1/posts/{id}/embed` — (re)compute the post's embedding now.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/embed", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses((status = 200, description = "Outcome", body = serde_json::Value))
)]
pub async fn embed(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    policy::post_for_edit(&state, &principal, id).await?;
    require(AiSettings::load(&state).await.embeddings, "Embeddings")?;
    Ok(Json(ai_features::embed_post(&state, id).await?))
}

/// `POST /api/v1/posts/{id}/autofill` — fill an empty excerpt and SEO
/// description from the text model, now.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/autofill", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses((status = 200, description = "Which fields were filled", body = serde_json::Value))
)]
pub async fn autofill(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    policy::post_for_edit(&state, &principal, id).await?;
    require(AiSettings::load(&state).await.autofill, "Autofill")?;
    Ok(Json(ai_features::autofill_post(&state, id).await?))
}

/// `POST /api/v1/comments/{id}/screen` — run the moderation model on a
/// comment now. Records the verdict; moves to spam only when the site's
/// screening mode says so.
#[utoipa::path(
    post, path = "/api/v1/comments/{id}/screen", tag = "comments",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Comment id")),
    responses((status = 200, description = "Verdict", body = serde_json::Value))
)]
pub async fn screen(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Value>> {
    let mode = AiSettings::load(&state).await.comment_screening;
    require(mode != Screening::Off, "Comment screening")?;
    Ok(Json(ai_features::moderate_comment(&state, id, mode).await?))
}

/// Image request.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ImageBody {
    /// What to draw.
    pub prompt: String,
    /// `1024x1024` (default), `1024x1536`, `1536x1024`, `1024x1792`, `1792x1024`.
    #[serde(default)]
    pub size: String,
}

/// What AI the editor can offer right now.
///
/// A struct of switches is the honest shape here: the editor reads each
/// one independently to decide which affordance to show.
#[allow(clippy::struct_excessive_bools)]
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct AiAvailable {
    /// A text model is registered: writing assist, suggestions, continue.
    pub text: bool,
    /// A vision model is registered: alt text from the picture.
    pub vision: bool,
    /// An image model is registered and image generation is switched on.
    pub image: bool,
    /// A transcription model is registered and transcription is switched on.
    pub transcription: bool,
    /// A speech model is registered and read-aloud is switched on.
    pub speech: bool,
    /// Embedding generation is enabled and configured.
    pub embeddings: bool,
    /// Empty-field completion is enabled and configured.
    pub autofill: bool,
    /// Comment screening is enabled.
    pub screening: bool,
}

/// Whether at least one usable model of `kind` is registered.
async fn has_model(state: &AppState, kind: vyasa_core::ai_models::ModelKind) -> bool {
    state
        .ai_models
        .chain(kind)
        .await
        .is_ok_and(|models| !models.is_empty())
}

/// `GET /api/v1/ai/available` — which AI affordances to show.
///
/// Any admin-area user may ask: the editor uses it to hide an Assist
/// button that could only fail, rather than reading the model registry,
/// which stays with `ManageOptions`. Nothing here spends budget.
#[utoipa::path(
    get, path = "/api/v1/ai/available", tag = "ai",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Availability by feature", body = AiAvailable))
)]
pub async fn available(State(state): State<AppState>) -> ApiResult<Json<AiAvailable>> {
    use vyasa_core::ai_models::ModelKind;
    let settings = AiSettings::load(&state).await;
    // The text chain also counts the pre-registry environment key.
    let text = crate::ai_registry::text(&state).await.is_ok();
    Ok(Json(AiAvailable {
        embeddings: settings.embeddings && has_model(&state, ModelKind::Embedding).await,
        autofill: settings.autofill && text,
        screening: settings.comment_screening != Screening::Off,
        text,
        vision: has_model(&state, ModelKind::Vision).await,
        image: settings.images && has_model(&state, ModelKind::Image).await,
        transcription: settings.transcription && has_model(&state, ModelKind::Transcription).await,
        speech: settings.read_aloud && has_model(&state, ModelKind::Speech).await,
    }))
}

/// `POST /api/v1/ai/images` — generate an image into the media library.
#[utoipa::path(
    post, path = "/api/v1/ai/images", tag = "ai",
    security(("session_cookie" = [])),
    request_body = ImageBody,
    responses((status = 201, description = "The new media item", body = MediaResponse),
              (status = 429, description = "Daily image cap reached"))
)]
pub async fn generate_image(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<ImageBody>,
) -> ApiResult<(StatusCode, Json<MediaResponse>)> {
    if !AiSettings::load(&state).await.images {
        return Err(ApiError(AppError::validation(
            "image generation is turned off in Settings → AI features",
        )));
    }
    let today = vyasa_db::repo::AiLogRepo::new(state.pool.clone())
        .count_purpose_since(
            "image-generate",
            chrono::Utc::now() - chrono::Duration::hours(24),
        )
        .await?;
    if today >= IMAGES_PER_DAY {
        return Err(ApiError(AppError::rate_limited(format!(
            "the site's {IMAGES_PER_DAY} generated images for today are used up"
        ))));
    }
    let row =
        ai_features::generate_image(&state, principal.user().id, &body.prompt, &body.size).await?;
    Ok((StatusCode::CREATED, Json(MediaResponse::from(row))))
}

/// A bounded, explicitly requested batch of existing content to process.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct BackfillRequest {
    /// `embeddings`, `autofill`, or `screen`.
    pub feature: String,
    /// Continue after this id; omitted starts at the beginning.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub after_id: Option<i64>,
}

/// Queue up to 100 missing results. Enabling a feature alone never spends on a backlog.
#[utoipa::path(post, path = "/api/v1/ai/backfill", tag = "ai",
    request_body = BackfillRequest,
    responses((status = 200, description = "Queued batch and continuation", body = serde_json::Value)))]
pub async fn backfill(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<BackfillRequest>,
) -> ApiResult<Json<Value>> {
    // Repeats the route's access on purpose: this function is also called
    // directly (graphql/tests.rs), where no route guard has run.
    principal.ensure(Capability::ManageOptions)?;
    let settings = AiSettings::load(&state).await;
    let (kind, key, source) = match body.feature.as_str() {
        "embeddings" => {
            require(settings.embeddings, "Embeddings")?;
            ("ai_embed_post", "post_id", "SELECT p.id FROM posts p WHERE p.status = 'published' AND p.password_hash IS NULL AND NOT EXISTS (SELECT 1 FROM post_embeddings e WHERE e.post_id = p.id)")
        }
        "autofill" => {
            require(settings.autofill, "Autofill")?;
            ("ai_autofill_post", "post_id", "SELECT p.id FROM posts p WHERE p.status = 'published' AND p.password_hash IS NULL AND (COALESCE(trim(p.excerpt), '') = '' OR COALESCE(trim(p.meta->>'seo_description'), '') = '')")
        }
        "screen" => {
            require(
                settings.comment_screening != Screening::Off,
                "Comment screening",
            )?;
            ("ai_moderate_comment", "comment_id", "SELECT p.id FROM comments p WHERE p.status IN ('pending', 'approved') AND p.moderation IS NULL")
        }
        _ => {
            return Err(crate::error::ApiError(AppError::validation(
                "unknown backfill feature",
            )))
        }
    };
    let db = |e: sqlx::Error| AppError::db(e.to_string());
    let mut tx = state.pool.begin().await.map_err(db)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("ai-backfill:{kind}"))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
    let ids: Vec<i64> = sqlx::query_scalar(&format!("{source} AND p.id > $1 AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.kind = $2 AND j.status IN ('queued', 'running') AND j.payload->>$3 = p.id::text) ORDER BY p.id LIMIT 101"))
        .bind(body.after_id.unwrap_or(0)).bind(kind).bind(key).fetch_all(&mut *tx).await.map_err(db)?;
    let more = ids.len() > 100;
    let batch = &ids[..ids.len().min(100)];
    for id in batch {
        sqlx::query("INSERT INTO jobs (id, kind, payload, run_at, status) VALUES ($1, $2, $3, now(), 'queued')")
            .bind(vyasa_common::next_id_i64()).bind(kind).bind(json!({(key): id}))
            .execute(&mut *tx).await.map_err(db)?;
    }
    tx.commit().await.map_err(db)?;
    state.publisher_notify.notify_one();
    Ok(Json(
        json!({"queued":batch.len(), "next_after_id": if more { batch.last().map(i64::to_string) } else { None }}),
    ))
}
