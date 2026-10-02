//! `/api/v1/patterns`: saved arrangements of blocks for the editor.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;

use crate::error::ApiResult;
use crate::middleware::Principal;
use crate::patterns::{self, PatternInput, PatternRow};
use crate::state::AppState;

/// `GET /api/v1/patterns`
#[utoipa::path(get, path = "/api/v1/patterns", tag = "content",
    responses((status = 200, description = "Every pattern", body = [PatternRow])))]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<PatternRow>>> {
    Ok(Json(patterns::list(&state).await?))
}

/// `GET /api/v1/patterns/{id}`
#[utoipa::path(get, path = "/api/v1/patterns/{id}", tag = "content",
    responses((status = 200, description = "The pattern", body = PatternRow)))]
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<PatternRow>> {
    Ok(Json(patterns::get(&state, id).await?))
}

/// `POST /api/v1/patterns`
#[utoipa::path(post, path = "/api/v1/patterns", tag = "content", request_body = PatternInput,
    responses((status = 201, description = "Created", body = PatternRow)))]
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(input): Json<PatternInput>,
) -> ApiResult<(StatusCode, Json<PatternRow>)> {
    let row = patterns::create(&state, principal.user().id, input).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// `PUT /api/v1/patterns/{id}` — editing a synced pattern changes every
/// page that uses it; the render cache is bumped.
#[utoipa::path(put, path = "/api/v1/patterns/{id}", tag = "content", request_body = PatternInput,
    responses((status = 200, description = "Updated", body = PatternRow)))]
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<PatternInput>,
) -> ApiResult<Json<PatternRow>> {
    let row = patterns::update(&state, id, input).await?;
    state
        .surface_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(Json(row))
}

/// `DELETE /api/v1/patterns/{id}` — documents that referenced a synced
/// pattern render nothing where it stood.
#[utoipa::path(delete, path = "/api/v1/patterns/{id}", tag = "content",
    responses((status = 204, description = "Removed")))]
pub async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    patterns::remove(&state, id).await?;
    state
        .surface_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(StatusCode::NO_CONTENT)
}
