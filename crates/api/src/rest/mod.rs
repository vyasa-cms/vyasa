//! REST routes.

pub mod ai;
pub mod ai_assist;
pub mod ai_features;
pub mod ai_models;
pub mod analytics;
pub mod api_keys;
pub mod audience;
pub mod auth;
pub mod comments;
pub mod content_types;
pub mod docs;
pub mod embed_preview;
pub mod export;
pub mod forms;
pub mod language;
pub mod link_suggest;
pub mod locks;
pub mod mail;
pub mod media;
pub mod menus;
pub mod options;
pub mod patterns;
pub mod plugins;
pub mod posts;
pub mod privacy;
pub mod registration;
pub mod registry;
pub mod roles;
pub mod search;
pub mod seo;
pub mod setup;
pub mod site_health;
pub mod terms;
pub mod theme_studio;
pub mod themes;
pub mod updates;
pub mod users;
pub mod webhooks;

use axum::Router;
use serde::Serialize;
use vyasa_core::user::Capability;

use crate::authz::{Access, Guarded, RouteRule};
use crate::state::AppState;

/// Response shape for any user (no secrets).
#[derive(Serialize, utoipa::ToSchema)]
pub struct UserResponse {
    /// User id.
    pub id: i64,
    /// Email address.
    pub email: String,
    /// Username.
    pub username: String,
    /// Display name.
    pub display_name: String,
    /// Built-in role. `subscriber` for a user with a custom role, whose
    /// capabilities then come from `custom_role` alone.
    pub role: String,
    /// The custom role's slug, when the user has one.
    pub custom_role: Option<String>,
    /// What the user's role is called: the custom role's name, or the
    /// built-in role's.
    pub role_name: String,
    /// Biography.
    pub bio: String,
    /// Last successful sign-in, if ever.
    pub last_login_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Set while suspended.
    pub suspended_at: Option<chrono::DateTime<chrono::Utc>>,
    /// False for an invited account that has not set a password yet.
    pub has_password: bool,
    /// The account's email address is confirmed. False only for an
    /// account made by registration whose owner has not followed the
    /// link yet; it cannot sign in until then (phase 98).
    pub email_verified: bool,
    /// A second factor is on (filled by the users list only).
    #[serde(default)]
    pub mfa_enabled: bool,
    /// The chosen avatar, served by the media endpoint.
    pub avatar_url: Option<String>,
    /// Creation time.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<vyasa_db::models::UserRow> for UserResponse {
    fn from(user: vyasa_db::models::UserRow) -> Self {
        let role_name = roles::role_name(&user);
        Self {
            id: user.id,
            email: user.email,
            username: user.username,
            display_name: user.display_name,
            role: user.role.as_str().to_string(),
            role_name,
            custom_role: user.custom_role,
            bio: user.bio,
            created_at: user.created_at,
            last_login_at: user.last_login_at,
            suspended_at: user.suspended_at,
            has_password: user.password_hash.as_deref().is_some_and(|h| !h.is_empty()),
            email_verified: user.email_verified_at.is_some(),
            mfa_enabled: false,
            avatar_url: user
                .avatar_media_id
                .map(|id| format!("/api/v1/media/{id}/raw")),
        }
    }
}

impl UserResponse {
    /// Maps a user row reference to its public shape.
    #[must_use]
    pub fn from_row(user: &vyasa_db::models::UserRow) -> Self {
        Self::from(user.clone())
    }
}

/// Reading media metadata: an uploader, or anyone who writes entries.
const MEDIA_READERS: &[Capability] = &[Capability::UploadMedia, Capability::EditPosts];

/// Browsing the registry: whoever may install either kind of listing.
const REGISTRY_BROWSERS: &[Capability] = &[Capability::ManagePlugins, Capability::ManageThemes];

