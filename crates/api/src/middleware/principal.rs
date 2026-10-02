//! Combined authentication principal: session cookie or API key.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use vyasa_common::AppError;

use super::api_key::ApiKeyPrincipal;
use super::auth::CurrentUser;
use crate::state::AppState;

/// Either authentication mode, for endpoints that serve both the admin
/// SPA (cookie) and headless consumers (API key).
#[derive(Clone, Debug)]
pub enum Principal {
    /// Browser session.
    Session(CurrentUser),
    /// Headless API key.
    ApiKey(ApiKeyPrincipal),
}

impl Principal {
    /// The authenticated user in either mode.
    #[must_use]
    #[allow(dead_code)] // first consumer arrives with post endpoints (phase 10)
    pub fn user(&self) -> &vyasa_db::models::UserRow {
        match self {
            Principal::Session(current) => &current.user,
            Principal::ApiKey(principal) => &principal.user,
        }
    }

    /// Ensures the principal holds `cap`.
    ///
    /// API keys must additionally declare the capability themselves, so
    /// a leaked key never exceeds its grants.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Forbidden`] otherwise.
    pub fn ensure(&self, cap: vyasa_core::user::Capability) -> Result<(), AppError> {
        match self {
            Principal::Session(current) => vyasa_core::user::ensure(&current.user, cap),
            Principal::ApiKey(principal) => principal.ensure(cap),
        }
    }
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = crate::error::ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Already resolved by the route guard (`crate::authz`): one lookup
        // per request.
        if let Some(resolved) = parts.extensions.get::<Principal>() {
            return Ok(resolved.clone());
        }
        // Bearer header present → API key auth.
        if parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| !t.is_empty())
        {
            let principal = ApiKeyPrincipal::from_request_parts(parts, state).await?;
            Ok(Principal::ApiKey(principal))
        } else {
            let current = CurrentUser::from_request_parts(parts, state).await?;
            Ok(Principal::Session(current))
        }
    }
}

impl Principal {
    /// What a plugin is allowed to know about the signed-in visitor.
    ///
    /// Name and role only: enough to greet someone or gate a members'
    /// section, and nothing a plugin could sign in with. In particular no
    /// email address, which is both a credential and personal data.
    ///
    /// `role` is the role the account really holds: a custom role's slug
    /// when one is assigned, the built-in role's name otherwise. The
    /// built-in role under a custom one is always `subscriber`, which
    /// would tell a plugin gating a members' section the wrong thing. The
    /// WIT `viewer` record is part of the plugin contract and has no
    /// field for a display name, so none is added here.
    #[must_use]
    pub fn viewer(&self) -> vyasa_plugins::host::Viewer {
        let user = self.user();
        vyasa_plugins::host::Viewer {
            id: user.id,
            display_name: user.display_name.clone(),
            role: user
                .custom_role
                .clone()
                .unwrap_or_else(|| user.role.as_str().to_owned()),
        }
    }
}

/// Optional principal for public endpoints (e.g. comment submission).
#[derive(Clone, Debug)]
pub struct MaybePrincipal(pub Option<Principal>);

impl FromRequestParts<AppState> for MaybePrincipal {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match Principal::from_request_parts(parts, state).await {
            Ok(principal) => Ok(Self(Some(principal))),
            Err(_) => Ok(Self(None)),
        }
    }
}
