//! Comment endpoints: public submission + threaded read, admin moderation.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use vyasa_db::content_models::{CommentRow, CommentStatus};

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::MaybePrincipal;
use crate::state::AppState;

/// Request for submitting a comment (public).
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateCommentRequest {
    /// Display name.
    pub author_name: String,
    /// Email.
    pub author_email: String,
    /// Body.
    pub content: String,
    /// Parent comment id, if reply.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
}

/// How a caller shows it has unlocked a password-protected entry.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct UnlockQuery {
    /// Signed read token for a password-protected entry (from
    /// `POST /posts/{id}/verify-password`). The entry page's unlock cookie
    /// is accepted instead. Without either, a protected entry's comments
    /// answer 404 to anyone who may not edit it.
    pub token: Option<String>,
}

impl UnlockQuery {
    /// The token given: the query's, else the unlock cookie's.
    fn token<'a>(&'a self, headers: &'a axum::http::HeaderMap, post_id: i64) -> Option<&'a str> {
        self.token
            .as_deref()
            .or_else(|| crate::public::routes::unlock_cookie(headers, post_id))
    }
}

/// Response for a comment.
#[derive(Serialize, utoipa::ToSchema)]
pub struct CommentResponse {
    /// Comment id.
    pub id: i64,
    /// Post id.
    pub post_id: i64,
    /// Author user id, if logged in.
    pub author_user_id: Option<i64>,
    /// Display name.
    pub author_name: String,
    /// Body.
    pub content: String,
    /// Parent id.
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub parent_id: Option<i64>,
    /// Status.
    pub status: String,
    /// Creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// AI screening verdict, when the comment was screened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderation: Option<serde_json::Value>,
}

impl From<CommentRow> for CommentResponse {
    fn from(row: CommentRow) -> Self {
        Self {
            id: row.id,
            post_id: row.post_id,
            author_user_id: row.author_user_id,
            author_name: row.author_name,
            content: row.content,
            parent_id: row.parent_id,
            status: row.status.as_str().to_string(),
            created_at: row.created_at,
            moderation: None,
        }
    }
}

/// Query for admin listing.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct ListCommentsQuery {
    /// Filter by status.
    pub status: Option<String>,
    /// Page size.
    pub limit: Option<i64>,
    /// Offset.
    pub offset: Option<i64>,
}

/// `POST /api/v1/posts/{id}/comments` — public submit.
///
/// # Errors
///
/// 400 on validation, 404 when post missing.
#[utoipa::path(
    post, path = "/api/v1/posts/{id}/comments",
    tag = "comments",
    params(("id" = i64, Path, description = "Post id"), UnlockQuery),
    request_body = CreateCommentRequest,
    responses(
        (status = 201, description = "Comment created", body = CommentResponse),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 404, description = "No such entry, or not visible to the caller (a protected entry not unlocked)", body = ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    Path(post_id): Path<i64>,
    Query(unlock): Query<UnlockQuery>,
    headers: axum::http::HeaderMap,
    // Optional auth: if a valid session is present, we capture author_user_id.
    maybe: MaybePrincipal,
    Json(body): Json<CreateCommentRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<CommentResponse>)> {
    // Missing, hidden from the caller (a protected entry not unlocked
    // included), or of a type they may not read: all the same 404,
    // decided before the comment is looked at.
    crate::policy::comment_target(
        &state,
        maybe.0.as_ref(),
        post_id,
        unlock.token(&headers, post_id),
    )
    .await?;
    let author_user_id = maybe.0.as_ref().map(|p| p.user().id);
    let row = state
        .comments
        .submit(vyasa_core::comment::service::SubmitComment {
            post_id,
            author_user_id,
            author_name: body.author_name,
            author_email: body.author_email,
            content: body.content,
            parent_id: body.parent_id,
        })
        .await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(CommentResponse::from(row)),
    ))
}

