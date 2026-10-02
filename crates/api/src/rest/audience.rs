//! Admin endpoints over the audience tables: the form inbox and the
//! subscriber list.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use crate::error::{ApiErrorBody, ApiResult};
use crate::state::AppState;

/// One form submission, as the inbox shows it.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SubmissionResponse {
    /// Id.
    pub id: i64,
    /// Form name.
    pub form: String,
    /// Visitor name.
    pub name: String,
    /// Visitor email.
    pub email: String,
    /// Message.
    pub message: String,
    /// When it arrived.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// One subscriber. The confirm/unsubscribe token stays server-side.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SubscriberResponse {
    /// Id.
    pub id: i64,
    /// Address.
    pub email: String,
    /// `pending` / `confirmed` / `unsubscribed`.
    pub status: String,
    /// Signup time.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Confirmation time, when confirmed.
    pub confirmed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// `GET /api/v1/audience/submissions` — the form inbox, newest first.
///
/// # Errors
/// 401 unauthenticated, 403 without `EditOthers`.
#[utoipa::path(
    get, path = "/api/v1/audience/submissions", tag = "audience",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Submissions", body = [SubmissionResponse]),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn submissions(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<SubmissionResponse>>> {
    let rows = state.audience.submissions(500).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| SubmissionResponse {
                id: r.id,
                form: r.form,
                name: r.name,
                email: r.email,
                message: r.message,
                created_at: r.created_at,
            })
            .collect(),
    ))
}

/// `DELETE /api/v1/audience/submissions/{id}` — done with one.
///
/// # Errors
/// 404 when missing, 403 without `ManageOptions`.
#[utoipa::path(
    delete, path = "/api/v1/audience/submissions/{id}", tag = "audience",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Submission id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn delete_submission(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    state.audience.delete_submission(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/audience/subscribers` — everyone on the list.
///
/// # Errors
/// 401 unauthenticated, 403 without `EditOthers`.
#[utoipa::path(
    get, path = "/api/v1/audience/subscribers", tag = "audience",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Subscribers", body = [SubscriberResponse]),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn subscribers(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<SubscriberResponse>>> {
    let rows = state.audience.subscribers(1000).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| SubscriberResponse {
                id: r.id,
                email: r.email,
                status: r.status,
                created_at: r.created_at,
                confirmed_at: r.confirmed_at,
            })
            .collect(),
    ))
}

/// `DELETE /api/v1/audience/subscribers/{id}` — remove an address.
///
/// # Errors
/// 404 when missing, 403 without `ManageOptions`.
#[utoipa::path(
    delete, path = "/api/v1/audience/subscribers/{id}", tag = "audience",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Subscriber id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = ApiErrorBody),
    )
)]
pub async fn delete_subscriber(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    state.audience.delete_subscriber(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
