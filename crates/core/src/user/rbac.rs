//! Role-based access control: capabilities, role grants, per-user
//! overrides, and the `ensure` gate.
//!
//! Capabilities are additive; a user's effective set is
//! `ROLE_CAPS[role] ∪ enabled_caps(meta) − disabled_caps(meta)`, where a
//! custom role's own list stands in for `ROLE_CAPS[role]` when the user
//! has one.

use serde::{Deserialize, Serialize};

use vyasa_common::AppError;
use vyasa_db::models::{Role, UserRow};

/// A discrete permission. Roles map to sets of these (see [`ROLE_CAPS`]);
/// per-user overrides can add or remove.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Create and edit own draft content.
    EditPosts,
    /// Publish own content.
    PublishPosts,
    /// Edit content authored by others.
    EditOthers,
    /// Delete content (own or others, per role).
    DeletePosts,
    /// Manage categories and tags.
    ManageCategories,
    /// Moderate comments (approve/spam/trash).
    ModerateComments,
    /// Upload media files.
    UploadMedia,
    /// Create, edit, and delete users.
    ManageUsers,
    /// Install, activate, and remove themes.
    ManageThemes,
    /// Install, enable, and remove plugins.
    ManagePlugins,
    /// Change site options/settings.
    ManageOptions,
    /// Access the admin area at all.
    ViewAdmin,
}

impl Capability {
    /// All capabilities (iteration/testing convenience).
    pub const ALL: [Capability; 12] = [
        Capability::EditPosts,
        Capability::PublishPosts,
        Capability::EditOthers,
        Capability::DeletePosts,
        Capability::ManageCategories,
        Capability::ModerateComments,
        Capability::UploadMedia,
        Capability::ManageUsers,
        Capability::ManageThemes,
        Capability::ManagePlugins,
        Capability::ManageOptions,
        Capability::ViewAdmin,
    ];
}

/// The static role → capability grants.
///
/// | role | grants |
/// |---|---|
/// | admin | everything |
/// | editor | content + comments + media incl. others' |
/// | author | own content + publish + media |
/// | contributor | own drafts, no publish, no media upload |
/// | subscriber | admin access only (profile) |
#[must_use]
pub fn role_caps(role: Role) -> &'static [Capability] {
    match role {
        Role::Admin => &[
            Capability::EditPosts,
            Capability::PublishPosts,
            Capability::EditOthers,
            Capability::DeletePosts,
            Capability::ManageCategories,
            Capability::ModerateComments,
            Capability::UploadMedia,
            Capability::ManageUsers,
            Capability::ManageThemes,
            Capability::ManagePlugins,
            Capability::ManageOptions,
            Capability::ViewAdmin,
        ],
        Role::Editor => &[
            Capability::EditPosts,
            Capability::PublishPosts,
            Capability::EditOthers,
            Capability::DeletePosts,
            Capability::ManageCategories,
            Capability::ModerateComments,
            Capability::UploadMedia,
            Capability::ViewAdmin,
        ],
        Role::Author => &[
            Capability::EditPosts,
            Capability::PublishPosts,
            Capability::UploadMedia,
            Capability::ViewAdmin,
        ],
        Role::Contributor => &[Capability::EditPosts, Capability::ViewAdmin],
        Role::Subscriber => &[Capability::ViewAdmin],
    }
}

/// Computes a user's effective capabilities including meta overrides.
///
/// A user with a custom role starts from exactly that role's capabilities
/// (carried on the row by every user query), not from the built-in table:
/// the `subscriber` base role underneath grants nothing on its own. Names
/// the role lists that are not capabilities are ignored.
///
/// The `users.meta` JSONB may carry `{"enabled_caps": [...],`
/// `"disabled_caps": [...]}` (string names, snake_case); they apply on
/// top of either kind of role.
#[must_use]
pub fn effective_caps(user: &UserRow) -> Vec<Capability> {
    let mut caps: Vec<Capability> = match (&user.custom_role_caps, &user.custom_role) {
        (Some(names), _) => {
            let mut caps = Vec::with_capacity(names.len());
            for cap in names.iter().filter_map(|name| parse_cap(name)) {
                if !caps.contains(&cap) {
                    caps.push(cap);
                }
            }
            caps
        }
        // A custom role whose capabilities are not on the row grants
        // nothing, rather than the base role's grants.
        (None, Some(_)) => Vec::new(),
        (None, None) => role_caps(user.role).to_vec(),
    };
    let meta = user.meta.as_object();
    if let Some(enabled) = meta
        .and_then(|m| m.get("enabled_caps"))
        .and_then(|v| v.as_array())
    {
        for name in enabled.iter().filter_map(|v| v.as_str()) {
            if let Some(cap) = parse_cap(name) {
                if !caps.contains(&cap) {
                    caps.push(cap);
                }
            }
        }
    }
    if let Some(disabled) = meta
        .and_then(|m| m.get("disabled_caps"))
        .and_then(|v| v.as_array())
    {
        for name in disabled.iter().filter_map(|v| v.as_str()) {
            if let Some(cap) = parse_cap(name) {
                caps.retain(|c| *c != cap);
            }
        }
    }
    caps
}

