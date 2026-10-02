//! Public registration (phase 98): whether it is open, registering,
//! confirming the address, and sending the confirmation again.
//!
//! [`vyasa_core::user::RegistrationService`] decides and stores; this
//! module answers the client, applies the limits and sends the mail each
//! outcome asks for. Every well-formed request is answered the same way
//! whatever the address turns out to be (new, confirmed, unconfirmed,
//! suspended), so the answer never says which addresses have accounts;
//! only the mailbox's owner sees the difference.
//!
//! ## Limits, and where they live
//!
//! All three are checked *before* the service is called, because both
//! `register` and `resend` write (a token, and for a contested repeat the
//! password's removal) whether or not a mail follows.
//!
//! - Per client address ([`crate::client_ip`]): five registrations and five
//!   re-send requests an hour (buckets [`rate_limit::REGISTER`],
//!   [`rate_limit::REGISTER_RESEND`] in the shared in-process limiter).
//!   Over it: `429 rate_limited`, which says nothing about any account.
//! - Per email address: three confirmation mails an hour, keyed by the
//!   SHA-256 of the address as `citext` compares it
//!   ([`rate_limit::address_key`]), in the same limiter
//!   ([`rate_limit::CONFIRMATION_MAIL`]). Every request for the address
//!   counts, whether or not it has an account; something that is not an
//!   address (over 254 bytes, no `@`) is answered alike and never counted.
//! - Per account: three links issued in the last hour, confirmation and
//!   password-reset links together, read from the token table, so links
//!   issued by another node or before a restart count too.
//!
//! `/auth/forgot` for an unconfirmed account goes through the same two
//! mail limits ([`may_mail`]), so it is not a way around them; and every
//! address, whatever its account, may ask `/auth/forgot` three times an
//! hour ([`rate_limit::RESET_MAIL`]).
//!
//! Over either mail limit the request is answered exactly as any other
//! (`202`, same body), after the same password-hashing work, and no
//! account, token or mail is made. The contest step still runs
//! ([`vyasa_core::user::RegistrationService::register_without_mail`]): a
//! pending account whose password the request does not match loses it,
//! so spending an address's mail budget first cannot keep a squatter's
//! password in place.
//!
//! The per-client and per-address buckets are in-process: each node of a
//! multi-node deployment counts on its own, and a restart forgets them
//! (the per-account count, in the database, does not). Anyone who knows
//! an address can spend its three confirmation mails an hour, which
//! delays the owner's registration by up to an hour; it cannot take the
//! account, because a password the owner did not choose never survives a
//! contested registration and the owner confirms from their own mailbox.
//!
//! ## Sign-in before confirmation
//!
//! An unconfirmed account's correct password is refused with the ordinary
//! `401` (same body, and it counts towards the lockout): anyone can
//! register an address and try the password they chose, so any other
//! answer would say whether the address was new.
//!
//! ## Confirming keeps only a password the mailbox's owner shows they know
//!
//! One registration by a stranger puts the stranger's password on the
//! address, and a link opened (by the owner, or by a mail scanner) proves
//! only that the mailbox was reached. So `POST /auth/verify` keeps the
//! stored password only when the request carries it again; without it the
//! address is confirmed, the password removed and a set-password link
//! mailed (the answer is the same `204`); a wrong one is `400
//! password_mismatch` and the link still works. The admin page never posts
//! the token by itself. A password reset through the mailbox confirms the
//! address too, and an administrator's confirmation removes the password
//! and mails the link.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_core::user::{NewRegistration, RegisterOutcome, ResendOutcome, VerifyOutcome};
use vyasa_db::models::{TokenPurpose, UserRow};

use crate::client_ip::ClientIp;
use crate::error::{ApiError, ApiErrorBody};
use crate::middleware::rate_limit;
use crate::state::AppState;

/// Confirmation links one account may be issued in an hour.
const LINKS_PER_ACCOUNT_PER_HOUR: i64 = 3;

/// What every well-formed register or re-send request is answered with.
const ACCEPTED: &str = "If this address can be registered, we have sent a message to it. \
                        Follow the link in it to finish; it works for 24 hours.";

/// A handler's answer: the success value, or a finished error response
/// (some of the codes here are not [`AppError`] codes).
type Answer<T> = Result<T, Response>;

