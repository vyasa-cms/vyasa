//! `/api/v1/setup/*`: the first-run wizard's HTTP surface.
//!
//! Before an administrator exists, the setup cookie (claimed with the
//! boot token) is the only credential; after, an administrator session is.

use axum::extract::{FromRequestParts, State};
use axum::http::{header, request::Parts, HeaderMap, HeaderValue, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_core::user::Capability;

use crate::error::{ApiError, ApiResult};
use crate::middleware::Principal;
use crate::policy;
use crate::setup::{
    self, AccountInput, AssistantsInput, ContentInput, DeliveryInput, Keypair, MailInput,
    SiteInput, UpdatesInput,
};
use crate::state::AppState;

const SETUP_COOKIE: &str = "vy_setup";

/// Whoever may run setup right now: the token holder before the first
/// administrator exists, a signed-in `manage_options` holder after.
pub struct SetupPrincipal {
    /// The signed-in caller's id, once there is an administrator.
    pub admin_id: Option<i64>,
    /// The signed-in caller; `None` for the setup-token holder.
    principal: Option<Principal>,
}

impl SetupPrincipal {
    /// The wizard steps that write what `policy::full_administrator`
    /// guards elsewhere (the site address, the mail relay, the update
    /// channel and trusted keys) ask the same of a signed-in caller. The
    /// setup-token holder is the person installing the site and has no
    /// principal to ask: first-run setup is unchanged.
    fn full_administrator(&self, action: &str) -> Result<(), AppError> {
        self.principal
            .as_ref()
            .map_or(Ok(()), |p| policy::full_administrator(p, action))
    }
}

impl FromRequestParts<AppState> for SetupPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        if let Ok(principal) = Principal::from_request_parts(parts, state).await {
            principal.ensure(Capability::ManageOptions)?;
            return Ok(Self {
                admin_id: Some(principal.user().id),
                principal: Some(principal),
            });
        }
        let cookie_ok = parts
            .headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|raw| {
                raw.split(';')
                    .map(str::trim)
                    .any(|pair| token_matches(state, pair))
            });
        if cookie_ok && setup::needs_admin(state).await? {
            return Ok(Self {
                admin_id: None,
                principal: None,
            });
        }
        Err(ApiError(AppError::auth(
            "setup needs the setup token, or an administrator",
        )))
    }
}

fn token_matches(state: &AppState, pair: &str) -> bool {
    let Some(value) = pair
        .strip_prefix(SETUP_COOKIE)
        .and_then(|v| v.strip_prefix('='))
    else {
        return false;
    };
    let guard = state
        .setup_token
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.as_deref() == Some(value)
}

/// What a visitor may know before anything else.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SetupStatus {
    /// No administrator exists yet.
    pub needs_admin: bool,
    /// Setup has not been finished (may be true with an admin).
    pub needs_setup: bool,
    /// The step to resume at.
    pub step: Option<String>,
    /// A nonce minted at boot; `site_url` verification looks for it.
    pub instance: String,
    /// On a public demo, the shared account the sign-in page offers;
    /// `null` everywhere else.
    pub demo: Option<DemoAccount>,
}

/// The shared account of a public demo. Not a secret: it is printed on
/// every page of the demo.
#[derive(Serialize, utoipa::ToSchema)]
pub struct DemoAccount {
    pub username: String,
    pub password: String,
}

/// `GET /api/v1/setup/status` — public.
#[utoipa::path(get, path = "/api/v1/setup/status", tag = "setup",
    responses((status = 200, description = "Where setup stands", body = SetupStatus)))]
pub async fn status(State(state): State<AppState>) -> ApiResult<Json<SetupStatus>> {
    let needs_admin = setup::needs_admin(&state).await?;
    let progress = setup::progress(&state).await;
    Ok(Json(SetupStatus {
        needs_admin,
        // An install with an administrator and no progress record predates
        // the wizard (or used `vyasa admin create`); it is not unfinished.
        needs_setup: needs_admin || progress.as_deref().is_some_and(|p| p != "done"),
        step: progress,
        instance: state.instance_nonce.clone(),
        demo: state.config.demo.enabled.then(|| DemoAccount {
            username: state.config.demo.username.clone(),
            password: state.config.demo.password.clone(),
        }),
    }))
}

/// The token from the server log.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ClaimBody {
    pub token: String,
}

/// `POST /api/v1/setup/claim` — trade the boot token for the setup cookie.
#[utoipa::path(post, path = "/api/v1/setup/claim", tag = "setup", request_body = ClaimBody,
    responses((status = 204, description = "Claimed"), (status = 403, description = "Wrong token"),
              (status = 410, description = "An administrator already exists")))]
