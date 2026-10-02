//! User management endpoints (admin-scoped) and self-profile endpoints.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;

use vyasa_core::user::{CreateUser, UpdateProfile};
use vyasa_db::models::Role;

use super::roles::RoleChoice;
use super::UserResponse;
use vyasa_common::AppError;

use crate::client_ip::ClientIp;
use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::CurrentUser;
use crate::policy;
use crate::state::AppState;

/// Request body for creating a user.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateUserRequest {
    /// Email address.
    pub email: String,
    /// Username (derived from email when absent).
    pub username: Option<String>,
    /// Display name (falls back to username).
    pub display_name: Option<String>,
    /// Initial password; `None` creates an invited/passwordless account.
    pub password: Option<String>,
    /// A built-in role's name or a custom role's slug (defaults to
    /// subscriber). Every capability it holds must be one the caller holds.
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "subscriber".to_string()
}

/// Request body for updating a profile.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateProfileRequest {
    /// New display name.
    pub display_name: Option<String>,
    /// New biography.
    pub bio: Option<String>,
    /// New password. Requires `current_password`.
    pub password: Option<String>,
    /// The password being replaced; required with `password`, so a
    /// borrowed or stolen session cannot lock the owner out.
    #[serde(default)]
    pub current_password: Option<String>,
    /// A media id for the avatar; `Some(None)` clears it. Absent keeps.
    /// Three states, so the nested option is the point.
    #[serde(default, deserialize_with = "deserialize_double_option")]
    #[allow(clippy::option_option)]
    pub avatar_media_id: Option<Option<i64>>,
}

#[allow(clippy::option_option)]
fn deserialize_double_option<'de, D>(d: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<i64>::deserialize(d)?))
}

/// Query parameters for listing users.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct ListUsersQuery {
    /// Page size (default 20, max 100).
    pub per_page: Option<u32>,
    /// Page number (1-based).
    pub page: Option<u32>,
    /// Matches email, username or display name.
    pub q: Option<String>,
}

/// An administrator's edit of someone's identity; every field optional.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateUserRequest {
    pub email: Option<String>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub bio: Option<String>,
}

/// Suspend or reinstate.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct SuspendRequest {
    pub suspended: bool,
}

/// `GET /api/v1/users` — list users (manage_users).
///
/// # Errors
///
/// 403 without the capability.
#[utoipa::path(
    get, path = "/api/v1/users",
    tag = "users",
    security(("session_cookie" = [])),
    params(ListUsersQuery),
    responses(
        (status = 200, description = "Paged user list", body = [UserResponse]),
        (status = 403, description = "Missing manage_users", body = ApiErrorBody),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListUsersQuery>,
) -> ApiResult<(axum::http::HeaderMap, Json<Vec<UserResponse>>)> {
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let page = query.page.unwrap_or(1).max(1);
    let (users, total) = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .search(
            query.q.as_deref().unwrap_or_default(),
            per_page,
            (page - 1).saturating_mul(per_page),
        )
        .await?;
    let mut headers = axum::http::HeaderMap::new();
    if let Ok(v) = axum::http::HeaderValue::from_str(&total.to_string()) {
        headers.insert("x-total-count", v);
    }
    let with_mfa = crate::mfa::enabled_ids(&state).await;
    Ok((
        headers,
        Json(
            users
                .into_iter()
                .map(|u| {
                    let mut r = UserResponse::from(u);
                    r.mfa_enabled = with_mfa.contains(&r.id);
                    r
                })
                .collect(),
        ),
    ))
}

/// `PATCH /api/v1/users/{id}` — a user manager edits someone's identity.
/// The account must hold no capability the caller lacks.
#[utoipa::path(
    patch, path = "/api/v1/users/{id}",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "Updated", body = UserResponse),
        (status = 403, description = "Missing manage_users, or the account holds a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "Unknown user", body = ApiErrorBody),
        (status = 409, description = "Email/username taken", body = ApiErrorBody),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateUserRequest>,
) -> ApiResult<Json<UserResponse>> {
    let target = policy::account_for_management(&state, &user, id).await?;
    policy::demo_account(&state, &target, "Changing")?;
    let email = body.email.as_deref().map(str::trim);
    if let Some(e) = email {
        if e.is_empty() || !e.contains('@') {
            return Err(ApiError(AppError::validation(
                "email must be a valid address",
            )));
        }
    }
    let username = body.username.as_deref().map(str::trim);
    if let Some(u) = username {
        if u.is_empty()
            || !u
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            return Err(ApiError(AppError::validation(
                "username may contain letters, digits, dots, dashes and underscores",
            )));
        }
    }
    let row = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .update_identity(
            id,
            email,
            username,
            body.display_name.as_deref().map(str::trim),
            body.bio.as_deref(),
        )
        .await
        .map_err(|e| match e {
            AppError::Db { message } if message.contains("duplicate key") => {
                AppError::conflict("that email or username is already taken")
            }
            other => other,
        })?;
    crate::audit::record(
        &state,
        &user.user,
        "user.update",
        format!("user:{id}"),
        serde_json::json!({}),
    );
    Ok(Json(UserResponse::from(row)))
}