fn refusal(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(ApiErrorBody {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
    )
        .into_response()
}

fn api(err: impl Into<ApiError>) -> Response {
    err.into().into_response()
}

fn closed() -> Response {
    refusal(
        StatusCode::FORBIDDEN,
        "registration_closed",
        "this site does not take registrations",
    )
}

/// Refuses unless registration is on and can work: a mail relay to send
/// the confirmation and a site address to put in its link. Refused
/// before anything is read about the request, so nothing is created.
async fn ensure_open(state: &AppState) -> Answer<()> {
    if !state.registration.status().await.map_err(api)?.enabled {
        return Err(closed());
    }
    if !ready_to_mail(state).await {
        return Err(refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "registration_unavailable",
            "registration is not available right now; try again later",
        ));
    }
    Ok(())
}

/// A mail relay is configured and `site_url` is set.
pub(crate) async fn ready_to_mail(state: &AppState) -> bool {
    crate::mail::effective(state).await.is_some()
        && crate::rest::auth::security_link_base(state).await.is_ok()
}

/// What `GET /auth/registration` answers.
#[derive(Serialize, utoipa::ToSchema)]
pub struct RegistrationInfo {
    /// Visitors may register (`registration_enabled`).
    pub enabled: bool,
    /// Registration is on *and* can work now (a mail relay and `site_url`
    /// are configured): what decides whether to offer the form. Enabled
    /// but not available, every registration is refused with `503`.
    pub available: bool,
    /// The shortest password accepted.
    pub password_min_length: usize,
}

/// `GET /api/v1/auth/registration` — whether to offer the form.
#[utoipa::path(get, path = "/api/v1/auth/registration", tag = "auth",
    responses((status = 200, description = "Whether registration is open", body = RegistrationInfo)))]
pub async fn status(State(state): State<AppState>) -> Result<Json<RegistrationInfo>, ApiError> {
    let status = state.registration.status().await?;
    // Readiness is only looked at (and so only said) once it is on.
    let available = status.enabled && ready_to_mail(&state).await;
    Ok(Json(RegistrationInfo {
        enabled: status.enabled,
        available,
        password_min_length: status.password_min_length,
    }))
}

/// Body of `POST /auth/register`. There is no username: one is
/// generated from the display name.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct RegisterRequest {
    /// The address to confirm; case and surrounding space do not matter.
    pub email: String,
    /// Shown on the account; at most 100 characters.
    #[serde(default)]
    pub display_name: Option<String>,
    /// At least `password_min_length` characters.
    pub password: String,
    /// Leave empty. A form hides this field from people; whatever fills
    /// it in is taken for a robot, thanked, and ignored.
    #[serde(default)]
    pub website: Option<String>,
}

/// The answer to every well-formed register or re-send request.
#[derive(Serialize, utoipa::ToSchema)]
pub struct Accepted {
    /// Always the same sentence.
    pub message: String,
}

fn accepted() -> (StatusCode, Json<Accepted>) {
    (
        StatusCode::ACCEPTED,
        Json(Accepted {
            message: ACCEPTED.to_owned(),
        }),
    )
}

/// `POST /api/v1/auth/register` — register an account, confirmed by mail.
#[utoipa::path(post, path = "/api/v1/auth/register", tag = "auth", request_body = RegisterRequest,
    responses(
        (status = 202, description = "Always, for a well-formed request, whether or not the address \
            has an account", body = Accepted),
        (status = 400, description = "Malformed address, display name or password", body = ApiErrorBody),
        (status = 403, description = "`registration_closed`", body = ApiErrorBody),
        (status = 429, description = "`rate_limited`: this client address registered too often", body = ApiErrorBody),
        (status = 503, description = "`registration_unavailable`: no mail relay or site address", body = ApiErrorBody),
    ))]
pub async fn register(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
    Json(body): Json<RegisterRequest>,
) -> Answer<(StatusCode, Json<Accepted>)> {
    ensure_open(&state).await?;
    let input = NewRegistration {
        email: body.email,
        display_name: body.display_name,
        password: body.password,
    };
    let email = vyasa_core::user::validate_registration(&input).map_err(api)?;
    if !state.rate_limiter.check(rate_limit::REGISTER, &client_ip) {
        tracing::info!(
            limit = rate_limit::REGISTER,
            client = %client_ip,
            "registration refused: over the per-client limit"
        );
        return Err(api(AppError::rate_limited(
            "too many registrations from here; try again in an hour",
        )));
    }
    let robot = body
        .website
        .as_deref()
        .is_some_and(|w| !w.trim().is_empty());
    if robot {
        // The same work as a real registration, so the answer takes as
        // long; nothing is written.
        drop(vyasa_core::user::password::hash_password(&input.password));
        return Ok(accepted());
    }
    if !may_mail(&state, &email).await {
        // No token and no mail, but the contest still runs: a pending
        // account whose password this request does not match loses it.
        return match state.registration.register_without_mail(input).await {
            Ok(()) => Ok(accepted()),
            Err(AppError::Forbidden { .. }) => Err(closed()),
            Err(err) => Err(api(err)),
        };
    }
    match state.registration.register(input).await {
        Ok(outcome) => owe_mail(&state, outcome).await,
        // Closed between the check above and now.
        Err(AppError::Forbidden { .. }) => return Err(closed()),
        Err(err) => return Err(api(err)),
    }
    Ok(accepted())
}

