//! Roles: the five built in, and the custom ones an administrator defines
//! from the existing capabilities.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use vyasa_common::AppError;
use vyasa_core::user::{cap_name, parse_cap, role_caps, Capability, RoleInput, RolePatch};
use vyasa_db::models::{Role, RoleRow, UserRow};

use crate::error::{ApiErrorBody, ApiResult};
use crate::middleware::CurrentUser;
use crate::state::AppState;

/// A role, built in or custom.
#[derive(Serialize, utoipa::ToSchema)]
pub struct RoleResponse {
    /// What `PUT /users/{id}/role` takes: a built-in role's name, or a
    /// custom role's slug.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// What the role is for.
    pub description: String,
    /// Capability names the role grants.
    pub capabilities: Vec<String>,
    /// One of the five fixed roles: it cannot be edited or deleted.
    pub built_in: bool,
    /// How many users hold the role.
    pub users: i64,
}

impl RoleResponse {
    fn custom(role: RoleRow, users: i64) -> Self {
        Self {
            slug: role.slug,
            name: role.name,
            description: role.description,
            capabilities: role.capabilities,
            built_in: false,
            users,
        }
    }

    fn built_in(role: Role, users: i64) -> Self {
        Self {
            slug: role.as_str().to_owned(),
            name: built_in_name(role).to_owned(),
            description: built_in_description(role).to_owned(),
            capabilities: role_caps(role)
                .iter()
                .map(|cap| cap_name(*cap).to_owned())
                .collect(),
            built_in: true,
            users,
        }
    }
}

/// A new custom role.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateRoleRequest {
    /// 2 to 40 lowercase letters, digits and hyphens, not starting with a
    /// hyphen, and not a built-in role's name.
    pub slug: String,
    /// Display name, 1 to 60 characters.
    pub name: String,
    /// What the role is for.
    #[serde(default)]
    pub description: String,
    /// Capability names; each must be one the caller holds. May be empty.
    pub capabilities: Vec<String>,
}

/// Changes to a custom role; an absent field stays as it is.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateRoleRequest {
    /// New slug; the users holding the role keep it.
    pub slug: Option<String>,
    /// New display name.
    pub name: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// New capability list, replacing the old one.
    pub capabilities: Option<Vec<String>>,
}

/// The display name of a built-in role.
#[must_use]
pub fn built_in_name(role: Role) -> &'static str {
    match role {
        Role::Admin => "Administrator",
        Role::Editor => "Editor",
        Role::Author => "Author",
        Role::Contributor => "Contributor",
        Role::Subscriber => "Subscriber",
    }
}

fn built_in_description(role: Role) -> &'static str {
    match role {
        Role::Admin => "Everything, including users, settings, themes and plugins.",
        Role::Editor => "Writes, publishes and manages everyone's content, comments and media.",
        Role::Author => "Writes and publishes their own content and uploads media.",
        Role::Contributor => "Writes their own drafts; cannot publish or upload.",
        Role::Subscriber => "Signs in and keeps a profile.",
    }
}

/// What a user's role is called: their custom role's name, or the
/// built-in role's.
#[must_use]
pub fn role_name(user: &UserRow) -> String {
    match (&user.custom_role_name, &user.custom_role) {
        (Some(name), _) => name.clone(),
        (None, Some(slug)) => slug.clone(),
        (None, None) => built_in_name(user.role).to_owned(),
    }
}

/// A role as a request names it.
#[derive(Clone, Debug)]
pub enum RoleChoice {
    /// One of the five fixed roles.
    BuiltIn(Role),
    /// A custom role.
    Custom(RoleRow),
}

impl RoleChoice {
    /// The role `name` says: a built-in role's name, else a custom role's
    /// slug.
    ///
    /// # Errors
    /// [`AppError::Validation`] when it is neither.
    pub async fn resolve(state: &AppState, name: &str) -> Result<Self, AppError> {
        if let Ok(role) = Role::parse(name) {
            return Ok(Self::BuiltIn(role));
        }
        match state.roles.get(name).await {
            Ok(role) => Ok(Self::Custom(role)),
            Err(AppError::NotFound { .. }) => {
                Err(AppError::validation(format!("unknown role: {name:?}")))
            }
            Err(err) => Err(err),
        }
    }

    /// How an account holding this role is stored: the built-in role
    /// (`subscriber` under a custom role) and the custom role's slug.
    #[must_use]
    pub fn stored(&self) -> (Role, Option<&str>) {
        match self {
            Self::BuiltIn(role) => (*role, None),
            Self::Custom(role) => (Role::Subscriber, Some(role.slug.as_str())),
        }
    }

    /// What the role grants.
    #[must_use]
    pub fn capabilities(&self) -> Vec<Capability> {
        match self {
            Self::BuiltIn(role) => role_caps(*role).to_vec(),
            Self::Custom(role) => known_caps(&role.capabilities),
        }
    }
}

/// The capabilities a stored list names (anything else grants nothing).
#[must_use]
pub fn known_caps(names: &[String]) -> Vec<Capability> {
    names.iter().filter_map(|name| parse_cap(name)).collect()
}

/// The built-in roles are not rows and do not change.
fn custom_only(slug: &str) -> Result<(), AppError> {
    if Role::parse(slug).is_ok() {
        return Err(AppError::validation(format!(
            "{slug:?} is a built-in role; it cannot be edited or deleted"
        )));
    }
    Ok(())
}

