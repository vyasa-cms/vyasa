//! Row-model structs for the database tables.
//!
//! These are plain data carriers with sqlx row mapping. Domain rules and
//! repository semantics live elsewhere (`vyasa-core` and
//! `vyasa-db::repo` in later phases).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

/// User roles. The set mirrors the `users_role_check` constraint in
/// migration 0001; RBAC semantics (which role gets which capability) are
/// defined in `vyasa-core` (phase 07).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Full site control.
    Admin,
    /// Publishes and manages all content.
    Editor,
    /// Writes and publishes own content.
    Author,
    /// Writes drafts; cannot publish.
    Contributor,
    /// Reads only (comments, profile).
    Subscriber,
}

impl Role {
    /// All roles in declaration order.
    pub const ALL: [Role; 5] = [
        Role::Admin,
        Role::Editor,
        Role::Author,
        Role::Contributor,
        Role::Subscriber,
    ];

    /// Parses a role from its database string form.
    ///
    /// # Errors
    ///
    /// Returns [`vyasa_common::AppError::Validation`] for unknown role
    /// strings.
    pub fn parse(value: &str) -> Result<Self, vyasa_common::AppError> {
        match value {
            "admin" => Ok(Role::Admin),
            "editor" => Ok(Role::Editor),
            "author" => Ok(Role::Author),
            "contributor" => Ok(Role::Contributor),
            "subscriber" => Ok(Role::Subscriber),
            other => Err(vyasa_common::AppError::validation(format!(
                "unknown role: {other:?}"
            ))),
        }
    }

    /// Returns the database string form of this role.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Editor => "editor",
            Role::Author => "author",
            Role::Contributor => "contributor",
            Role::Subscriber => "subscriber",
        }
    }
}

/// A row of the `users` table.
///
/// `password_hash` must never be serialized to API clients; see the auth
/// phase for response DTOs.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct UserRow {
    /// Snowflake id.
    pub id: i64,
    /// Case-insensitive-unique email.
    pub email: String,
    /// Case-insensitive-unique username.
    pub username: String,
    /// Display name shown in UIs.
    pub display_name: String,
    /// Argon2 hash; `None` for passwordless/invited accounts.
    #[serde(skip_serializing)]
    pub password_hash: Option<String>,
    /// Site role.
    pub role: Role,
    /// Free-form biography.
    pub bio: String,
    /// Referenced media row for the avatar, if any.
    pub avatar_media_id: Option<i64>,
    /// Extension metadata (per-user preferences, capability overrides).
    pub meta: serde_json::Value,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Last successful sign-in, if ever.
    #[sqlx(default)]
    pub last_login_at: Option<DateTime<Utc>>,
    /// Set while an administrator has suspended the account.
    #[sqlx(default)]
    pub suspended_at: Option<DateTime<Utc>>,
    /// Slug of the administrator-defined role, if one is assigned. The
    /// base [`Self::role`] is then `subscriber`.
    ///
    /// The three `custom_role*` fields have no `sqlx(default)` on purpose:
    /// a query that forgets them fails to map instead of yielding a user
    /// without its role's capabilities. Select through
    /// [`crate::repo::users::USER_COLUMNS`].
    pub custom_role: Option<String>,
    /// Display name of the custom role (from `roles`).
    pub custom_role_name: Option<String>,
    /// Capability names the custom role grants (from `roles`); `None`
    /// without a custom role.
    pub custom_role_caps: Option<Vec<String>>,
    /// When the address was confirmed. `None` only for an account made
    /// through public registration that has not followed its confirmation
    /// link yet: it cannot sign in or be resolved from an API key. Every
    /// other way of creating an account confirms it at creation.
    ///
    /// No `sqlx(default)`, for the same reason as the role fields: a query
    /// that forgets the column must fail, not yield an account that looks
    /// unconfirmed (or, worse, one that does not).
    pub email_verified_at: Option<DateTime<Utc>>,
}

/// What a row of `reset_tokens` may be redeemed for. A token is only ever
/// accepted for the purpose it was issued for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenPurpose {
    /// Setting a password: a reset, or accepting an invitation.
    Reset,
    /// Confirming the address of an account made by registration.
    Verify,
}

impl TokenPurpose {
    /// The database string form (`reset_tokens.purpose`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TokenPurpose::Reset => "reset",
            TokenPurpose::Verify => "verify",
        }
    }
}

/// A row of the `roles` table: an administrator-defined role.
#[derive(Clone, Debug, PartialEq, Eq, FromRow, Serialize)]
pub struct RoleRow {
    /// Identifier, `^[a-z0-9][a-z0-9-]{1,39}$`, never a built-in role name.
    pub slug: String,
    /// Display name (1-60 characters).
    pub name: String,
    /// What the role is for.
    pub description: String,
    /// Capability names (snake_case) the role grants.
    pub capabilities: Vec<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

/// A row of the `sessions` table. The primary key is the session token.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SessionRow {
    /// Session token (the primary key).
    #[serde(skip_serializing)]
    pub id: String,
    /// Owning user.
    pub user_id: i64,
    /// Expiry; stale rows are purgeable afterwards.
    pub expires_at: DateTime<Utc>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Browser user-agent captured at login, if any.
    pub user_agent: Option<String>,
    /// Client IP captured at login, if any.
    pub ip: Option<String>,
}

/// A row of the `api_keys` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct ApiKeyRow {
    /// Snowflake id.
    pub id: i64,
    /// Owning user.
    pub user_id: i64,
    /// Human label, e.g. "Next.js frontend".
    pub name: String,
    /// Hash of the raw key; the raw key exists only at creation.
    #[serde(skip_serializing)]
    pub key_hash: String,
    /// Granted capabilities (array of strings).
    pub capabilities: serde_json::Value,
    /// Last successful authentication, if any.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Revocation time; `None` while active.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A row of the `options` table.
#[derive(Clone, Debug, FromRow, Serialize)]
pub struct OptionRow {
    /// Option name.
    pub key: String,
    /// JSONB value.
    pub value: serde_json::Value,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::Role;

    #[test]
    fn role_round_trips_with_database_form() {
        for role in Role::ALL {
            let parsed = Role::parse(role.as_str()).unwrap_or(Role::Subscriber);
            assert_eq!(parsed, role);
        }
    }

    #[test]
    fn role_parse_rejects_unknown_values() {
        assert!(Role::parse("root").is_err());
        assert!(Role::parse("").is_err());
        assert!(Role::parse("ADMIN").is_err());
    }
    #[test]
    fn role_serializes_lowercase() {
        let json = serde_json::to_string(&Role::Editor).expect("serialize role");
        assert_eq!(json, "\"editor\"");
    }
}