/// Writing and reading entries: CRUD, revisions, previews, composition.
///
/// Split out because the whole `/api/v1` router had grown past what fits in
/// one readable function, and entries are its largest coherent group.
///
/// Reads are `Authenticated`: what a caller may see depends on the entry
/// (status, author, password token), which the handlers decide.
fn post_routes() -> Guarded {
    use Access::{Authenticated, Cap, Public};
    use Capability::EditPosts;
    Guarded::new()
        .at("/posts", |r| {
            r.get(Authenticated, posts::list)
                .post(Cap(EditPosts), posts::create)
        })
        .at("/posts/batch", |r| r.post(Cap(EditPosts), posts::batch))
        .at("/posts/{id}/duplicate", |r| {
            r.post(Cap(EditPosts), posts::duplicate)
        })
        .at("/posts/{id}", |r| {
            r.get(Authenticated, posts::get)
                .put(Cap(EditPosts), posts::update)
                .delete(Cap(EditPosts), posts::trash)
        })
        .at("/posts/slug/{type}/{slug}", |r| {
            r.get(Authenticated, posts::get_by_slug)
        })
        .at("/posts/{id}/restore", |r| {
            r.post(Cap(EditPosts), posts::restore)
        })
        .at("/posts/{id}/verify-password", |r| {
            r.post(Authenticated, posts::verify_password)
        })
        .at("/posts/{id}/revisions", |r| {
            r.get(Cap(EditPosts), posts::list_revisions)
                .post(Cap(EditPosts), posts::save_revision)
        })
        .at("/posts/{id}/revisions/{rid}", |r| {
            r.get(Cap(EditPosts), posts::get_revision)
        })
        .at("/posts/{id}/revisions/{rid}/restore", |r| {
            r.post(Cap(EditPosts), posts::restore_revision)
        })
        .at("/posts/{id}/autosave", |r| {
            r.put(Cap(EditPosts), posts::autosave)
        })
        .at("/posts/{id}/compose", |r| {
            r.post(Cap(EditPosts), posts::compose)
        })
        .at("/posts/{id}/language", |r| {
            r.get(Authenticated, language::get)
                .put(Cap(EditPosts), language::put)
        })
        .at("/posts/{id}/render", |r| {
            r.post(Cap(EditPosts), posts::render)
        })
        .at("/posts/{id}/preview-token", |r| {
            r.post(Cap(EditPosts), posts::preview_token)
        })
        // Gated by the signed preview token in the query, not a principal.
        .at("/posts/{id}/preview", |r| r.get(Public, posts::preview))
        .at("/posts/{id}/terms", |r| {
            r.get(Authenticated, terms::list_post_terms)
                .post(Cap(EditPosts), terms::set_post_terms)
        })
}

/// Plugins: install, inspect, lifecycle, settings, surface, audit.
fn plugin_routes() -> Guarded {
    let access = Access::Cap(Capability::ManagePlugins);
    // A package is capped at 10 MiB by the parser; the body limit sits
    // just above it so the parser's message, not a bare 413, explains.
    let package_limit = || {
        axum::extract::DefaultBodyLimit::max(
            vyasa_plugins::package::MAX_PACKAGE_BYTES + 1024 * 1024,
        )
    };
    Guarded::new()
        .at_layered(
            "/plugins",
            |r| r.post(access, plugins::install),
            |methods| methods.layer(package_limit()),
        )
        // Registered apart from the install above so the body limit stays
        // on the upload alone, as it always was.
        .at("/plugins", |r| r.get(access, plugins::list))
        .at_layered(
            "/plugins/inspect",
            |r| r.post(access, plugins::inspect),
            |methods| methods.layer(package_limit()),
        )
        .at("/plugins/surface", |r| r.get(access, plugins::surface))
        .at("/plugins/{id}/audit", |r| r.get(access, plugins::audit))
        .at("/plugins/settings/{id}", |r| {
            r.get(access, plugins::settings_all)
        })
        .at("/plugins/{id}", |r| r.delete(access, plugins::delete))
        .at("/plugins/{id}/enable", |r| r.post(access, plugins::enable))
        .at("/plugins/{id}/disable", |r| {
            r.post(access, plugins::disable)
        })
        .at("/plugins/{id}/rollback", |r| {
            r.post(access, plugins::rollback)
        })
        .at("/plugins/settings/{id}/{key}", |r| {
            r.get(access, plugins::setting_get)
                .put(access, plugins::setting_put)
        })
}

/// Accounts: listing, creation, identity, role, suspension, links; and
/// the roles they can be given.
///
/// `Authenticated` where the handler lets a user act on their own account
/// and asks for `manage_users` only when the target is someone else.
fn user_routes() -> Guarded {
    use Access::{Authenticated, Cap};
    use Capability::ManageUsers;
    Guarded::new()
        .at("/users", |r| {
            r.get(Cap(ManageUsers), users::list)
                .post(Cap(ManageUsers), users::create)
        })
        .at("/users/me", |r| r.put(Authenticated, users::update_me))
        .at("/users/{id}", |r| {
            r.get(Authenticated, users::get)
                .delete(Cap(ManageUsers), users::delete)
                .patch(Cap(ManageUsers), users::update)
        })
        .at("/users/{id}/role", |r| {
            r.put(Cap(ManageUsers), users::set_role)
        })
        .at("/users/{id}/suspend", |r| {
            r.post(Cap(ManageUsers), users::suspend)
        })
        .at("/users/{id}/sessions/revoke", |r| {
            r.post(Authenticated, users::revoke_sessions)
        })
        .at("/users/{id}/reset-link", |r| {
            r.post(Cap(ManageUsers), users::reset_link)
        })
        .at("/users/{id}/mfa", |r| {
            r.delete(Cap(ManageUsers), users::reset_mfa)
        })
        .at("/users/{id}/confirm", |r| {
            r.post(Cap(ManageUsers), users::confirm)
        })
        .at("/users/{id}/resend-confirmation", |r| {
            r.post(Cap(ManageUsers), users::resend_confirmation)
        })
        .at("/roles", |r| {
            r.get(Cap(ManageUsers), roles::list)
                .post(Cap(ManageUsers), roles::create)
        })
        .at("/roles/{slug}", |r| {
            r.patch(Cap(ManageUsers), roles::update)
                .delete(Cap(ManageUsers), roles::delete)
        })
}