/// Whether `user` holds `cap`.
#[must_use]
pub fn can(user: &UserRow, cap: Capability) -> bool {
    effective_caps(user).contains(&cap)
}

/// Ensures `user` holds `cap`.
///
/// # Errors
///
/// Returns [`AppError::Forbidden`] when the capability is missing.
pub fn ensure(user: &UserRow, cap: Capability) -> Result<(), AppError> {
    if can(user, cap) {
        Ok(())
    } else {
        Err(AppError::forbidden(format!(
            "this action requires the {} capability",
            cap_name(cap)
        )))
    }
}

/// Parses a capability from its snake_case wire name.
#[must_use]
pub fn parse_cap(name: &str) -> Option<Capability> {
    match name {
        "edit_posts" => Some(Capability::EditPosts),
        "publish_posts" => Some(Capability::PublishPosts),
        "edit_others" => Some(Capability::EditOthers),
        "delete_posts" => Some(Capability::DeletePosts),
        "manage_categories" => Some(Capability::ManageCategories),
        "moderate_comments" => Some(Capability::ModerateComments),
        "upload_media" => Some(Capability::UploadMedia),
        "manage_users" => Some(Capability::ManageUsers),
        "manage_themes" => Some(Capability::ManageThemes),
        "manage_plugins" => Some(Capability::ManagePlugins),
        "manage_options" => Some(Capability::ManageOptions),
        "view_admin" => Some(Capability::ViewAdmin),
        _ => None,
    }
}

/// Returns the snake_case wire name of a capability.
#[must_use]
pub fn cap_name(cap: Capability) -> &'static str {
    match cap {
        Capability::EditPosts => "edit_posts",
        Capability::PublishPosts => "publish_posts",
        Capability::EditOthers => "edit_others",
        Capability::DeletePosts => "delete_posts",
        Capability::ManageCategories => "manage_categories",
        Capability::ModerateComments => "moderate_comments",
        Capability::UploadMedia => "upload_media",
        Capability::ManageUsers => "manage_users",
        Capability::ManageThemes => "manage_themes",
        Capability::ManagePlugins => "manage_plugins",
        Capability::ManageOptions => "manage_options",
        Capability::ViewAdmin => "view_admin",
    }
}

#[cfg(test)]
mod tests {
    use super::{can, cap_name, effective_caps, ensure, parse_cap, role_caps, Capability};
    use vyasa_common::AppError;
    use vyasa_db::models::{Role, UserRow};

    fn user(role: Role, meta: serde_json::Value) -> UserRow {
        UserRow {
            id: 1,
            email: "u@example.com".into(),
            username: "u".into(),
            display_name: "U".into(),
            password_hash: None,
            role,
            bio: String::new(),
            avatar_media_id: None,
            meta,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            suspended_at: None,
            custom_role: None,
            custom_role_name: None,
            custom_role_caps: None,
            email_verified_at: Some(chrono::Utc::now()),
        }
    }

    fn custom(caps: &[&str], meta: serde_json::Value) -> UserRow {
        UserRow {
            custom_role: Some("moderator".into()),
            custom_role_name: Some("Moderator".into()),
            custom_role_caps: Some(caps.iter().map(|c| (*c).to_owned()).collect()),
            ..user(Role::Subscriber, meta)
        }
    }