pub async fn claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ClaimBody>,
) -> ApiResult<(StatusCode, HeaderMap)> {
    if !setup::needs_admin(&state).await? {
        // No `Gone` variant in AppError; the status alone says it.
        return Ok((StatusCode::GONE, HeaderMap::new()));
    }
    let ok = {
        let guard = state
            .setup_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.as_deref() == Some(body.token.trim())
    };
    if !ok {
        return Err(ApiError(AppError::forbidden(
            "that is not this server's setup token",
        )));
    }
    let secure = if crate::feeds::origin_from_headers(&headers).starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let mut out = HeaderMap::new();
    out.insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{SETUP_COOKIE}={}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age=3600",
            body.token.trim()
        ))
        .map_err(|e| AppError::internal_msg(e.to_string()))?,
    );
    Ok((StatusCode::NO_CONTENT, out))
}

/// One environment check, as the wizard shows it.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SetupCheck {
    pub name: String,
    /// `ok`, `warn` or `fail`.
    pub status: String,
    pub detail: String,
}

/// `GET /api/v1/setup/checks` — the environment, tested.
#[utoipa::path(get, path = "/api/v1/setup/checks", tag = "setup",
    responses((status = 200, description = "Checks", body = [SetupCheck])))]
pub async fn checks(
    State(state): State<AppState>,
    _who: SetupPrincipal,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<SetupCheck>>> {
    let https = crate::feeds::origin_from_headers(&headers).starts_with("https://");
    let out = setup::checks(&state, https)
        .await
        .into_iter()
        .map(|c| SetupCheck {
            name: c.name,
            status: serde_json::to_value(c.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            detail: c.detail,
        })
        .collect();
    Ok(Json(out))
}

/// `POST /api/v1/setup/account` — the first administrator; signs them in.
#[utoipa::path(post, path = "/api/v1/setup/account", tag = "setup", request_body = AccountInput,
    responses((status = 201, description = "Created and signed in"), (status = 409, description = "An administrator exists")))]
pub async fn account(
    State(state): State<AppState>,
    _who: SetupPrincipal,
    headers: HeaderMap,
    Json(input): Json<AccountInput>,
) -> ApiResult<(StatusCode, HeaderMap, Json<crate::rest::UserResponse>)> {
    let password = input.password.clone();
    let user = setup::account(&state, input).await?;
    let session = state.auth.login(&user.email, &password, None, None).await?;
    let secure = if crate::feeds::origin_from_headers(&headers).starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let mut out = HeaderMap::new();
    out.append(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!(
            "{}={}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age=1209600",
            crate::middleware::SESSION_COOKIE,
            session.token
        ))
        .map_err(|e| AppError::internal_msg(e.to_string()))?,
    );
    out.append(
        header::SET_COOKIE,
        HeaderValue::from_static("vy_setup=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    Ok((StatusCode::CREATED, out, Json(session.user.into())))
}

/// What the site step reports back.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SiteResult {
    /// The address answers and it is this server.
    pub site_url_verified: bool,
}

/// `POST /api/v1/setup/site`
#[utoipa::path(post, path = "/api/v1/setup/site", tag = "setup", request_body = SiteInput,
    responses((status = 200, description = "Saved", body = SiteResult)))]
pub async fn site(
    State(state): State<AppState>,
    who: SetupPrincipal,
    Json(input): Json<SiteInput>,
) -> ApiResult<Json<SiteResult>> {
    // The step always writes `site_url`.
    who.full_administrator("change the site address")?;
    let url = input.site_url.clone();
    setup::site(&state, input).await?;
    Ok(Json(SiteResult {
        site_url_verified: setup::site_url_reaches_us(&state, &url).await,
    }))
}

/// The address to test.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct VerifyUrlQuery {
    pub url: String,
}

/// What the round trip found.
#[derive(Serialize, utoipa::ToSchema)]
pub struct VerifyUrlResult {
    /// The address answers, and it is this very server.
    pub reachable: bool,
}

/// `GET /api/v1/setup/verify-url?url=` — does an address reach this server?
/// Settings uses it before saving a new site address.
#[utoipa::path(get, path = "/api/v1/setup/verify-url", tag = "setup",
    params(("url" = String, Query, description = "Address to test")),
    responses((status = 200, description = "Result", body = VerifyUrlResult)))]