/// Outbound webhooks: subscriptions, secrets, tests and history.
fn webhook_routes() -> Guarded {
    let access = Access::Cap(Capability::ManagePlugins);
    Guarded::new()
        .at("/webhooks", |r| {
            r.post(access, webhooks::create).get(access, webhooks::list)
        })
        .at("/webhooks/{id}", |r| {
            r.delete(access, webhooks::delete)
                .patch(access, webhooks::update)
        })
        .at("/webhooks/{id}/test", |r| {
            r.post(access, webhooks::test_send)
        })
        .at("/webhooks/{id}/rotate-secret", |r| {
            r.post(access, webhooks::rotate_secret)
        })
        .at("/webhooks/{id}/deliveries", |r| {
            r.get(access, webhooks::deliveries)
        })
        .at("/webhooks/{id}/deliveries/{delivery_id}/redeliver", |r| {
            r.post(access, webhooks::redeliver)
        })
}

/// Site health and the mail relay: the operator's pages.
fn ops_routes() -> Guarded {
    use Access::Cap;
    use Capability::{EditOthers, EditPosts, ManageOptions, ManageUsers};
    Guarded::new()
        .at("/site-health", |r| {
            r.get(Cap(ManageOptions), site_health::get)
        })
        .at("/site-health/cleanup", |r| {
            r.post(Cap(ManageOptions), site_health::cleanup)
        })
        .at("/mail/settings", |r| {
            r.get(Cap(ManageOptions), mail::get)
                .put(Cap(ManageOptions), mail::put)
        })
        .at("/mail/test", |r| r.post(Cap(ManageOptions), mail::test))
        .at("/privacy/export", |r| {
            r.get(Cap(ManageUsers), privacy::export)
        })
        .at("/privacy/erase", |r| {
            r.post(Cap(ManageUsers), privacy::erase)
        })
        .at("/privacy/policy-page", |r| {
            r.post(Cap(ManageOptions), privacy::policy_page)
        })
        // Forms are site-wide, not authored content: their inboxes hold
        // visitors' names, emails and messages, and `notify_email` decides
        // where every future submission is mailed. `EditPosts` (which
        // Contributors hold) was far too little; `ManageOptions` (Admin
        // only) would lock Editors out of an inbox that is theirs to run.
        // `EditOthers` already means "may act on content that is not yours".
        .at("/forms", |r| {
            r.get(Cap(EditOthers), forms::list)
                .post(Cap(EditOthers), forms::create)
        })
        .at("/forms/submissions/read", |r| {
            r.post(Cap(EditOthers), forms::mark_read)
        })
        .at("/forms/{id}", |r| {
            r.put(Cap(EditOthers), forms::update)
                .delete(Cap(EditOthers), forms::remove)
        })
        .at("/forms/{id}/submissions", |r| {
            r.get(Cap(EditOthers), forms::submissions)
        })
        .at("/forms/{id}/submissions.csv", |r| {
            r.get(Cap(EditOthers), forms::submissions_csv)
        })
        .at("/patterns", |r| {
            r.get(Cap(EditPosts), patterns::list)
                .post(Cap(EditPosts), patterns::create)
        })
        .at("/patterns/{id}", |r| {
            r.get(Cap(EditPosts), patterns::get)
                .put(Cap(EditPosts), patterns::update)
                .delete(Cap(EditPosts), patterns::remove)
        })
        .at("/export", |r| r.get(Cap(ManageOptions), export::get))
        .at_layered(
            "/import",
            |r| r.post(Cap(ManageOptions), export::post),
            |methods| methods.layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
}

/// The first-run wizard's endpoints, kept together.
///
/// `Public` at the route: before the first administrator exists the caller
/// holds a setup token, not a session, and `setup::SetupPrincipal` decides
/// between the two inside each handler.
fn setup_routes() -> Guarded {
    use Access::Public;
    Guarded::new()
        .at("/setup/status", |r| r.get(Public, setup::status))
        .at("/setup/claim", |r| r.post(Public, setup::claim))
        .at("/setup/checks", |r| r.get(Public, setup::checks))
        .at("/setup/account", |r| r.post(Public, setup::account))
        .at("/setup/site", |r| r.post(Public, setup::site))
        .at("/setup/content", |r| r.post(Public, setup::content))
        .at("/setup/verify-url", |r| r.get(Public, setup::verify_url))
        .at("/setup/delivery", |r| r.post(Public, setup::delivery))
        .at("/setup/mail", |r| r.post(Public, setup::mail))
        .at("/setup/mail/test", |r| r.post(Public, setup::mail_test))
        .at("/setup/assistants", |r| r.post(Public, setup::assistants))
        .at("/setup/updates", |r| r.post(Public, setup::updates))
        .at("/setup/keypair", |r| r.post(Public, setup::keypair))
        .at("/setup/finish", |r| r.post(Public, setup::finish))
}

/// Sign-in, the signed-in account, and its API keys.
fn auth_routes() -> Guarded {
    use Access::{Authenticated, Public};
    Guarded::new()
        .at("/auth/forgot", |r| r.post(Public, auth::forgot))
        .at("/auth/reset", |r| r.post(Public, auth::reset))
        .at("/auth/login", |r| r.post(Public, auth::login))
        // Registration (phase 98): each handler refuses on its own when
        // registration is off or cannot mail, and limits by client and
        // email address.
        .at("/auth/registration", |r| {
            r.get(Public, registration::status)
        })
        .at("/auth/register", |r| r.post(Public, registration::register))
        .at("/auth/register/resend", |r| {
            r.post(Public, registration::resend)
        })
        .at("/auth/verify", |r| r.post(Public, registration::verify))
        .at("/auth/mfa", |r| r.post(Public, auth::mfa_login))
        .at("/auth/mfa/status", |r| {
            r.get(Authenticated, auth::mfa_status)
        })
        .at("/auth/mfa/setup", |r| {
            r.post(Authenticated, auth::mfa_setup)
        })
        .at("/auth/mfa/confirm", |r| {
            r.post(Authenticated, auth::mfa_confirm)
        })
        .at("/auth/mfa/disable", |r| {
            r.post(Authenticated, auth::mfa_disable)
        })
        .at("/auth/logout", |r| r.post(Authenticated, auth::logout))
        .at("/auth/me", |r| r.get(Authenticated, auth::me))
        .at("/auth/me/caps", |r| r.get(Authenticated, users::my_caps))
        .at("/api-keys", |r| {
            r.post(Authenticated, api_keys::create)
                .get(Authenticated, api_keys::list)
        })
        .at("/api-keys/{id}", |r| {
            r.delete(Authenticated, api_keys::revoke)
        })
}

/// Terms, menus and comments.
fn taxonomy_routes() -> Guarded {
    use Access::{Authenticated, Cap, Public};
    use Capability::{ManageCategories, ManageThemes, ModerateComments};
    Guarded::new()
        .at("/terms", |r| {
            r.get(Authenticated, terms::list)
                .post(Cap(ManageCategories), terms::create)
        })
        .at("/terms/{id}", |r| {
            r.get(Authenticated, terms::get)
                .put(Cap(ManageCategories), terms::update)
                .delete(Cap(ManageCategories), terms::delete_term)
        })
        .at("/terms/merge", |r| {
            r.post(Cap(ManageCategories), terms::merge)
        })
        .at("/menus", |r| {
            r.post(Cap(ManageThemes), menus::create)
                .get(Cap(ManageThemes), menus::list)
        })
        .at("/menus/items/{item_id}", |r| {
            r.delete(Cap(ManageThemes), menus::remove_item)
                .patch(Cap(ManageThemes), menus::update_item)
        })
        .at("/menus/{id}", |r| {
            r.get(Cap(ManageThemes), menus::get)
                .delete(Cap(ManageThemes), menus::delete)
        })
        .at("/menus/{id}/items", |r| {
            r.post(Cap(ManageThemes), menus::add_item)
        })
        // Readers comment and read approved comments without signing in.
        .at("/posts/{id}/comments", |r| {
            r.get(Public, comments::list_approved)
                .post(Public, comments::create)
        })
        .at("/comments", |r| {
            r.get(Cap(ModerateComments), comments::admin_list)
        })
        .at("/comments/{id}/approve", |r| {
            r.post(Cap(ModerateComments), comments::approve)
        })
        .at("/comments/{id}/spam", |r| {
            r.post(Cap(ModerateComments), comments::spam)
        })
        .at("/comments/{id}/trash", |r| {
            r.post(Cap(ModerateComments), comments::trash)
        })
        .at("/comments/{id}/restore", |r| {
            r.post(Cap(ModerateComments), comments::restore)
        })
}

/// The media library and the assistant features that hang off it.
fn media_routes() -> Guarded {
    use Access::{AnyOf, Cap, Public};
    use Capability::{
        EditOthers, EditPosts, ManageOptions, ModerateComments, UploadMedia, ViewAdmin,
    };
    // See guards::MEDIA_UPLOAD_LIMIT: the extractor has its own 2 MiB
    // default that no middleware can raise.
    let upload_limit =
        || axum::extract::DefaultBodyLimit::max(crate::middleware::guards::MEDIA_UPLOAD_LIMIT);
    Guarded::new()
        .at_layered(
            "/media",
            |r| {
                r.get(AnyOf(MEDIA_READERS), media::list)
                    .post(Cap(UploadMedia), media::upload)
            },
            |methods| methods.layer(upload_limit()),
        )
        .at("/media/stats", |r| {
            r.get(AnyOf(MEDIA_READERS), media::stats)
        })
        .at("/media/batch-delete", |r| {
            r.post(Cap(UploadMedia), media::batch_delete)
        })
        .at("/media/{id}", |r| {
            r.get(AnyOf(MEDIA_READERS), media::get)
                .patch(Cap(UploadMedia), media::update)
                .delete(Cap(UploadMedia), media::delete)
        })
        .at("/media/{id}/usage", |r| {
            r.get(AnyOf(MEDIA_READERS), media::usage)
        })
        .at("/media/{id}/edit", |r| {
            r.post(Cap(UploadMedia), media::edit)
        })
        .at("/media/{id}/restore", |r| {
            r.post(Cap(UploadMedia), media::restore)
        })
        .at("/media/trash/empty", |r| {
            r.post(Cap(EditOthers), media::empty_trash)
        })
        .at_layered(
            "/media/{id}/replace",
            |r| r.post(Cap(UploadMedia), media::replace),
            |methods| methods.layer(upload_limit()),
        )
        // The bytes a public page embeds.
        .at("/media/{id}/raw", |r| r.get(Public, media::raw))
        .at("/media/{id}/alt-text", |r| {
            r.post(Cap(UploadMedia), ai_features::alt_text)
        })
        .at("/media/{id}/transcribe", |r| {
            r.post(Cap(UploadMedia), ai_features::transcribe)
        })
        .at("/media/{id}/transcript", |r| {
            r.get(AnyOf(MEDIA_READERS), ai_features::transcript)
        })
        .at("/posts/{id}/read-aloud", |r| {
            r.post(Cap(EditPosts), ai_features::read_aloud)
        })
        .at("/posts/{id}/audio", |r| {
            r.get(Cap(ViewAdmin), ai_features::audio)
        })
        .at("/posts/{id}/related", |r| {
            r.get(Cap(ViewAdmin), ai_features::related)
        })
        .at("/ai/backfill", |r| {
            r.post(Cap(ManageOptions), ai_features::backfill)
        })
        .at("/posts/{id}/embed", |r| {
            r.post(Cap(EditPosts), ai_features::embed)
        })
        .at("/posts/{id}/autofill", |r| {
            r.post(Cap(EditPosts), ai_features::autofill)
        })
        .at("/comments/{id}/screen", |r| {
            r.post(Cap(ModerateComments), ai_features::screen)
        })
}

/// Settings, the registry, updates, search and the operator's odds and ends.
fn site_routes() -> Guarded {
    use Access::{AnyOf, Authenticated, Cap, Public};
    use Capability::ManageOptions;
    Guarded::new()
        // Anyone signed in reads the public subset; the handler widens it
        // for `manage_options`.
        .at("/options", |r| {
            r.get(Authenticated, options::get_all)
                .put(Cap(ManageOptions), options::put_many)
        })
        .at("/options/{key}", |r| {
            r.put(Cap(ManageOptions), options::put)
        })
        .at("/registry", |r| {
            r.get(AnyOf(REGISTRY_BROWSERS), registry::browse)
        })
        // An install needs the capability for the listing's kind, which is
        // in the body and checked by the handler; a caller with neither
        // cannot install anything.
        .at("/registry/install", |r| {
            r.post(AnyOf(REGISTRY_BROWSERS), registry::install)
        })
        .at("/updates", |r| r.get(Cap(ManageOptions), updates::get))
        .at("/updates/apply", |r| {
            r.post(Cap(ManageOptions), updates::apply)
        })
        .at("/updates/status", |r| {
            r.get(Cap(ManageOptions), updates::run_status)
        })
        .at("/version", |r| r.get(Public, version))
        .at("/audit-log", |r| r.get(Cap(ManageOptions), audit_log))
        .at("/search", |r| r.get(Public, search::search))
        .at("/search/reindex", |r| {
            r.post(Cap(ManageOptions), search::reindex)
        })
        .at("/viewer", |r| r.get(Public, viewer))
}

/// Content types and their fields (phase 99). Managing them is a site
/// setting; reading the list and a type's fields is part of editing
/// entries — the editor's type picker and Fields panel — so those need
/// `edit_posts`, not `manage_themes` (the list once lived with the theme
/// routes).
fn content_type_routes() -> Guarded {
    Guarded::new()
        .at("/content-types", |r| {
            r.get(Access::Cap(Capability::EditPosts), content_types::list)
                .post(
                    Access::Cap(Capability::ManageOptions),
                    content_types::create,
                )
        })
        .at("/content-types/{slug}", |r| {
            r.put(
                Access::Cap(Capability::ManageOptions),
                content_types::update,
            )
            .delete(
                Access::Cap(Capability::ManageOptions),
                content_types::delete,
            )
        })
        .at("/content-types/{slug}/fields", |r| {
            r.get(
                Access::Cap(Capability::EditPosts),
                content_types::list_fields,
            )
            .post(
                Access::Cap(Capability::ManageOptions),
                content_types::create_field,
            )
        })
        .at("/content-types/{slug}/fields/{key}", |r| {
            r.put(
                Access::Cap(Capability::ManageOptions),
                content_types::update_field,
            )
            .delete(
                Access::Cap(Capability::ManageOptions),
                content_types::delete_field,
            )
        })
        .at("/content-types/{slug}/field-order", |r| {
            r.put(
                Access::Cap(Capability::ManageOptions),
                content_types::reorder_fields,
            )
        })
        .at("/content-types/{slug}/orphans", |r| {
            r.get(
                Access::Cap(Capability::ManageOptions),
                content_types::orphans,
            )
        })
        .at("/content-types/{slug}/orphans/{key}/clean-up", |r| {
            r.post(
                Access::Cap(Capability::ManageOptions),
                content_types::clean_up,
            )
        })
}

fn guarded() -> Guarded {
    Guarded::new()
        .merge(auth_routes())
        .merge(user_routes())
        .merge(post_routes())
        .merge(taxonomy_routes())
        .merge(ai_routes())
        .merge(site_routes())
        .merge(ops_routes())
        .merge(plugin_routes())
        .merge(webhook_routes())
        .merge(theme_routes())
        .merge(media_routes())
        .merge(content_type_routes())
}

/// Builds the `/api/v1` router.
pub fn router() -> Router<AppState> {
    guarded().finish().0
}

/// What each `/api/v1` route requires, as registered.
// Read only by the role matrix and the access table, which are tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn rules() -> Vec<RouteRule> {
    guarded().finish().1
}

