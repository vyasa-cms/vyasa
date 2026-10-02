//! Authentication endpoints: login, logout, me.

use axum::extract::State;
use axum::http::header::SET_COOKIE;
use axum::http::StatusCode;
use axum::http::{HeaderMap, HeaderValue};
use axum::Json;
use serde::Deserialize;

use super::UserResponse;

use crate::client_ip::ClientIp;
use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::{CurrentUser, SESSION_COOKIE};
use crate::state::AppState;
use sha2::{Digest as _, Sha256};
use vyasa_common::AppError;

/// Request body for `POST /api/v1/auth/login`.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    /// Account email.
    pub email: String,
    /// Account password.
    pub password: String,
    /// Second-factor code consumed by auth:challenge plugins (phase 36).
    #[serde(default)]
    pub extra_code: Option<String>,
}

/// `POST /api/v1/auth/login` — verify credentials, set session cookie.
///
/// # Errors
///
/// 401 with code `unauthorized` for bad credentials, and for the right
/// password on an account whose address is not confirmed yet (the same
/// answer, and the same count towards the lockout).
#[utoipa::path(
    post, path = "/api/v1/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in; session cookie set", body = UserResponse),
        (status = 401, description = "Invalid credentials", body = ApiErrorBody),
    )
)]
pub async fn login(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> ApiResult<(HeaderMap, Json<UserResponse>)> {
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // auth:challenge stage (phase 36): plugins may demand a second factor.
    //
    // This used to test `CommentBody` for emptiness and then dispatch on
    // `SeoTitle`, where nothing is ever registered — so the filter returned
    // its input unchanged and no plugin could ever require a second factor.
    if !state
        .hook_registry
        .is_empty(vyasa_plugins::hooks::HookPoint::AuthChallenge)
        .await
    {
        let challenge = state
            .hook_registry
            .dispatch_filter(
                vyasa_plugins::hooks::HookPoint::AuthChallenge,
                serde_json::json!({
                    "stage": "auth:challenge",
                    "email": body.email,
                    "code": body.extra_code,
                })
                .to_string(),
                &ChallengeExecutor { state: &state },
                &DegradedLogger,
            )
            .await;
        // A plugin answers with JSON {"challenge_required":true,...};
        // anything else passes through.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&challenge) {
            if v.get("challenge_required")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                return Err(ApiError(vyasa_common::AppError::auth("challenge required")));
            }
        }
    }

    // Progressive lockout: the per-IP limiter alone lets a distributed
    // attempt grind one account indefinitely, since it resets every window.
    // Every attempt counts, under the address's fixed-size digest whatever
    // was typed: malformed input is an unknown address, and an account
    // whose address registration would refuse is protected all the same.
    if let Some(wait) = state.lockout.check(&body.email, &client_ip) {
        return Err(ApiError(AppError::auth(format!(
            "too many failed attempts; try again in {}s",
            wait.as_secs().max(1)
        ))));
    }

    let session = match state
        .auth
        .login(&body.email, &body.password, user_agent.as_deref(), None)
        .await
    {
        Ok(session) => {
            state.lockout.record_success(&body.email, &client_ip);
            session
        }
        Err(err) => {
            // An unconfirmed account is refused exactly as a wrong password
            // is, lockout included. Anyone can register an address and then
            // try to sign in with the password they chose: a distinct
            // answer, or a lockout that did not count it, would say whether
            // the address was new. The reason is for operators only.
            let err = if matches!(err, AppError::EmailUnconfirmed) {
                tracing::info!(reason = "email_unconfirmed", "sign-in refused");
                AppError::auth(vyasa_core::user::INVALID_CREDENTIALS)
            } else {
                err
            };
            state.lockout.record_failure(&body.email, &client_ip);
            // Deliberately carries no identifier. The address tried is
            // personal data, and events are delivered to every installed
            // plugin without any capability being declared — so putting it
            // here would hand a plugin the site's sign-in traffic for free.
            // A plugin counting failures is what the built-in progressive
            // lockout above already does.
            crate::plugin_hooks::emit(
                &state,
                crate::plugin_hooks::events::LOGIN_FAILED,
                serde_json::json!({}),
            );
            return Err(ApiError(err));
        }
    };
    // A second factor: the password step passed, so hand back a short
    // challenge instead of the session it just made, and end that session.
    if let Some(challenge) = login_mfa_challenge(&state, session.user.id, &session.token).await? {
        return Err(ApiError(AppError::MfaRequired { challenge }));
    }
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::LOGIN_SUCCEEDED,
        crate::plugin_hooks::account_payload(&session.user),
    );

    // `bind_addr` is `127.0.0.1:3000`, never a URL, so the old test
    // against "https://" was never true and the session cookie went out
    // without `Secure` on every deployment. What actually says whether the
    // visitor arrived over TLS is the site's address, or failing that the
    // proxy's forwarded scheme.
    let secure = if cookie_origin(&state, &headers)
        .await
        .starts_with("https://")
    {
        "; Secure"
    } else {
        ""
    };
    // HttpOnly keeps the token out of JS; SameSite=Lax blocks CSRF on
    // cross-site POSTs while allowing normal top-level navigation.
    let cookie = format!(
        "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age=1209600",
        session.token
    );
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|err| {
            vyasa_common::AppError::internal_msg(format!("invalid cookie header: {err}"))
        })?,
    );
    Ok((
        response_headers,
        Json(UserResponse::from_row(&session.user)),
    ))
}