pub async fn verify_url(
    State(state): State<AppState>,
    _who: SetupPrincipal,
    axum::extract::Query(q): axum::extract::Query<VerifyUrlQuery>,
) -> ApiResult<Json<VerifyUrlResult>> {
    let url = q.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Ok(Json(VerifyUrlResult { reachable: false }));
    }
    Ok(Json(VerifyUrlResult {
        reachable: setup::site_url_reaches_us(&state, url).await,
    }))
}

/// `POST /api/v1/setup/content`
#[utoipa::path(post, path = "/api/v1/setup/content", tag = "setup", request_body = ContentInput,
    responses((status = 204, description = "Saved")))]
pub async fn content(
    State(state): State<AppState>,
    who: SetupPrincipal,
    Json(input): Json<ContentInput>,
) -> ApiResult<StatusCode> {
    let Some(author) = who.admin_id else {
        return Err(ApiError(AppError::auth("create the account first")));
    };
    setup::content(&state, author, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/setup/delivery`
#[utoipa::path(post, path = "/api/v1/setup/delivery", tag = "setup", request_body = DeliveryInput,
    responses((status = 204, description = "Saved")))]
pub async fn delivery(
    State(state): State<AppState>,
    _who: SetupPrincipal,
    Json(input): Json<DeliveryInput>,
) -> ApiResult<StatusCode> {
    setup::delivery(&state, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/setup/mail`
#[utoipa::path(post, path = "/api/v1/setup/mail", tag = "setup", request_body = MailInput,
    responses((status = 204, description = "Saved")))]
pub async fn mail(
    State(state): State<AppState>,
    who: SetupPrincipal,
    Json(input): Json<MailInput>,
) -> ApiResult<StatusCode> {
    // Comment moderation and the newsletter switch are ordinary options;
    // the relay is not.
    if input.smtp.is_some() {
        who.full_administrator("change the mail relay")?;
    }
    setup::mail(&state, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Who receives the test message.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct MailTestBody {
    pub to: String,
}

/// `POST /api/v1/setup/mail/test` — send one message now.
#[utoipa::path(post, path = "/api/v1/setup/mail/test", tag = "setup", request_body = MailTestBody,
    responses((status = 204, description = "Delivered to the relay"), (status = 400, description = "No relay configured")))]
pub async fn mail_test(
    State(state): State<AppState>,
    who: SetupPrincipal,
    Json(body): Json<MailTestBody>,
) -> ApiResult<StatusCode> {
    who.full_administrator("send a test message through the mail relay")?;
    setup::mail_test(&state, body.to.trim()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/setup/assistants`
#[utoipa::path(post, path = "/api/v1/setup/assistants", tag = "setup", request_body = AssistantsInput,
    responses((status = 204, description = "Saved")))]
pub async fn assistants(
    State(state): State<AppState>,
    who: SetupPrincipal,
    Json(input): Json<AssistantsInput>,
) -> ApiResult<StatusCode> {
    // It writes a provider's key (and resets its address), which the
    // provider routes reserve for a full administrator.
    if input.provider.is_some() && input.api_key.is_some() {
        who.full_administrator(super::ai_models::AI_PROVIDER_WRITE)?;
    }
    setup::assistants(&state, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/setup/updates`
#[utoipa::path(post, path = "/api/v1/setup/updates", tag = "setup", request_body = UpdatesInput,
    responses((status = 204, description = "Saved")))]
pub async fn updates(
    State(state): State<AppState>,
    who: SetupPrincipal,
    Json(input): Json<UpdatesInput>,
) -> ApiResult<StatusCode> {
    if input.update_channel_url.is_some()
        || input.registry_url.is_some()
        || input.trusted_keys.is_some()
    {
        who.full_administrator(
            "change the update channel, the marketplace address or their trusted keys",
        )?;
    }
    setup::updates(&state, input).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/setup/keypair` — a plugin signing key, shown once.
#[utoipa::path(post, path = "/api/v1/setup/keypair", tag = "setup",
    responses((status = 200, description = "A fresh keypair", body = Keypair)))]
pub async fn keypair(_who: SetupPrincipal) -> Json<Keypair> {
    Json(setup::keypair())
}

/// What finishing did.
#[derive(Serialize, utoipa::ToSchema)]
pub struct FinishResult {
    /// Entries put in the search index.
    pub indexed: usize,
}

/// `POST /api/v1/setup/finish`
#[utoipa::path(post, path = "/api/v1/setup/finish", tag = "setup",
    responses((status = 200, description = "Done", body = FinishResult)))]
pub async fn finish(
    State(state): State<AppState>,
    _who: SetupPrincipal,
) -> ApiResult<Json<FinishResult>> {
    Ok(Json(FinishResult {
        indexed: setup::finish(&state).await?,
    }))
}