/// Build identity, for operators and for the admin footer.
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct VersionResponse {
    /// Crate version of the running binary.
    pub version: String,
    /// Highest applied migration, or `None` if the database is unreachable.
    pub migration_version: Option<i64>,
    /// The admin bundle this server currently serves, e.g.
    /// `index-W9mNgxa4.js`.
    ///
    /// A browser holding a stale `index.html` loads an older bundle while
    /// every other signal — server version, migration number — looks
    /// correct. This is the one value that distinguishes the two, and the
    /// admin reloads itself when it does not match what it loaded.
    pub ui_bundle: Option<String>,
}

/// `GET /api/v1/version` — what is actually running.
///
/// Unauthenticated on purpose: the version is inferable from behaviour
/// anyway, and an operator checking a deploy landed should not need to log
/// in first. The migration number is included because a binary and a
/// database at different versions is the failure that looks like corruption.
#[utoipa::path(
    get, path = "/api/v1/version", tag = "operations",
    responses((status = 200, description = "Build identity", body = VersionResponse))
)]
pub async fn version(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
) -> axum::Json<VersionResponse> {
    let migration_version =
        sqlx::query_scalar::<_, i64>("SELECT max(version) FROM _sqlx_migrations WHERE success")
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
    axum::Json(VersionResponse {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        migration_version,
        ui_bundle: current_ui_bundle().await,
    })
}

