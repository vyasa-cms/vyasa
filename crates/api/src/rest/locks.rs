//! `POST /api/v1/posts/{id}/lock` and friends: who is editing.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::error::{ApiErrorBody, ApiResult};
use crate::locks::{self, LockState};
use crate::middleware::Principal;
use crate::policy;
use crate::state::AppState;

/// `force=true` takes the lock from whoever holds it.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct LockQuery {
    /// Take over a lock someone else holds.
    #[serde(default)]
    pub force: bool,
}

/// `POST /api/v1/posts/{id}/lock` — take or refresh the editing lock.
///
/// The lock is for those who may edit the entry: asking who holds it, and
/// taking it over, follow the editing rule.
#[utoipa::path(post, path = "/api/v1/posts/{id}/lock", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id"), LockQuery),
    responses(
        (status = 200, description = "Lock state", body = LockState),
        (status = 403, description = "Missing edit_posts / edit_others", body = ApiErrorBody),
        (status = 404, description = "Not found", body = ApiErrorBody),
    ))]
pub async fn lock(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Query(q): Query<LockQuery>,
) -> ApiResult<Json<LockState>> {
    policy::post_for_edit(&state, &principal, id).await?;
    let user = principal.user();
    let name = if user.display_name.trim().is_empty() {
        user.username.clone()
    } else {
        user.display_name.clone()
    };
    Ok(Json(
        locks::acquire(&state.pool, id, user.id, &name, q.force).await?,
    ))
}

/// `DELETE /api/v1/posts/{id}/lock` — let go when leaving the editor.
///
/// Releases the caller's own lock and nothing else, so there is nothing
/// to decide about the entry: whoever held the lock can let go even after
/// losing the right to edit, and releasing a lock you do not hold (or one
/// on an entry that does not exist) is a quiet 204.
#[utoipa::path(delete, path = "/api/v1/posts/{id}/lock", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses(
        (status = 204, description = "Released, or the caller held no lock"),
        (status = 403, description = "Missing edit_posts", body = ApiErrorBody),
    ))]
pub async fn unlock(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    locks::release(&state.pool, id, principal.user().id).await?;
    Ok(StatusCode::NO_CONTENT)
}
