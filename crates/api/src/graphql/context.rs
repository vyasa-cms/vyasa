#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! GraphQL context.

use crate::middleware::Principal;
use crate::state::AppState;

/// Request-scoped GraphQL context.
#[derive(Clone)]
pub struct GqlContext {
    /// Shared app state.
    pub state: AppState,
    /// Optional authenticated principal (session or api key).
    pub principal: Option<Principal>,
}

impl GqlContext {
    /// Returns the principal or an auth error.
    pub fn require_principal(&self) -> Result<&Principal, async_graphql::Error> {
        self.principal
            .as_ref()
            .ok_or_else(|| async_graphql::Error::new("authentication required"))
    }

    /// Whether the current principal holds `cap` (false when unauthenticated).
    pub fn can(&self, cap: vyasa_core::user::Capability) -> bool {
        self.principal
            .as_ref()
            .is_some_and(|p| p.ensure(cap).is_ok())
    }
}