/// The `index-*.js` the admin shell currently points at.
///
/// Read from the same file the shell is served from, so it cannot drift
/// from what a fresh load would get.
async fn current_ui_bundle() -> Option<String> {
    let html = tokio::fs::read_to_string("admin/dist/index.html")
        .await
        .ok()?;
    let start = html.find("/assets/index-")? + "/assets/".len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// `GET /api/v1/audit-log` — recent destructive administrative actions.
///
/// Gated behind `manage_options`: the log names who did what, which is not
/// something every authenticated user should be able to read.
#[utoipa::path(
    get, path = "/api/v1/audit-log", tag = "operations",
    security(("session_cookie" = [])),
    params(("limit" = Option<i64>, Query, description = "Rows to return (1..=500)")),
    responses(
        (status = 200, description = "Recent actions, newest first", body = serde_json::Value),
        (status = 403, description = "Forbidden", body = crate::error::ApiErrorBody),
    )
)]
pub async fn audit_log(
    axum::extract::State(state): axum::extract::State<crate::state::AppState>,
    axum::extract::Query(params): axum::extract::Query<AuditQuery>,
) -> crate::error::ApiResult<axum::Json<Vec<serde_json::Value>>> {
    let rows = vyasa_db::repo::AuditRepo::new(state.pool.clone())
        .recent(params.limit.unwrap_or(100))
        .await
        .map_err(crate::error::ApiError)?;
    Ok(axum::Json(
        rows.iter()
            .map(|r| {
                serde_json::json!({
                    // Ids as strings: JavaScript cannot hold a snowflake.
                    "id": r.id.to_string(),
                    "actor_id": r.actor_id.map(|id| id.to_string()),
                    "actor_name": r.actor_name,
                    "action": r.action,
                    "target": r.target,
                    "detail": r.detail,
                    "created_at": r.created_at.to_rfc3339(),
                })
            })
            .collect(),
    ))
}