/// Sends what a registration's outcome asks for. A failure to queue is
/// logged, never answered: the client's answer must not differ.
async fn owe_mail(state: &AppState, outcome: RegisterOutcome) {
    let sent = match outcome {
        RegisterOutcome::Created { user, token } => {
            crate::audit::record(
                state,
                &user,
                "user.register",
                format!("user:{}", user.id),
                serde_json::json!({
                    "role": user.role.as_str(),
                    "custom_role": user.custom_role,
                }),
            );
            crate::plugin_hooks::emit(
                state,
                crate::plugin_hooks::events::USER_REGISTERED,
                crate::plugin_hooks::account_payload(&user),
            );
            send_confirmation(state, &user, &token, false).await
        }
        RegisterOutcome::ConfirmationResent {
            user,
            token,
            password_cleared,
        } => send_confirmation(state, &user, &token, password_cleared).await,
        RegisterOutcome::AlreadyRegistered { user } => send_existing(state, &user).await,
    };
    if let Err(err) = sent {
        tracing::warn!("registration mail could not be queued: {err}");
    }
}

/// Whether the address may be sent another registration mail this hour:
/// under the per-address count (every request for it counts, under
/// [`rate_limit::address_key`]) and under the per-account count of links
/// issued -- confirmation and password-reset links alike, so
/// `/auth/forgot` on an unconfirmed account shares the budget. See the
/// module documentation. Never for something that is not an address,
/// which is not counted either. A refusal is logged without the address.
pub(crate) async fn may_mail(state: &AppState, email: &str) -> bool {
    let Some(key) = rate_limit::address_key(email) else {
        return false;
    };
    if !state
        .rate_limiter
        .check(rate_limit::CONFIRMATION_MAIL, &key)
    {
        tracing::info!(
            limit = rate_limit::CONFIRMATION_MAIL,
            "mail suppressed: over the per-address limit"
        );
        return false;
    }
    let account = match vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .get_by_email(email.trim())
        .await
    {
        Ok(user) => user,
        Err(AppError::NotFound { .. }) => return true,
        Err(err) => {
            tracing::warn!("registration: account lookup failed: {err}");
            return false;
        }
    };
    let tokens = vyasa_db::repo::TokensRepo::new(state.pool.clone());
    let mut issued = 0;
    for purpose in [TokenPurpose::Verify, TokenPurpose::Reset] {
        match tokens
            .issued_since(account.id, purpose, chrono::Duration::hours(1))
            .await
        {
            Ok(count) => issued += count,
            Err(err) => {
                tracing::warn!("registration: link count failed: {err}");
                return false;
            }
        }
    }
    if issued >= LINKS_PER_ACCOUNT_PER_HOUR {
        tracing::info!(
            user_id = account.id,
            limit = "links-per-account",
            "mail suppressed: over the per-account limit"
        );
        return false;
    }
    true
}

/// Mails `user` a confirmation link carrying `token`. The link is built
/// from `site_url` alone.
///
/// # Errors
/// No site address; the queue.
pub(crate) async fn send_confirmation(
    state: &AppState,
    user: &UserRow,
    token: &str,
    password_cleared: bool,
) -> Result<(), AppError> {
    let base = crate::rest::auth::security_link_base(state).await?;
    queue(
        state,
        &user.email,
        "registration_confirm",
        &serde_json::json!({
            // No name: the display name was typed by whoever registered,
            // who may not own this mailbox.
            "site": crate::rest::auth::site_name(state).await,
            "url": format!("{base}/admin/verify?token={token}"),
            "password_cleared": password_cleared,
        }),
    )
    .await
}

