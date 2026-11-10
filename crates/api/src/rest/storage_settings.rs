//! `/api/v1/media/storage*`: where media bytes go, from the admin panel.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::error::ApiResult;
use crate::media_storage::{self, MigrationProgress, StorageInput, StorageSettings};
use crate::middleware::Principal;
use crate::policy;
use crate::state::AppState;

/// `GET /api/v1/media/storage`
#[utoipa::path(get, path = "/api/v1/media/storage", tag = "media",
    responses((status = 200, description = "The active store, without its secret", body = StorageSettings)))]
pub async fn get(State(state): State<AppState>) -> ApiResult<Json<StorageSettings>> {
    Ok(Json(media_storage::settings(&state).await))
}

/// `PUT /api/v1/media/storage` — a full administrator only: the keys
/// reach every byte the site serves.
#[utoipa::path(put, path = "/api/v1/media/storage", tag = "media", request_body = StorageInput,
    responses((status = 200, description = "Saved and active", body = StorageSettings),
              (status = 400, description = "The store refused the probe"),
              (status = 403, description = "Not a full administrator"),
              (status = 409, description = "Set in the environment, or files would be forgotten")))]
pub async fn put(
    State(state): State<AppState>,
    principal: Principal,
    Json(input): Json<StorageInput>,
) -> ApiResult<Json<StorageSettings>> {
    policy::full_administrator(&principal, "change media storage")?;
    Ok(Json(media_storage::save(&state, input).await?))
}

/// `POST /api/v1/media/storage/test` — one probe object, nothing saved.
#[utoipa::path(post, path = "/api/v1/media/storage/test", tag = "media", request_body = StorageInput,
    responses((status = 204, description = "Write, read and delete succeeded"),
              (status = 400, description = "The store refused"),
              (status = 403, description = "Not a full administrator")))]
pub async fn test(
    State(state): State<AppState>,
    principal: Principal,
    Json(input): Json<StorageInput>,
) -> ApiResult<StatusCode> {
    policy::full_administrator(&principal, "test media storage")?;
    media_storage::test(&state, &input).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/media/storage/migrate` — move files that sit in the
/// store that is not active.
#[utoipa::path(post, path = "/api/v1/media/storage/migrate", tag = "media",
    responses((status = 202, description = "Started", body = MigrationProgress),
              (status = 403, description = "Not a full administrator"),
              (status = 409, description = "Already running")))]
pub async fn migrate(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<(StatusCode, Json<MigrationProgress>)> {
    policy::full_administrator(&principal, "move media between stores")?;
    let progress = media_storage::start_migration(&state).await?;
    Ok((StatusCode::ACCEPTED, Json(progress)))
}