/// Query parameters for the audit log.
#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct AuditQuery {
    /// Rows to return; clamped to 1..=500.
    pub limit: Option<i64>,
}

/// Everything under `/ai`: assist, generation, theme building, and the
/// model registry.
fn ai_routes() -> Guarded {
    use Access::{Authenticated, Cap};
    use Capability::{EditOthers, EditPosts, ManageOptions, UploadMedia, ViewAdmin};
    Guarded::new()
        .at("/ai/assist/{kind}", |r| {
            r.post(Cap(EditPosts), ai_assist::suggest)
        })
        .at("/posts/link-suggestions", |r| {
            r.post(Authenticated, link_suggest::suggest)
        })
        .at("/analytics/summary", |r| {
            r.get(Cap(EditOthers), analytics::summary)
        })
        .at("/audience/submissions", |r| {
            r.get(Cap(EditOthers), audience::submissions)
        })
        .at("/audience/submissions/{id}", |r| {
            r.delete(Cap(ManageOptions), audience::delete_submission)
        })
        .at("/audience/subscribers", |r| {
            r.get(Cap(EditOthers), audience::subscribers)
        })
        .at("/audience/subscribers/{id}", |r| {
            r.delete(Cap(ManageOptions), audience::delete_subscriber)
        })
        .at("/ai/models", |r| {
            r.get(Cap(ManageOptions), ai_models::overview)
                .post(Cap(ManageOptions), ai_models::register)
        })
        .at("/ai/models/{id}", |r| {
            r.put(Cap(ManageOptions), ai_models::edit)
                .delete(Cap(ManageOptions), ai_models::remove)
        })
        .at("/ai/models/order", |r| {
            r.put(Cap(ManageOptions), ai_models::order)
        })
        .at("/ai/models/{id}/default", |r| {
            r.post(Cap(ManageOptions), ai_models::make_default)
        })
        .at("/ai/models/{id}/test", |r| {
            r.post(Cap(ManageOptions), ai_models::test_model)
        })
        .at("/ai/providers/{provider}", |r| {
            r.put(Cap(ManageOptions), ai_models::save_provider)
                .delete(Cap(ManageOptions), ai_models::delete_provider)
        })
        .at("/ai/images", |r| {
            r.post(Cap(UploadMedia), ai_features::generate_image)
        })
        .at("/ai/available", |r| {
            r.get(Cap(ViewAdmin), ai_features::available)
        })
        .at("/embeds/preview", |r| {
            r.get(Cap(EditPosts), embed_preview::preview)
        })
        .merge(setup_routes())
        .at("/redirects", |r| {
            r.get(Cap(ManageOptions), seo::list_redirects)
                .post(Cap(ManageOptions), seo::add_redirect)
        })
        .at("/redirects/{*from}", |r| {
            r.delete(Cap(ManageOptions), seo::delete_redirect)
        })
        .at("/posts/{id}/seo-signals", |r| {
            r.get(Cap(EditPosts), seo::signals)
        })
        .at("/posts/{id}/lock", |r| {
            r.post(Cap(EditPosts), locks::lock)
                .delete(Cap(EditPosts), locks::unlock)
        })
        .at("/posts/{id}/check-links", |r| {
            r.post(Cap(EditPosts), seo::check_links)
        })
        .at("/posts/{id}/link-check", |r| {
            r.get(Cap(EditPosts), seo::last_check)
        })
        .at("/ai/providers/{provider}/catalog", |r| {
            r.get(Cap(ManageOptions), ai_models::catalog)
        })
        .at("/ai/providers/{provider}/test", |r| {
            r.post(Cap(ManageOptions), ai_models::test_provider)
        })
        .at("/ai/generate", |r| r.post(Cap(ManageOptions), ai::generate))
        .at("/ai/usage", |r| r.get(Cap(ManageOptions), ai::usage))
}