// The password service has already issued a session. Revoke it whenever
// a second factor is required or its state cannot be determined.
async fn login_mfa_challenge(
    state: &AppState,
    user_id: i64,
    token: &str,
) -> Result<Option<String>, AppError> {
    let enabled = crate::mfa::enabled(state, user_id).await;
    if !matches!(enabled, Ok(false)) {
        let _ = state.auth.logout(token).await;
    }
    Ok(enabled?.then(|| crate::mfa::challenge(state, user_id)))
}

/// The second step of a sign-in with a second factor.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct MfaLoginRequest {
    /// From the 202 the password step answered with.
    pub challenge: String,
    /// A code from the authenticator app, or a recovery code.
    pub code: String,
}

/// `POST /api/v1/auth/mfa` — finish a sign-in with a code.
#[utoipa::path(post, path = "/api/v1/auth/mfa", tag = "auth", request_body = MfaLoginRequest,
    responses((status = 200, description = "Signed in", body = UserResponse),
              (status = 401, description = "Wrong code or stale challenge", body = ApiErrorBody)))]
pub async fn mfa_login(
    State(state): State<AppState>,
    ClientIp(client_ip): ClientIp,
    headers: HeaderMap,
    Json(body): Json<MfaLoginRequest>,
) -> ApiResult<(HeaderMap, Json<UserResponse>)> {
    let user_id = crate::mfa::open_challenge(&state, &body.challenge).map_err(ApiError)?;
    let user = state.users.get(user_id).await?;
    // The challenge outlives the password check by minutes; a suspension
    // in between must still keep the account out, as the password step does.
    if user.suspended_at.is_some() {
        return Err(ApiError(AppError::auth(
            "this account is suspended; ask an administrator",
        )));
    }
    // Likewise an address that stopped being confirmed in the meantime,
    // with this step's generic answer (as for a stale challenge).
    if vyasa_core::user::ensure_confirmed(&user).is_err() {
        tracing::info!(reason = "email_unconfirmed", "second step refused");
        state.lockout.record_failure(&user.email, &client_ip);
        return Err(ApiError(AppError::auth("bad challenge")));
    }
    if let Some(wait) = state.lockout.check(&user.email, &client_ip) {
        return Err(ApiError(AppError::auth(format!(
            "too many failed attempts; try again in {}s",
            wait.as_secs().max(1)
        ))));
    }
    if let Err(err) = crate::mfa::verify(&state, user.id, &user.email, &body.code).await {
        state.lockout.record_failure(&user.email, &client_ip);
        return Err(ApiError(err));
    }
    state.lockout.record_success(&user.email, &client_ip);
    let token = crate::mfa::session_token();
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    vyasa_db::repo::SessionsRepo::new(state.pool.clone())
        .insert(
            &token,
            user.id,
            vyasa_core::user::SESSION_TTL,
            user_agent,
            None,
        )
        .await?;
    let _ = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .touch_login(user.id)
        .await;
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::LOGIN_SUCCEEDED,
        crate::plugin_hooks::account_payload(&user),
    );
    let secure = if cookie_origin(&state, &headers)
        .await
        .starts_with("https://")
    {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age=1209600"
    );
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie)
            .map_err(|err| AppError::internal_msg(format!("invalid cookie header: {err}")))?,
    );
    Ok((response_headers, Json(UserResponse::from_row(&user))))
}