/// `POST /api/v1/users/{id}/suspend` — stop or allow sign-in. Suspending
/// also ends every session the account has, and is refused when it would
/// leave no active administrator.
#[utoipa::path(
    post, path = "/api/v1/users/{id}/suspend",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    request_body = SuspendRequest,
    responses(
        (status = 204, description = "Applied"),
        (status = 400, description = "Cannot suspend yourself or the last admin", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the account holds a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "Unknown user", body = ApiErrorBody),
    )
)]
pub async fn suspend(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Json(body): Json<SuspendRequest>,
) -> ApiResult<StatusCode> {
    if body.suspended && id == user.user.id {
        return Err(ApiError(AppError::validation(
            "you cannot suspend your own account",
        )));
    }
    let target = policy::account_for_management(&state, &user, id).await?;
    policy::demo_account(&state, &target, "Suspending")?;
    // One transaction decides and writes: two admins suspending each
    // other cannot both go through.
    state.users.set_suspended(id, body.suspended).await?;
    if body.suspended {
        vyasa_db::repo::SessionsRepo::new(state.pool.clone())
            .delete_for_user(id)
            .await?;
    }
    crate::audit::record(
        &state,
        &user.user,
        if body.suspended {
            "user.suspend"
        } else {
            "user.reinstate"
        },
        format!("user:{id}"),
        serde_json::json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/users/{id}/sessions/revoke` — sign the account out
/// everywhere.
#[utoipa::path(
    post, path = "/api/v1/users/{id}/sessions/revoke",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    responses(
        (status = 200, description = "Sessions ended", body = serde_json::Value),
        (status = 403, description = "Someone else's account without manage_users, or one holding a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "Unknown user (someone else's account)", body = ApiErrorBody),
    )
)]
pub async fn revoke_sessions(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    policy::account_as_self_or_managed(&state, &user, id).await?;
    let ended = vyasa_db::repo::SessionsRepo::new(state.pool.clone())
        .delete_for_user(id)
        .await?;
    Ok(Json(serde_json::json!({ "ended": ended })))
}

/// `DELETE /api/v1/users/{id}/mfa` — removes someone's second factor,
/// for when the phone is gone and the recovery codes with it.
#[utoipa::path(
    delete, path = "/api/v1/users/{id}/mfa",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    responses(
        (status = 204, description = "Removed"),
        (status = 403, description = "Missing manage_users, or the account holds a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "Unknown user", body = ApiErrorBody),
    )
)]
pub async fn reset_mfa(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    policy::account_for_management(&state, &user, id).await?;
    crate::mfa::reset(&state, id).await?;
    crate::audit::record(
        &state,
        &user.user,
        "user.mfa_reset",
        format!("user:{id}"),
        serde_json::json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/users/{id}/reset-link` — email a set-password link: the
/// invitation again for an account that never had a password, a reset
/// for one that has.
#[utoipa::path(
    post, path = "/api/v1/users/{id}/reset-link",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    responses(
        (status = 200, description = "Queued", body = serde_json::Value),
        (status = 400, description = "No mail relay or no site address", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the account holds a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "Unknown user", body = ApiErrorBody),
    )
)]
pub async fn reset_link(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    let target = policy::account_for_management(&state, &user, id).await?;
    let kind = crate::rest::auth::send_set_password_link(&state, &target, Some(&user.user)).await?;
    Ok(Json(serde_json::json!({ "sent": kind })))
}

/// What `POST /users/{id}/confirm` answers: the account, and whether the
/// link to set a password went out.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ConfirmResponse {
    /// The account as it is now.
    #[serde(flatten)]
    pub user: UserResponse,
    /// Present when this request confirmed the account and removed its
    /// password, so a set-password link was due: whether it was queued.
    /// `false` means the account is confirmed and has no password, and
    /// someone has to send it a link ("Send reset link"/"Resend invite").
    /// Absent when nothing was due (confirmed already).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_sent: Option<bool>,
}