/// Installed themes and the studio: drafts, edits, history, previews,
/// publishing.
fn theme_routes() -> Guarded {
    let access = Access::Cap(Capability::ManageThemes);
    Guarded::new()
        .at("/themes", |r| {
            r.post(access, themes::install).get(access, themes::list)
        })
        .at("/themes/{id}", |r| r.delete(access, themes::delete))
        .at("/themes/{id}/activate", |r| {
            r.post(access, themes::activate)
        })
        .at("/themes/{id}/tokens", |r| r.get(access, themes::tokens))
        .at("/themes/{id}/files", |r| r.get(access, themes::files))
        .at_layered(
            "/themes/{id}/files/{*path}",
            |r| {
                r.put(access, themes::put_file)
                    .delete(access, themes::delete_file)
            },
            // One bundled file may be as large as a package entry; the
            // default body cap is far below that.
            |methods| {
                methods.layer(axum::extract::DefaultBodyLimit::max(
                    vyasa_themes::package::MAX_ENTRY_BYTES + 1,
                ))
            },
        )
        .at("/themes/{name}/rollback", |r| {
            r.post(access, themes::rollback)
        })
        .at("/themes/vocabulary", |r| {
            r.get(access, theme_studio::vocabulary)
        })
        .at("/themes/preview", |r| {
            r.post(access, theme_studio::candidate)
        })
        .at("/themes/drafts/{id}/messages/{mid}/accept", |r| {
            r.post(access, theme_studio::accept_proposal)
        })
        .at("/themes/drafts", |r| {
            r.get(access, theme_studio::list)
                .post(access, theme_studio::create)
        })
        .at("/themes/drafts/{id}", |r| {
            r.get(access, theme_studio::get)
                .patch(access, theme_studio::rename)
                .delete(access, theme_studio::delete)
        })
        .at("/themes/drafts/{id}/ops", |r| {
            r.post(access, theme_studio::apply)
        })
        .at("/themes/drafts/{id}/revisions", |r| {
            r.get(access, theme_studio::revisions)
        })
        .at("/themes/drafts/{id}/revisions/{seq}", |r| {
            r.get(access, theme_studio::revision)
        })
        .at("/themes/drafts/{id}/revert", |r| {
            r.post(access, theme_studio::revert)
        })
        .at("/themes/drafts/{id}/publish", |r| {
            r.post(access, theme_studio::publish)
        })
        .at("/themes/drafts/{id}/preview", |r| {
            r.get(access, theme_studio::preview)
        })
        .at("/themes/drafts/{id}/messages", |r| {
            r.get(access, theme_studio::messages)
        })
        .at("/themes/drafts/{id}/chat", |r| {
            r.post(access, theme_studio::chat)
        })
}