    #[test]
    fn a_custom_role_grants_exactly_its_capabilities() {
        let u = custom(
            &["moderate_comments", "upload_media"],
            serde_json::json!({}),
        );
        assert_eq!(
            effective_caps(&u),
            [Capability::ModerateComments, Capability::UploadMedia]
        );
        // Not even the base role's view_admin unless the role lists it.
        assert!(!can(&u, Capability::ViewAdmin));

        let nothing = custom(&[], serde_json::json!({}));
        assert!(effective_caps(&nothing).is_empty());
    }

    #[test]
    fn a_custom_role_ignores_unknown_and_repeated_names() {
        let u = custom(
            &["root", "edit_posts", "edit_posts", "Manage_Users"],
            serde_json::json!({}),
        );
        assert_eq!(effective_caps(&u), [Capability::EditPosts]);
    }

    #[test]
    fn meta_overrides_still_apply_on_top_of_a_custom_role() {
        let u = custom(
            &["moderate_comments", "upload_media"],
            serde_json::json!({
                "enabled_caps": ["edit_posts"],
                "disabled_caps": ["upload_media"]
            }),
        );
        assert_eq!(
            effective_caps(&u),
            [Capability::ModerateComments, Capability::EditPosts]
        );
    }

    #[test]
    fn a_custom_role_whose_capabilities_did_not_load_grants_nothing() {
        // A row built without the roles lookup must not fall back to the
        // built-in table.
        let u = UserRow {
            custom_role: Some("moderator".into()),
            ..user(Role::Subscriber, serde_json::json!({}))
        };
        assert!(effective_caps(&u).is_empty());
    }

    #[test]
    fn every_capability_has_a_round_tripping_name() {
        for cap in Capability::ALL {
            assert_eq!(parse_cap(cap_name(cap)), Some(cap));
        }
    }

    #[test]
    fn role_matrix() {
        let admin = user(Role::Admin, serde_json::json!({}));
        let editor = user(Role::Editor, serde_json::json!({}));
        let author = user(Role::Author, serde_json::json!({}));
        let contributor = user(Role::Contributor, serde_json::json!({}));
        let subscriber = user(Role::Subscriber, serde_json::json!({}));

        for cap in Capability::ALL {
            assert!(can(&admin, cap), "admin must have {cap:?}");
        }
        assert!(can(&editor, Capability::EditOthers));
        assert!(!can(&editor, Capability::ManageUsers));
        assert!(can(&author, Capability::PublishPosts));
        assert!(!can(&author, Capability::EditOthers));
        assert!(can(&contributor, Capability::EditPosts));
        assert!(!can(&contributor, Capability::PublishPosts));
        assert!(!can(&subscriber, Capability::EditPosts));
        assert!(can(&subscriber, Capability::ViewAdmin));

        // Every role's grant set only contains known capabilities.
        for role in Role::ALL {
            for cap in role_caps(role) {
                assert_eq!(parse_cap(cap_name(*cap)), Some(*cap));
            }
        }
    }

    #[test]
    fn meta_overrides_add_and_remove() {
        let promoted = user(
            Role::Contributor,
            serde_json::json!({"enabled_caps": ["upload_media"]}),
        );
        assert!(can(&promoted, Capability::UploadMedia));

        let demoted = user(
            Role::Author,
            serde_json::json!({"disabled_caps": ["publish_posts"]}),
        );
        assert!(!can(&demoted, Capability::PublishPosts));
        assert!(can(&demoted, Capability::EditPosts));

        // Disabled wins when a cap appears in both lists.
        let both = user(
            Role::Author,
            serde_json::json!({
                "enabled_caps": ["manage_users"],
                "disabled_caps": ["manage_users"]
            }),
        );
        assert!(!can(&both, Capability::ManageUsers));
    }

    #[test]
    fn unknown_override_names_are_ignored() {
        let u = user(
            Role::Subscriber,
            serde_json::json!({"enabled_caps": ["root", 42]}),
        );
        assert_eq!(effective_caps(&u).len(), 1); // view_admin only
    }

    #[test]
    fn ensure_returns_forbidden_with_capability_name() {
        let subscriber = user(Role::Subscriber, serde_json::json!({}));
        let err = ensure(&subscriber, Capability::ManagePlugins);
        assert!(matches!(err, Err(AppError::Forbidden { .. })));
        let message = err.err().map_or(String::new(), |e| e.to_string());
        assert!(message.contains("manage_plugins"), "message: {message}");
    }
}