/// A code, for confirm and disable.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct MfaCode {
    pub code: String,
}

/// `GET /api/v1/auth/mfa/status` — where your own second factor stands.
#[utoipa::path(get, path = "/api/v1/auth/mfa/status", tag = "auth",
    responses((status = 200, description = "Status", body = crate::mfa::MfaStatus)))]
pub async fn mfa_status(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<crate::mfa::MfaStatus>> {
    Ok(Json(crate::mfa::status(&state, user.user.id).await?))
}

/// `POST /api/v1/auth/mfa/setup` — a fresh secret to scan; not on yet.
#[utoipa::path(post, path = "/api/v1/auth/mfa/setup", tag = "auth",
    responses((status = 200, description = "Scan this", body = crate::mfa::MfaSetup)))]
pub async fn mfa_setup(
    State(state): State<AppState>,
    user: CurrentUser,
) -> ApiResult<Json<crate::mfa::MfaSetup>> {
    Ok(Json(
        crate::mfa::begin(&state, user.user.id, &user.user.email).await?,
    ))
}

/// `POST /api/v1/auth/mfa/confirm` — the first code turns it on; the
/// recovery codes come back once.
#[utoipa::path(post, path = "/api/v1/auth/mfa/confirm", tag = "auth", request_body = MfaCode,
    responses((status = 200, description = "On; keep these", body = crate::mfa::RecoveryCodes)))]
pub async fn mfa_confirm(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<MfaCode>,
) -> ApiResult<Json<crate::mfa::RecoveryCodes>> {
    Ok(Json(
        crate::mfa::confirm(&state, user.user.id, &user.user.email, &body.code).await?,
    ))
}

/// `POST /api/v1/auth/mfa/disable` — one last valid code turns it off.
#[utoipa::path(post, path = "/api/v1/auth/mfa/disable", tag = "auth", request_body = MfaCode,
    responses((status = 204, description = "Off")))]