/// `GET /api/v1/viewer` — who is reading, for the `viewer-greeting` block.
///
/// Deliberately outside the render cache and marked `no-store`: this is
/// the one per-reader thing a public page carries, and the whole reason it
/// is a separate request is that the page itself is shared. Anonymous is a
/// 200 with `signedIn: false`, not a 401 — a greeting that is absent is
/// not an error, and a 401 in a browser console looks like one.
async fn viewer(
    axum::extract::State(_state): axum::extract::State<crate::state::AppState>,
    crate::middleware::MaybePrincipal(principal): crate::middleware::MaybePrincipal,
) -> impl axum::response::IntoResponse {
    let body = principal.as_ref().map_or_else(
        || serde_json::json!({ "signedIn": false }),
        |p| {
            let viewer = p.viewer();
            // Name and role only — the same fields a plugin gets, and for
            // the same reason: enough to greet someone, nothing to
            // impersonate them with. `role` is a custom role's slug when
            // the reader holds one; `roleName` is what to call it.
            serde_json::json!({
                "signedIn": true,
                "displayName": viewer.display_name,
                "role": viewer.role,
                "roleName": roles::role_name(p.user()),
            })
        },
    );
    (
        [
            (axum::http::header::CACHE_CONTROL, "no-store, private"),
            (axum::http::header::CONTENT_TYPE, "application/json"),
        ],
        body.to_string(),
    )
}