/// Mails the owner of an existing account that someone tried to register
/// its address, with the ways back in.
async fn send_existing(state: &AppState, user: &UserRow) -> Result<(), AppError> {
    let base = crate::rest::auth::security_link_base(state).await?;
    queue(
        state,
        &user.email,
        "registration_existing",
        &serde_json::json!({
            // No name: it may have been typed by whoever registered first.
            "site": crate::rest::auth::site_name(state).await,
            "sign_in_url": format!("{base}/admin/login"),
            "forgot_url": format!("{base}/admin/forgot"),
        }),
    )
    .await
}

async fn queue(
    state: &AppState,
    to: &str,
    template: &str,
    ctx: &serde_json::Value,
) -> Result<(), AppError> {
    vyasa_core::notify::EmailService::new(state.pool.clone())
        .queue(to, template, ctx)
        .await?;
    state.metrics.inc_email_queued();
    state.publisher_notify.notify_one();
    Ok(())
}

/// Body of `POST /auth/verify`.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct VerifyRequest {
    /// The token from the confirmation link.
    pub token: String,
    /// The password chosen when signing up. Given and right: the address
    /// is confirmed and the password kept. Given and wrong:
    /// `400 password_mismatch`, nothing changes and the link still works.
    /// Left out: the address is confirmed, any stored password is removed
    /// (it may have been typed by someone who does not own the mailbox),
    /// and a link to set one is mailed.
    #[serde(default)]
    pub password: Option<String>,
}

/// `POST /api/v1/auth/verify` — confirm an address with the mailed link.
///
/// Only a password the link's holder shows they know survives: without
/// `password`, or for an account whose password a contest removed, the
/// account is mailed a set-password link at once. The answer is the same
/// `204` whether the password was kept or removed.
#[utoipa::path(post, path = "/api/v1/auth/verify", tag = "auth", request_body = VerifyRequest,
    responses(
        (status = 204, description = "Confirmed (the password kept, or removed and a set-password \
            link mailed)"),
        (status = 400, description = "`password_mismatch`: not the password signed up with (the \
            link still works); otherwise the link is invalid, used or expired", body = ApiErrorBody),
    ))]
pub async fn verify(
    State(state): State<AppState>,
    Json(body): Json<VerifyRequest>,
) -> Answer<StatusCode> {
    let outcome = state
        .registration
        .verify(body.token.trim(), body.password.as_deref())
        .await
        .map_err(api)?;
    let VerifyOutcome::Confirmed {
        user,
        needs_password,
    } = outcome
    else {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "password_mismatch",
            "that is not the password this sign-up was made with",
        ));
    };
    crate::audit::record(
        &state,
        &user,
        "user.confirm",
        format!("user:{}", user.id),
        serde_json::json!({ "by": "link", "password_kept": !needs_password }),
    );
    if needs_password {
        match crate::rest::auth::send_set_password_link(&state, &user, None).await {
            Ok(_) => tracing::info!(user_id = user.id, "set-password link queued"),
            Err(err) => tracing::warn!(user_id = user.id, "set-password link not queued: {err}"),
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Body of `POST /auth/register/resend`.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ResendRequest {
    /// The address registered with.
    pub email: String,
}

/// `POST /api/v1/auth/register/resend` — send the confirmation again.
#[utoipa::path(post, path = "/api/v1/auth/register/resend", tag = "auth", request_body = ResendRequest,
    responses(
        (status = 202, description = "Always, whether or not the address has an account waiting \
            for confirmation", body = Accepted),
        (status = 403, description = "`registration_closed`", body = ApiErrorBody),
        (status = 429, description = "`rate_limited`: this client address asked too often", body = ApiErrorBody),
        (status = 503, description = "`registration_unavailable`", body = ApiErrorBody),
    ))]
pub async fn resend(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
    Json(body): Json<ResendRequest>,
) -> Answer<(StatusCode, Json<Accepted>)> {
    ensure_open(&state).await?;
    if !state
        .rate_limiter
        .check(rate_limit::REGISTER_RESEND, &client_ip)
    {
        tracing::info!(
            limit = rate_limit::REGISTER_RESEND,
            client = %client_ip,
            "resend refused: over the per-client limit"
        );
        return Err(api(AppError::rate_limited(
            "too many requests from here; try again in an hour",
        )));
    }
    if !may_mail(&state, &body.email).await {
        return Ok(accepted());
    }
    match state.registration.resend(&body.email).await.map_err(api)? {
        ResendOutcome::Send {
            user,
            token,
            needs_password,
        } => {
            if let Err(err) = send_confirmation(&state, &user, &token, needs_password).await {
                tracing::warn!(user_id = user.id, "confirmation mail not queued: {err}");
            }
        }
        ResendOutcome::Nothing => {}
    }
    Ok(accepted())
}