/// `GET /api/v1/posts/{id}/comments` — approved threaded comments (public).
///
/// # Errors
///
/// 404 when post missing.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}/comments",
    tag = "comments",
    params(("id" = i64, Path, description = "Post id"), UnlockQuery),
    responses(
        (status = 200, description = "Approved comments", body = [CommentResponse]),
        (status = 404, description = "No such entry, or not visible to the caller (a protected entry not unlocked)", body = ApiErrorBody),
    )
)]
pub async fn list_approved(
    State(state): State<AppState>,
    maybe: MaybePrincipal,
    Path(post_id): Path<i64>,
    Query(unlock): Query<UnlockQuery>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Json<Vec<CommentResponse>>> {
    crate::policy::comment_target(
        &state,
        maybe.0.as_ref(),
        post_id,
        unlock.token(&headers, post_id),
    )
    .await?;
    let rows = state.comments.list_approved_threaded(post_id).await?;
    Ok(Json(rows.into_iter().map(CommentResponse::from).collect()))
}

/// `GET /api/v1/comments` — admin list by status.
///
/// # Errors
///
/// 403 without `moderate_comments`.
#[utoipa::path(
    get, path = "/api/v1/comments",
    tag = "comments",
    security(("session_cookie" = [])),
    params(ListCommentsQuery),
    responses(
        (status = 200, description = "Comments", body = [CommentResponse]),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn admin_list(
    State(state): State<AppState>,
    Query(query): Query<ListCommentsQuery>,
) -> ApiResult<(axum::http::HeaderMap, Json<Vec<CommentResponse>>)> {
    let status = match query.status {
        Some(ref s) if !s.is_empty() => CommentStatus::parse(s).map_err(ApiError)?,
        _ => CommentStatus::Pending,
    };
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let offset = query.offset.unwrap_or(0).max(0);
    let total = state.comments.count_by_status(status).await?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-total-count", total.into());
    let rows = state.comments.list_by_status(status, limit, offset).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let verdicts = vyasa_db::repo::AiDataRepo::new(state.pool.clone())
        .comment_moderation(&ids)
        .await
        .unwrap_or_default();
    Ok((
        headers,
        Json(
            rows.into_iter()
                .map(|row| {
                    let mut out = CommentResponse::from(row);
                    out.moderation = verdicts
                        .iter()
                        .find(|(id, _)| *id == out.id)
                        .map(|(_, v)| v.clone());
                    out
                })
                .collect(),
        ),
    ))
}

/// Drops cached entry pages so a moderation decision is visible at once.
async fn purge_entry_pages(state: &AppState) {
    let Some(cache) = &state.render_cache else {
        return;
    };
    if let Ok(Some(theme)) = state.themes.get_active().await {
        cache.invalidate_entries(&crate::render_cache::theme_namespace(
            &theme.name,
            theme.version,
        ));
    }
}

/// `POST /api/v1/comments/{id}/approve` — moderate to approved.
///
/// # Errors
///
/// 403/404 as appropriate.
#[utoipa::path(
    post, path = "/api/v1/comments/{id}/approve",
    tag = "comments",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Comment id")),
    responses(
        (status = 200, description = "Approved", body = CommentResponse),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn approve(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<CommentResponse>> {
    let row = state.comments.moderate(id, CommentStatus::Approved).await?;
    purge_entry_pages(&state).await;
    // Approved comments render inside the cached single page (phase 25
    // dynamic block). Emit so subscribers (search, render-cache when wired)
    // can invalidate. Direct prefix purge as a safety net when the cache is
    // wired.
    vyasa_core::events::emit_comment_approved(row.id, row.post_id);
    // Distinct from `comment-added`, which fires when a visitor submits:
    // this is the moment a comment becomes publicly visible.
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::COMMENT_APPROVED,
        serde_json::json!({ "comment_id": row.id, "post_id": row.post_id }),
    );
    Ok(Json(CommentResponse::from(row)))
}

/// `POST /api/v1/comments/{id}/spam` — mark as spam.
///
/// # Errors
///
/// 403/404 as appropriate.
#[utoipa::path(
    post, path = "/api/v1/comments/{id}/spam",
    tag = "comments",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Comment id")),
    responses(
        (status = 200, description = "Marked spam", body = CommentResponse),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn spam(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<CommentResponse>> {
    let row = state.comments.moderate(id, CommentStatus::Spam).await?;
    purge_entry_pages(&state).await;
    Ok(Json(CommentResponse::from(row)))
}

/// `POST /api/v1/comments/{id}/trash` — trash.
///
/// # Errors
///
/// 403/404 as appropriate.
#[utoipa::path(
    post, path = "/api/v1/comments/{id}/trash",
    tag = "comments",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Comment id")),
    responses(
        (status = 200, description = "Trashed", body = CommentResponse),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn trash(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<CommentResponse>> {
    let row = state.comments.moderate(id, CommentStatus::Trash).await?;
    purge_entry_pages(&state).await;
    Ok(Json(CommentResponse::from(row)))
}

/// `POST /api/v1/comments/{id}/restore` — out of the trash or spam,
/// back to pending for another look.
#[utoipa::path(post, path = "/api/v1/comments/{id}/restore", tag = "comments",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Pending again", body = CommentResponse)))]
pub async fn restore(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<CommentResponse>> {
    let row = state.comments.moderate(id, CommentStatus::Pending).await?;
    Ok(Json(CommentResponse::from(row)))
}
