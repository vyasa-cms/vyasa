//! `/api/v1/mail/*`: the relay, from the admin panel.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::error::ApiResult;
use crate::mail::{self, MailInput, MailSettings};
use crate::middleware::Principal;
use crate::policy;
use crate::state::AppState;

/// `GET /api/v1/mail/settings`
#[utoipa::path(get, path = "/api/v1/mail/settings", tag = "mail",
    responses((status = 200, description = "The relay, without its password", body = MailSettings)))]
pub async fn get(State(state): State<AppState>) -> ApiResult<Json<MailSettings>> {
    Ok(Json(mail::settings(&state).await))
}

/// `PUT /api/v1/mail/settings` — a full administrator only: the relay is
/// where password-reset links are sent through.
#[utoipa::path(put, path = "/api/v1/mail/settings", tag = "mail", request_body = MailInput,
    responses((status = 200, description = "Saved", body = MailSettings),
              (status = 403, description = "Not a full administrator")))]
pub async fn put(
    State(state): State<AppState>,
    principal: Principal,
    Json(input): Json<MailInput>,
) -> ApiResult<Json<MailSettings>> {
    policy::full_administrator(&principal, "change the mail relay")?;
    Ok(Json(mail::save(&state, input).await?))
}

/// Who receives the test message.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct TestBody {
    pub to: String,
}

/// `POST /api/v1/mail/test` — one message through the relay, now, to any
/// address: a full administrator only.
#[utoipa::path(post, path = "/api/v1/mail/test", tag = "mail", request_body = TestBody,
    responses((status = 204, description = "Handed to the relay"), (status = 400, description = "No relay, or it refused"),
              (status = 403, description = "Not a full administrator")))]
pub async fn test(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<TestBody>,
) -> ApiResult<StatusCode> {
    policy::full_administrator(&principal, "send a test message through the mail relay")?;
    mail::send_test(&state, body.to.trim()).await?;
    Ok(StatusCode::NO_CONTENT)
}