/// `POST /api/v1/users/{id}/confirm` — confirm an account's address on
/// the administrator's word. The administrator vouches for the mailbox,
/// not for the password a registrant typed: that password is removed and
/// the address is mailed a link to set one. Idempotent: an account
/// confirmed already is left as it is and sent nothing.
///
/// A link that could not be queued once the account is confirmed does not
/// undo the confirmation; the answer says so (`link_sent: false`).
#[utoipa::path(
    post, path = "/api/v1/users/{id}/confirm",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    responses(
        (status = 200, description = "Confirmed; `link_sent` says whether the set-password link was \
            queued (absent when the account was confirmed already)", body = ConfirmResponse),
        (status = 400, description = "No mail relay or no site address, so the set-password link \
            could not be sent (nothing was changed)", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the account holds a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "Unknown user", body = ApiErrorBody),
    )
)]
pub async fn confirm(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<ConfirmResponse>> {
    let target = policy::account_for_management(&state, &user, id).await?;
    if target.email_verified_at.is_some() {
        return Ok(Json(ConfirmResponse {
            user: UserResponse::from(target),
            link_sent: None,
        }));
    }
    // Checked before anything changes: the owner will need the link.
    if crate::mail::effective(&state).await.is_none() {
        return Err(ApiError(AppError::validation(
            "no mail relay is configured, so the link to set a password cannot be sent; \
             add one under Settings",
        )));
    }
    crate::rest::auth::security_link_base(&state).await?;
    let row = state.registration.confirm(id).await?;
    crate::audit::record(
        &state,
        &user.user,
        "user.confirm",
        format!("user:{id}"),
        serde_json::json!({ "by": "administrator" }),
    );
    let link_sent = if row.password_hash.as_deref().is_none_or(str::is_empty) {
        match crate::rest::auth::send_set_password_link(&state, &row, None).await {
            Ok(_) => Some(true),
            Err(err) => {
                tracing::warn!(user_id = id, "set-password link not queued: {err}");
                Some(false)
            }
        }
    } else {
        None
    };
    Ok(Json(ConfirmResponse {
        user: UserResponse::from(row),
        link_sent,
    }))
}

/// `POST /api/v1/users/{id}/resend-confirmation` — mail an unconfirmed
/// account a fresh confirmation link.
#[utoipa::path(
    post, path = "/api/v1/users/{id}/resend-confirmation",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    responses(
        (status = 204, description = "Queued"),
        (status = 400, description = "Already confirmed, or no mail relay or no site address", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the account holds a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "Unknown user", body = ApiErrorBody),
    )
)]
pub async fn resend_confirmation(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let target = policy::account_for_management(&state, &user, id).await?;
    if target.email_verified_at.is_some() {
        return Err(ApiError(AppError::validation(
            "this account's email address is already confirmed",
        )));
    }
    if crate::mail::effective(&state).await.is_none() {
        return Err(ApiError(AppError::validation(
            "no mail relay is configured, so the link cannot be sent; add one under Settings",
        )));
    }
    // Checked before a token is made: no site address, no link.
    crate::rest::auth::security_link_base(&state).await?;
    let token = state.registration.issue_confirmation(&target).await?;
    let needs_password = target.password_hash.as_deref().is_none_or(str::is_empty);
    crate::rest::registration::send_confirmation(&state, &target, &token, needs_password).await?;
    crate::audit::record(
        &state,
        &user.user,
        "user.resend_confirmation",
        format!("user:{id}"),
        serde_json::json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/users/{id}` — fetch a user (manage_users, or self).
///
/// # Errors
///
/// 403/404 as appropriate.
#[utoipa::path(
    get, path = "/api/v1/users/{id}",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    responses(
        (status = 200, description = "User", body = UserResponse),
        (status = 403, description = "Missing manage_users (non-self)", body = ApiErrorBody),
        (status = 404, description = "Unknown user", body = ApiErrorBody),
    )
)]
pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<UserResponse>> {
    policy::account_as_self_or_manager(&user, id)?;
    Ok(Json(UserResponse::from(state.users.get(id).await?)))
}

/// `POST /api/v1/users` — create a user (manage_users).
///
/// # Errors
///
/// 403 without the capability; 409 on email/username conflict.
#[utoipa::path(
    post, path = "/api/v1/users",
    tag = "users",
    security(("session_cookie" = [])),
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "User created", body = UserResponse),
        (status = 400, description = "Validation failed, or an unknown role", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the role holds a capability the caller lacks", body = ApiErrorBody),
        (status = 409, description = "Email/username taken", body = ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateUserRequest>,
) -> ApiResult<(StatusCode, Json<UserResponse>)> {
    // Decided before anything is written: no account is left behind by a
    // refusal.
    let choice = policy::may_grant_role(&state, &user, &body.role).await?;
    let invited = body.password.is_none();
    let input = |role| CreateUser {
        email: body.email,
        username: body.username,
        display_name: body.display_name,
        password: body.password,
        role,
    };
    // One write either way: a custom role that went away in the meantime
    // fails the creation, and nothing below (the invitation, the hook)
    // happens for an account that is not there.
    let created = match &choice {
        RoleChoice::BuiltIn(role) => state.users.create(input(*role)).await?,
        RoleChoice::Custom(role) => {
            state
                .users
                .create_with_custom_role(input(Role::Subscriber), &role.slug)
                .await?
        }
    };
    if invited {
        // The account cannot sign in until a password is set; without this
        // mail the person was never told they had one.
        if let Err(err) =
            crate::rest::auth::send_set_password_link(&state, &created, Some(&user.user)).await
        {
            tracing::warn!(
                user_id = created.id,
                "invitation could not be queued: {err}"
            );
        }
    }
    // Who created whom, with which role: an account is as hard to take
    // back as a role change, and is recorded like one.
    crate::audit::record(
        &state,
        &user.user,
        "user.create",
        format!("user:{}", created.id),
        serde_json::json!({
            "role": created.role.as_str(),
            "custom_role": created.custom_role,
            "invited": invited,
        }),
    );
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::USER_CREATED,
        crate::plugin_hooks::account_payload(&created),
    );
    Ok((
        axum::http::StatusCode::CREATED,
        Json(UserResponse::from(created)),
    ))
}

/// `PUT /api/v1/users/me` — update own profile.
///
/// Changing the password needs `current_password` too, and signs the
/// account out everywhere except this session.
///
/// # Errors
///
/// 401 when unauthenticated; 400 on validation failures or a missing
/// `current_password`; 403 when `current_password` is wrong.
#[utoipa::path(
    put, path = "/api/v1/users/me",
    tag = "users",
    security(("session_cookie" = [])),
    request_body = UpdateProfileRequest,
    responses(
        (status = 204, description = "Profile updated"),
        (status = 400, description = "Validation failed or current_password missing", body = ApiErrorBody),
        (status = 403, description = "current_password is wrong", body = ApiErrorBody),
    )
)]
pub async fn update_me(
    State(state): State<AppState>,
    user: CurrentUser,
    ClientIp(client_ip): ClientIp,
    Json(body): Json<UpdateProfileRequest>,
) -> ApiResult<StatusCode> {
    let body_avatar = body.avatar_media_id;
    let changing_password = body.password.is_some();
    policy::demo_password_change(&state, changing_password)?;
    if changing_password {
        check_current_password(&state, &user, &client_ip, body.current_password.as_deref()).await?;
    }
    state
        .users
        .update_profile(
            user.user.id,
            UpdateProfile {
                display_name: body.display_name,
                bio: body.bio,
                password: body.password,
            },
        )
        .await?;
    if let Some(avatar) = body_avatar {
        vyasa_db::repo::UsersRepo::new(state.pool.clone())
            .set_avatar(user.user.id, avatar)
            .await?;
    }
    if changing_password {
        // Whoever else is signed in as this account is signed in with the
        // old password; this session is the one that just proved the new
        // change, so it stays.
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id <> $2")
            .bind(user.user.id)
            .bind(&user.token)
            .execute(&state.pool)
            .await
            .map_err(|e| AppError::db(format!("session revoke: {e}")))?;
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// A password change must prove the old one, under the same progressive
/// lockout as sign-in so the check is not a free guessing oracle.
async fn check_current_password(
    state: &AppState,
    user: &CurrentUser,
    client_ip: &str,
    current: Option<&str>,
) -> Result<(), ApiError> {
    let Some(current) = current.filter(|c| !c.is_empty()) else {
        return Err(ApiError(AppError::validation(
            "current_password is required to change the password",
        )));
    };
    let email = &user.user.email;
    if let Some(wait) = state.lockout.check(email, client_ip) {
        return Err(ApiError(AppError::forbidden(format!(
            "too many failed attempts; try again in {}s",
            wait.as_secs().max(1)
        ))));
    }
    let fresh = state.users.get(user.user.id).await?;
    let hash = fresh.password_hash.unwrap_or_default();
    if hash.is_empty() || vyasa_core::user::password::verify_password(current, &hash).is_err() {
        state.lockout.record_failure(email, client_ip);
        return Err(ApiError(AppError::forbidden(
            "the current password is not right",
        )));
    }
    state.lockout.record_success(email, client_ip);
    Ok(())
}

/// Body of `PUT /api/v1/users/{id}/role`.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct SetRoleRequest {
    /// A built-in role's name (`admin`, `editor`, `author`, `contributor`,
    /// `subscriber`) or a custom role's slug.
    pub role: String,
}

/// `PUT /api/v1/users/{id}/role` — change a user's role (manage_users):
/// a built-in role, which takes any custom role away, or a custom role,
/// under which the built-in role is `subscriber`.
///
/// # Errors
///
/// 403 without the capability, for a role (built-in or custom) holding a
/// capability the caller lacks, or for an account that holds one; 400 for an unknown role or when the last admin would
/// lose the admin role; 404 for an unknown user.
#[utoipa::path(
    put, path = "/api/v1/users/{id}/role",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id")),
    request_body = SetRoleRequest,
    responses(
        (status = 204, description = "Role changed"),
        (status = 400, description = "Unknown role, or the last admin would lose the admin role", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users; the role holds a capability the caller lacks; or the account does", body = ApiErrorBody),
        (status = 404, description = "Unknown user", body = ApiErrorBody),
    )
)]
pub async fn set_role(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Json(body): Json<SetRoleRequest>,
) -> ApiResult<StatusCode> {
    let choice = policy::may_grant_role(&state, &user, &body.role).await?;
    let target = policy::account_for_management(&state, &user, id).await?;
    policy::demo_account(&state, &target, "Changing the role of")?;
    match &choice {
        RoleChoice::BuiltIn(role) => state.users.set_role(id, *role).await?,
        RoleChoice::Custom(role) => {
            state.roles.assign(&user.user, id, Some(&role.slug)).await?;
        }
    }
    crate::audit::record(
        &state,
        &user.user,
        "user.role_change",
        format!("user:{id}"),
        serde_json::json!({ "role": body.role }),
    );
    let (base, custom) = choice.stored();
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::USER_ROLE_CHANGED,
        crate::plugin_hooks::role_payload(id, base, custom),
    );
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Query of `DELETE /api/v1/users/{id}`.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct DeleteUserQuery {
    /// The user who takes over the deleted user's posts, media and
    /// revisions. Required when the user owns posts or media.
    pub reassign_to: Option<i64>,
}

/// `DELETE /api/v1/users/{id}` — delete a user (manage_users), handing
/// what they own to `reassign_to`.
///
/// # Errors
///
/// 403 without the capability; 400 when deleting your own account, the
/// last admin, or reassigning to the user being deleted; 404 for an unknown user or
/// target; 409 when the user owns content and no target was given.
#[utoipa::path(
    delete, path = "/api/v1/users/{id}",
    tag = "users",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "User id"), DeleteUserQuery),
    responses(
        (status = 204, description = "User deleted"),
        (status = 400, description = "Your own account, the last admin, or reassign_to is the user being deleted", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the account holds a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "Unknown user or reassign_to target", body = ApiErrorBody),
        (status = 409, description = "The user owns posts or media and no reassign_to was given", body = ApiErrorBody),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
    Query(query): Query<DeleteUserQuery>,
) -> ApiResult<StatusCode> {
    if id == user.user.id {
        return Err(ApiError(AppError::validation(
            "you cannot delete your own account",
        )));
    }
    // The account deleted is the one that must be within reach; whoever
    // takes over its content only gains content.
    let target = policy::account_for_management(&state, &user, id).await?;
    policy::demo_account(&state, &target, "Deleting")?;
    state
        .users
        .delete_reassigning(id, query.reassign_to)
        .await?;
    crate::audit::record(
        &state,
        &user.user,
        "user.delete",
        format!("user:{id}"),
        serde_json::json!({ "reassign_to": query.reassign_to }),
    );
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::USER_DELETED,
        serde_json::json!({ "user_id": id }),
    );
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Effective capabilities of the current user (admin UI badge logic).
///
/// # Errors
///
/// 401 when unauthenticated.
#[utoipa::path(
    get, path = "/api/v1/auth/me/caps",
    tag = "auth",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Capability wire names", body = [String]),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn my_caps(user: CurrentUser) -> ApiResult<Json<Vec<&'static str>>> {
    let caps = vyasa_core::user::effective_caps(&user.user);
    let names = caps
        .iter()
        .map(|c| vyasa_core::user::cap_name(*c))
        .collect();
    Ok(Json(names))
}

use axum::http::StatusCode;