pub async fn mfa_disable(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<MfaCode>,
) -> ApiResult<StatusCode> {
    crate::mfa::disable(&state, user.user.id, &user.user.email, &body.code).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/auth/logout` — revoke the session, clear the cookie.
///
/// # Errors
///
/// 401 when not authenticated.
#[utoipa::path(
    post, path = "/api/v1/auth/logout",
    tag = "auth",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Session revoked"),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn logout(State(state): State<AppState>, user: CurrentUser) -> ApiResult<HeaderMap> {
    state.auth.logout(&user.token).await?;
    let mut response_headers = HeaderMap::new();
    let clear = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
    response_headers.insert(
        SET_COOKIE,
        HeaderValue::from_str(&clear).map_err(|err| {
            vyasa_common::AppError::internal_msg(format!("invalid cookie header: {err}"))
        })?,
    );
    Ok(response_headers)
}

/// `GET /api/v1/auth/me` — current user.
///
/// # Errors
///
/// 401 when not authenticated.
#[utoipa::path(
    get, path = "/api/v1/auth/me",
    tag = "auth",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Current user", body = UserResponse),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn me(user: CurrentUser) -> ApiResult<Json<UserResponse>> {
    Ok(Json(UserResponse::from_row(&user.user)))
}

/// Runs the auth:challenge filter through the plugin host.
struct ChallengeExecutor<'a> {
    state: &'a AppState,
}

impl vyasa_plugins::hooks::FilterExecutor for ChallengeExecutor<'_> {
    fn invoke(
        &self,
        plugin_id: i64,
        _hook: vyasa_plugins::hooks::HookPoint,
        content: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>> {
        let broker = std::sync::Arc::clone(&self.state.broker);
        let dest = self.state.pool.clone();
        let host = std::sync::Arc::clone(&self.state.plugin_host);
        Box::pin(async move {
            let env = vyasa_plugins::host::HostEnv::background(broker, dest);
            match host
                .invoke_filter(
                    plugin_id,
                    vyasa_plugins::host::FilterStage::Content,
                    content,
                    &env,
                )
                .await
            {
                vyasa_plugins::host::InvokeOutcome::Ok(s) => Ok(s),
                vyasa_plugins::host::InvokeOutcome::Failed(r) => Err(r),
            }
        })
    }
}

/// Logs degraded auth-challenge plugins without touching the response.
struct DegradedLogger;

impl vyasa_plugins::hooks::DegradedSink for DegradedLogger {
    fn mark_degraded(&self, plugin_id: i64, reason: &str) {
        tracing::warn!(plugin_id, "auth:challenge plugin degraded: {reason}");
    }
}

/// A password-reset token nobody can guess.
///
/// It used to be the SHA-256 of `reset:{snowflake id}:{email}`. A snowflake
/// is a timestamp, a worker number and a counter — anyone who knows the
/// address and roughly when the request was made can enumerate the few
/// thousand candidates and take the account. The hash added nothing:
/// hashing a predictable value gives a predictable hash. 256 bits from the
/// operating system's generator is the whole of the fix.
fn fresh_reset_token() -> String {
    use rand::RngCore as _;
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    format!("vy-{}", hex::encode(buf))
}

/// Emails `target` a link that sets a password: the invitation when an
/// administrator (`inviter`) sends it to an account that has never had
/// one, a plain "set a password" note when nobody did (the account's owner
/// asked, or confirmed a registration whose password was dropped), a
/// reset otherwise. Returns which: `invitation`, `set_password`, `reset`.
///
/// # Errors
/// [`AppError::Validation`] when no mail relay or no site address is
/// configured, so the administrator learns the link went nowhere; database
/// errors.
pub async fn send_set_password_link(
    state: &AppState,
    target: &vyasa_db::models::UserRow,
    inviter: Option<&vyasa_db::models::UserRow>,
) -> Result<&'static str, AppError> {
    if crate::mail::effective(state).await.is_none() {
        return Err(AppError::validation(
            "no mail relay is configured, so the link cannot be sent; add one under Settings",
        ));
    }
    let base = security_link_base(state).await?;
    let raw_token = fresh_reset_token();
    let token_hash = hex::encode(Sha256::digest(&raw_token));
    vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .create_reset_token(target.id, &token_hash)
        .await?;
    let site = site_name(state).await;
    let never_had_password = target
        .password_hash
        .as_deref()
        .unwrap_or_default()
        .is_empty();
    let kind = match (never_had_password, inviter) {
        (true, Some(_)) => "invitation",
        (true, None) => "set_password",
        (false, _) => "reset",
    };
    let (template, ctx) = if kind == "set_password" {
        (
            "password_set",
            // No name: a registrant's display name may be a stranger's.
            serde_json::json!({
                "site": site,
                "url": format!("{base}/admin/reset?token={raw_token}&welcome=1"),
            }),
        )
    } else if kind == "invitation" {
        (
            "user_invited",
            serde_json::json!({
                "site": site,
                "inviter": inviter.map_or("An administrator", |u| u.display_name.as_str()),
                "role": target
                    .custom_role_name
                    .as_deref()
                    .unwrap_or(target.role.as_str()),
                "username": target.username,
                "url": format!("{base}/admin/reset?token={raw_token}&welcome=1"),
            }),
        )
    } else {
        (
            "password_reset",
            // No name: an account a registration created carries the name
            // whoever registered first typed, who may not own the mailbox.
            serde_json::json!({
                "site": site,
                "url": format!("{base}/admin/reset?token={raw_token}"),
            }),
        )
    };
    vyasa_core::notify::EmailService::new(state.pool.clone())
        .queue(&target.email, template, &ctx)
        .await?;
    state.metrics.inc_email_queued();
    state.publisher_notify.notify_one();
    Ok(kind)
}

/// The site's name for mail: its title, or `Vyasa` without one.
pub(crate) async fn site_name(state: &AppState) -> String {
    state
        .options_service
        .site_identity()
        .await
        .ok()
        .map(|i| i.title)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| String::from("Vyasa"))
}

/// `POST /api/v1/auth/forgot` — mail a password-reset link. Always 200,
/// whatever the address and whatever was sent.
///
/// Every address may ask three times an hour
/// ([`crate::middleware::rate_limit::RESET_MAIL`]), counted whether or not
/// it has an account, under a fixed-size key
/// ([`crate::middleware::rate_limit::address_key`]); something that is not
/// an address is answered the same and not counted. An unconfirmed account
/// is mailed only within the registration mail limits
/// ([`crate::rest::registration::may_mail`]): anyone can register an
/// address, so this route would otherwise be a way to mail it as often as
/// they liked.
#[utoipa::path(
    post, path = "/api/v1/auth/forgot", tag = "auth",
    request_body(content = serde_json::Value),
    responses((status = 200, description = "Always 200"))
)]
pub async fn forgot(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<StatusCode> {
    let email = body["email"].as_str().unwrap_or_default().trim();
    // Not an address (empty, too long, no `@`): nothing to mail, and
    // nothing to count -- the limiter would otherwise hold whatever was
    // typed for an hour.
    let Some(key) = crate::middleware::rate_limit::address_key(email) else {
        return Ok(StatusCode::OK);
    };
    if !state
        .rate_limiter
        .check(crate::middleware::rate_limit::RESET_MAIL, &key)
    {
        tracing::info!(
            limit = crate::middleware::rate_limit::RESET_MAIL,
            "reset mail suppressed: over the per-address limit"
        );
        return Ok(StatusCode::OK);
    }
    // Look up the user; send only when the account exists. Still 200
    // either way: saying which addresses exist is what always-200 prevents.
    if let Ok(user) = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .get_by_email(email)
        .await
    {
        if user.email_verified_at.is_none()
            && !crate::rest::registration::may_mail(&state, email).await
        {
            return Ok(StatusCode::OK);
        }
        match send_set_password_link(&state, &user, None).await {
            Ok(_) => tracing::info!(user_id = user.id, "password reset email queued"),
            Err(err) => tracing::warn!(user_id = user.id, "reset email could not be queued: {err}"),
        }
    }
    Ok(StatusCode::OK)
}

/// The origin the visitor used, for deciding whether the session cookie
/// is `Secure`: the site's address, or failing that the request's own.
/// Only the visitor's own cookie depends on it, so a forged header gains
/// nothing.
async fn cookie_origin(state: &AppState, headers: &HeaderMap) -> String {
    match state.options_service.site_url().await {
        Ok(url) if !url.is_empty() => url,
        _ => crate::feeds::origin_from_headers(headers),
    }
}

/// The base of a link that sets a password. Only the configured
/// `site_url` will do: the request's `Host` and `X-Forwarded-*` are the
/// caller's to choose, and a reset asked for with a forged host mailed the
/// victim a genuine token on a link to the attacker's server.
///
/// # Errors
/// [`AppError::Validation`] when `site_url` is not configured.
pub(crate) async fn security_link_base(state: &AppState) -> Result<String, AppError> {
    match state.options_service.site_url().await {
        Ok(url) if !url.trim().is_empty() => Ok(url.trim().trim_end_matches('/').to_owned()),
        Ok(_) => Err(AppError::validation(
            "the site address is not configured, so a sign-in link cannot be mailed; \
             set it under Settings",
        )),
        Err(e) => Err(e),
    }
}

/// `POST /api/v1/auth/reset` — consume a token and set a new password.
///
/// The link was mailed to the account's address, so following it proves
/// the mailbox: an account whose address was not confirmed yet is
/// confirmed by it, with the password just chosen.
///
/// The token is looked at first, without spending it: a dead one is
/// refused before the password is checked or hashed, so a request with a
/// made-up token costs a lookup, not an argon2 hash. It is spent, in the
/// same transaction as the password is set, only after both.
#[utoipa::path(
    post, path = "/api/v1/auth/reset", tag = "auth",
    request_body(content = serde_json::Value),
    responses(
        (status = 200, description = "Password updated (and the address confirmed, if it was not)"),
        (status = 400, description = "Missing token, or a password the rules refuse (for a live token)", body = ApiErrorBody),
        (status = 404, description = "Invalid, used or expired token; checked before the password", body = ApiErrorBody),
    )
)]
pub async fn reset(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<StatusCode> {
    let Some(token) = body["token"].as_str() else {
        return Err(ApiError(AppError::validation("token required")));
    };
    let Some(password) = body["password"].as_str() else {
        return Err(ApiError(AppError::validation("password required")));
    };
    let token_hash = hex::encode(Sha256::digest(token));
    if !vyasa_db::repo::TokensRepo::new(state.pool.clone())
        .is_live(&token_hash, vyasa_db::models::TokenPurpose::Reset)
        .await?
    {
        return Err(ApiError(AppError::not_found("reset_token", "")));
    }
    // The one rule every path that sets a password applies.
    vyasa_core::user::password::validate_password(password)?;
    let hash = vyasa_core::user::password::hash_password(password)?;
    let redeemed = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .reset_password_with_token(&token_hash, &hash)
        .await
        .map_err(ApiError)?;
    let user_id = redeemed.user_id;
    if redeemed.confirmed_now {
        if let Ok(user) = state.users.get(user_id).await {
            crate::audit::record(
                &state,
                &user,
                "user.confirm",
                format!("user:{user_id}"),
                serde_json::json!({ "by": "reset" }),
            );
        }
    }
    // The point of a reset is that the old credential no longer gets in.
    // Every session was left running before, so a stolen one outlived the
    // password it was stolen with.
    vyasa_db::repo::SessionsRepo::new(state.pool.clone())
        .delete_for_user(user_id)
        .await
        .map_err(ApiError)?;
    // The same goes for API keys: one minted with a stolen session is as
    // good as the session, and outlived the reset until now.
    state
        .api_keys
        .revoke_all_for_user(user_id)
        .await
        .map_err(ApiError)?;
    Ok(StatusCode::OK)
}

#[cfg(test)]
mod reset_token_tests {
    use super::fresh_reset_token;

    #[test]
    fn a_reset_token_is_not_something_you_can_work_out() {
        // Derived from a snowflake and the address, two requests a moment
        // apart produced tokens an attacker could enumerate. From the OS
        // generator they share nothing but the prefix.
        let a = fresh_reset_token();
        let b = fresh_reset_token();
        assert_ne!(a, b);
        for t in [&a, &b] {
            assert!(t.starts_with("vy-"), "{t}");
            let hex = &t[3..];
            assert_eq!(hex.len(), 64, "256 bits: {t}");
            assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), "{t}");
        }
    }
}

/// Every per-address count and the sign-in lockout hold a fixed-size key,
/// and only for something that could be an address: a stranger cannot make
/// them hold whatever they typed (phase 98 review).
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod address_key_tests {
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::Json;
    use serde_json::json;
    use vyasa_db::repo::OptionsRepo;
    use vyasa_testkit::TestDb;

    use super::{forgot, login, LoginRequest};
    use crate::authz::tests::test_state;
    use crate::client_ip::ClientIp;
    use crate::error::ApiResult;
    use crate::middleware::rate_limit::{CONFIRMATION_MAIL, RESET_MAIL};
    use crate::rest::registration::{resend, Accepted, ResendRequest};
    use crate::state::AppState;

    /// Things typed into an email field that are not addresses: the 2 MB
    /// a client could send at the `write` cap, just past the 254 bytes
    /// SMTP allows, no `@`, a space.
    fn not_addresses() -> Vec<String> {
        vec![
            format!("{}@example.test", "a".repeat(2 * 1024 * 1024)),
            format!("{}@example.test", "a".repeat(250)),
            "no-at-sign".to_owned(),
            "two words@example.test".to_owned(),
        ]
    }

    fn by_address(state: &AppState, bucket: &str) -> Vec<String> {
        state
            .rate_limiter
            .keys()
            .into_iter()
            .filter(|(b, _)| b == bucket)
            .map(|(_, key)| key)
            .collect()
    }

    #[tokio::test]
    async fn forgot_answers_a_malformed_address_alike_and_counts_nothing() {
        let db = TestDb::new().await;
        let state = test_state(&db);
        let ask = |email: &str| forgot(State((*state).clone()), Json(json!({ "email": email })));
        for email in not_addresses() {
            let answer = ask(&email).await.map_err(|e| e.0.to_string());
            assert_eq!(answer, Ok(StatusCode::OK), "{}", email.len());
        }
        assert!(
            state.rate_limiter.keys().is_empty(),
            "{:?}",
            state
                .rate_limiter
                .keys()
                .iter()
                .map(|k| k.1.len())
                .collect::<Vec<_>>()
        );

        // A real address is counted, under a fixed-size key, and limited
        // as before: three an hour, however it is spelt.
        let email = format!("{}@example.test", "b".repeat(200));
        for spelling in [
            email.clone(),
            email.to_uppercase(),
            format!(" {email} "),
            email.clone(),
        ] {
            assert_eq!(ask(&spelling).await.ok(), Some(StatusCode::OK));
        }
        let keys = by_address(&state, RESET_MAIL);
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert_eq!(keys[0].len(), 64, "{}", keys[0]);
        assert_eq!(state.rate_limiter.keys().len(), 1);
        assert!(
            !state.rate_limiter.check(RESET_MAIL, &keys[0]),
            "the address's three requests are spent"
        );
    }

    #[tokio::test]
    async fn resend_answers_a_malformed_address_alike_and_counts_no_address() {
        let db = TestDb::new().await;
        let options = OptionsRepo::new(db.pool().clone());
        for (key, value) in [
            ("registration_enabled", json!(true)),
            ("smtp_host", json!("127.0.0.1")),
            ("smtp_port", json!("1")),
            ("site_url", json!("https://site.example")),
        ] {
            options.set(key, &value).await.unwrap();
        }
        let state = test_state(&db);
        let ask = |client: u8, email: &str| {
            resend(
                State((*state).clone()),
                ClientIp(format!("192.0.2.{client}")),
                Json(ResendRequest {
                    email: email.to_owned(),
                }),
            )
        };
        let seen = |answer: Result<(StatusCode, Json<Accepted>), Response>| {
            let (status, Json(body)) = answer.expect("accepted");
            (status, body.message)
        };
        let reference = seen(ask(1, "nobody@example.test").await);
        assert_eq!(reference.0, StatusCode::ACCEPTED);
        for (n, email) in (2..).zip(not_addresses()) {
            assert_eq!(seen(ask(n, &email).await), reference, "{}", email.len());
        }
        let keys = by_address(&state, CONFIRMATION_MAIL);
        assert_eq!(keys.len(), 1, "only the real address is counted");
        assert_eq!(keys[0].len(), 64, "{}", keys[0]);
        assert!(state
            .rate_limiter
            .keys()
            .iter()
            .all(|(_, key)| key.len() <= 64));
    }

    async fn refusal(answer: ApiResult<impl IntoResponse>) -> (StatusCode, axum::body::Bytes) {
        let response = match answer {
            Ok(_) => panic!("signed in"),
            Err(err) => err.into_response(),
        };
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, body)
    }

    fn sign_in(
        state: &AppState,
        email: &str,
        password: &str,
    ) -> impl std::future::Future<Output = ApiResult<(HeaderMap, Json<super::UserResponse>)>> {
        login(
            State(state.clone()),
            ClientIp("192.0.2.9".to_owned()),
            HeaderMap::new(),
            Json(LoginRequest {
                email: email.to_owned(),
                password: password.to_owned(),
                extra_code: None,
            }),
        )
    }

    /// Malformed input is an unknown address: the ordinary 401, and counted
    /// by the lockout under its fixed-size digest like any other.
    #[tokio::test]
    async fn a_sign_in_with_a_malformed_address_is_an_unknown_address() {
        let db = TestDb::new().await;
        let state = test_state(&db);
        let reference =
            refusal(sign_in(&state, "nobody@example.test", "not-the-password").await).await;
        assert_eq!(reference.0, StatusCode::UNAUTHORIZED);
        for email in not_addresses() {
            assert_eq!(
                refusal(sign_in(&state, &email, "not-the-password").await).await,
                reference
            );
        }
        let (entries, bytes) = state.lockout.footprint();
        assert_eq!(entries, 1 + not_addresses().len());
        assert!(
            bytes <= entries * (32 + "192.0.2.9".len()),
            "{bytes} bytes for {entries} entries"
        );
        // And it is slowed down like one. (A short one: the first delay is
        // a second, and folding two megabytes per attempt in a debug build
        // can take that long on a busy machine.)
        for _ in 0..4 {
            drop(sign_in(&state, "no-at-sign", "not-the-password").await);
        }
        let (_, body) = refusal(sign_in(&state, "no-at-sign", "not-the-password").await).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("too many failed attempts"), "{body}");
    }

    /// An account an administrator made is only required to have an `@`;
    /// one whose address registration's rule would refuse is still
    /// protected by the lockout.
    #[tokio::test]
    async fn an_account_with_an_unusual_address_keeps_its_lockout() {
        let db = TestDb::new().await;
        let odd = "first last@example.test";
        let hash = vyasa_core::user::password::hash_password("the-real-password").unwrap();
        vyasa_db::repo::UsersRepo::new(db.pool().clone())
            .insert(&vyasa_db::repo::NewUser {
                id: 99_301,
                email: odd,
                username: "odd",
                display_name: "odd",
                password_hash: Some(&hash),
                role: vyasa_db::models::Role::Subscriber,
                bio: "",
            })
            .await
            .unwrap();
        let state = test_state(&db);
        for _ in 0..4 {
            let (status, _) = refusal(sign_in(&state, odd, "a-guess").await).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let (_, body) = refusal(sign_in(&state, odd, "the-real-password").await).await;
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("too many failed attempts"), "{body}");
    }
}
