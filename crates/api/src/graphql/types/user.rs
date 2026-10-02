#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! User GraphQL type.

use async_graphql::{Context, Object};

use crate::graphql::context::GqlContext;

/// GraphQL wrapper for a user row (public fields only).
#[derive(Clone, Debug)]
pub struct GqlUser(pub vyasa_db::models::UserRow);

#[Object]
impl GqlUser {
    async fn id(&self) -> i64 {
        self.0.id
    }
    async fn email(&self, ctx: &Context<'_>) -> async_graphql::Result<String> {
        // Email visible only to self or users with ManageUsers.
        let gql = ctx.data::<GqlContext>()?;
        let is_self = gql
            .principal
            .as_ref()
            .is_some_and(|p| p.user().id == self.0.id);
        if is_self || gql.can(vyasa_core::user::Capability::ManageUsers) {
            Ok(self.0.email.clone())
        } else {
            // For others, hide email (return empty to avoid leaking).
            Ok(String::new())
        }
    }
    async fn username(&self) -> &str {
        &self.0.username
    }
    async fn display_name(&self) -> &str {
        &self.0.display_name
    }
    /// The built-in role. Under a custom role this is `subscriber`, and
    /// `customRole` says which role the user really holds.
    async fn role(&self) -> &str {
        self.0.role.as_str()
    }
    /// The slug of the user's custom role, or null on a built-in role.
    async fn custom_role(&self) -> Option<&str> {
        self.0.custom_role.as_deref()
    }
    /// What the user's role is called: the custom role's name, or the
    /// built-in role's.
    async fn role_name(&self) -> String {
        crate::rest::roles::role_name(&self.0)
    }
    async fn bio(&self) -> &str {
        &self.0.bio
    }
    async fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.0.created_at
    }
    async fn avatar_media_id(&self) -> Option<i64> {
        self.0.avatar_media_id
    }
}