/// How many users hold the custom role `slug`.
async fn holders(state: &AppState, slug: &str) -> Result<i64, AppError> {
    Ok(state
        .roles
        .list()
        .await?
        .into_iter()
        .find(|(role, _)| role.slug == slug)
        .map_or(0, |(_, users)| users))
}

/// `GET /api/v1/roles` — every role: the built-in ones in their fixed
/// order, then the custom ones by name.
#[utoipa::path(
    get, path = "/api/v1/roles",
    tag = "roles",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Built-in roles, then custom roles by name", body = [RoleResponse]),
        (status = 403, description = "Missing manage_users", body = ApiErrorBody),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> ApiResult<Json<Vec<RoleResponse>>> {
    let held = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .count_by_builtin_role()
        .await?;
    let mut roles: Vec<RoleResponse> = Role::ALL
        .iter()
        .map(|role| {
            let users = held
                .iter()
                .find(|(name, _)| name == role.as_str())
                .map_or(0, |(_, users)| *users);
            RoleResponse::built_in(*role, users)
        })
        .collect();
    let mut custom = state.roles.list().await?;
    custom.sort_by_cached_key(|(role, _)| (role.name.to_lowercase(), role.slug.clone()));
    roles.extend(
        custom
            .into_iter()
            .map(|(role, users)| RoleResponse::custom(role, users)),
    );
    Ok(Json(roles))
}

/// `POST /api/v1/roles` — define a custom role. Every capability in it
/// must be one the caller holds.
#[utoipa::path(
    post, path = "/api/v1/roles",
    tag = "roles",
    security(("session_cookie" = [])),
    request_body = CreateRoleRequest,
    responses(
        (status = 201, description = "Created", body = RoleResponse),
        (status = 400, description = "Bad or reserved slug, bad name, or unknown capability", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the role holds a capability the caller lacks", body = ApiErrorBody),
        (status = 409, description = "The slug is taken", body = ApiErrorBody),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<CreateRoleRequest>,
) -> ApiResult<(StatusCode, Json<RoleResponse>)> {
    crate::policy::demo_role_capabilities(&state, &body.capabilities)?;
    let role = state
        .roles
        .create(
            &user.user,
            RoleInput {
                slug: body.slug,
                name: body.name,
                description: body.description,
                capabilities: body.capabilities,
            },
        )
        .await?;
    crate::audit::record(
        &state,
        &user.user,
        "role.create",
        format!("role:{}", role.slug),
        serde_json::json!({ "capabilities": role.capabilities }),
    );
    Ok((StatusCode::CREATED, Json(RoleResponse::custom(role, 0))))
}

/// `PATCH /api/v1/roles/{slug}` — change a custom role. The role must
/// hold only capabilities the caller holds, both as it is and as it will
/// be. Its users have the new capabilities from their next request.
#[utoipa::path(
    patch, path = "/api/v1/roles/{slug}",
    tag = "roles",
    security(("session_cookie" = [])),
    params(("slug" = String, Path, description = "Custom role slug")),
    request_body = UpdateRoleRequest,
    responses(
        (status = 200, description = "Updated", body = RoleResponse),
        (status = 400, description = "A built-in role, a bad or reserved slug, a bad name, or an unknown capability", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the role holds or would hold a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "No such custom role", body = ApiErrorBody),
        (status = 409, description = "The new slug is taken", body = ApiErrorBody),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
    Json(body): Json<UpdateRoleRequest>,
) -> ApiResult<Json<RoleResponse>> {
    custom_only(&slug)?;
    if let Some(capabilities) = &body.capabilities {
        crate::policy::demo_role_capabilities(&state, capabilities)?;
    }
    let role = state
        .roles
        .update(
            &user.user,
            &slug,
            RolePatch {
                slug: body.slug,
                name: body.name,
                description: body.description,
                capabilities: body.capabilities,
            },
        )
        .await?;
    crate::audit::record(
        &state,
        &user.user,
        "role.update",
        format!("role:{slug}"),
        serde_json::json!({ "slug": role.slug, "capabilities": role.capabilities }),
    );
    let users = holders(&state, &role.slug).await?;
    Ok(Json(RoleResponse::custom(role, users)))
}

/// `DELETE /api/v1/roles/{slug}` — remove a custom role nobody holds.
/// Every capability in it must be one the caller holds.
#[utoipa::path(
    delete, path = "/api/v1/roles/{slug}",
    tag = "roles",
    security(("session_cookie" = [])),
    params(("slug" = String, Path, description = "Custom role slug")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 400, description = "A built-in role", body = ApiErrorBody),
        (status = 403, description = "Missing manage_users, or the role holds a capability the caller lacks", body = ApiErrorBody),
        (status = 404, description = "No such custom role", body = ApiErrorBody),
        (status = 409, description = "Users still hold the role; the message says how many", body = ApiErrorBody),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(slug): Path<String>,
) -> ApiResult<StatusCode> {
    custom_only(&slug)?;
    state.roles.delete(&user.user, &slug).await?;
    crate::audit::record(
        &state,
        &user.user,
        "role.delete",
        format!("role:{slug}"),
        serde_json::json!({}),
    );
    Ok(StatusCode::NO_CONTENT)
}
