//! Resource-scoped authorization: load a resource and decide whether the
//! caller may act on it, in one step. Route-level access (`authz`) says who
//! may call an endpoint at all; these say which resources that caller may
//! touch. Handlers call these instead of fetching and checking by hand.
//!
//! Every decision goes through [`Principal::ensure`], so an API key is held
//! to its own grants: a key whose owner is an Editor but which was not
//! granted `edit_others` is refused wherever `EditOthers` is needed.

use vyasa_common::AppError;
use vyasa_core::user::{cap_name, Capability};
use vyasa_db::content_models::{MediaRow, PostRow, PostStatus, PostType};
use vyasa_db::models::UserRow;

use crate::error::ApiResult;
use crate::middleware::auth::CurrentUser;
use crate::middleware::Principal;
use crate::rest::roles::RoleChoice;
use crate::state::AppState;

#[cfg(test)]
mod endpoint_tests;
#[cfg(test)]
mod tests;

// ---------------------------------------------------------- content types

/// The custom types the public may open right now (served publicly by a
/// plugin or an administrator), for deciding about many entries at once:
/// listings, feeds, mail, pings, related posts.
pub struct PublicTypes(std::collections::HashSet<String>);

impl PublicTypes {
    /// Reads the live set.
    pub async fn load(surface: &crate::plugin_surface::PluginSurface, pool: &sqlx::PgPool) -> Self {
        Self(crate::entry_fields::public_custom_slugs(surface, pool).await)
    }

    /// Whether the public may open entries of `post_type`: posts, pages
    /// and these custom types — never reusable blocks.
    #[must_use]
    pub fn opens(&self, post_type: PostType) -> bool {
        crate::entry_fields::listed_publicly(post_type, &self.0)
    }

    /// The custom slugs as a set.
    #[must_use]
    pub fn into_set(self) -> std::collections::HashSet<String> {
        self.0
    }

    /// The custom slugs, for a SQL filter.
    #[must_use]
    pub fn slugs(&self) -> Vec<String> {
        self.0.iter().cloned().collect()
    }
}

/// Whether the public may open entries of `post_type` (one entry: a
/// mail, a ping, a link in a page head).
pub async fn publicly_openable(
    surface: &crate::plugin_surface::PluginSurface,
    pool: &sqlx::PgPool,
    post_type: PostType,
) -> bool {
    crate::entry_fields::publicly_served(surface, pool, post_type).await
}

/// The entry `post_id` as a comment target for `principal`: it must
/// exist, be visible to them and be of a type they may read. Visible
/// means published and open — or password-protected and unlocked by
/// `unlock`, the signed token `POST /posts/{id}/verify-password` or the
/// page's unlock form issued — or theirs to see without one (their own
/// entry, or anyone's with `EditOthers`), drafts included. Anything else
/// is answered exactly as a missing entry (`post_not_found`), before any
/// validation of the comment, so no answer says that a hidden entry
/// exists. REST, GraphQL (which has no unlock) and the HTML form all
/// decide here.
///
/// # Errors
/// `NotFound` (`post`).
pub async fn comment_target(
    state: &AppState,
    principal: Option<&Principal>,
    post_id: i64,
    unlock: Option<&str>,
) -> Result<PostRow, AppError> {
    let missing = || AppError::not_found("post", post_id);
    let post = state.posts.get(post_id).await.map_err(|e| match e {
        AppError::NotFound { .. } => missing(),
        other => other,
    })?;
    let unlocked = || {
        post.password_hash.is_none()
            || unlock.is_some_and(|token| {
                state
                    .posts
                    .verify_token(post.id, token, &state.private_token_secret)
                    .is_ok()
            })
    };
    let visible = (post.status == PostStatus::Published && unlocked())
        || principal.is_some_and(|p| sees_post(p, &post));
    if !visible || !may_read_type(state, principal, post.post_type).await {
        return Err(missing());
    }
    Ok(post)
}

/// Whether `principal` reads entries whatever their type: anyone who
/// edits content. Everyone else reads only types the public may see.
fn reads_every_type(principal: Option<&Principal>) -> bool {
    principal.is_some_and(|p| p.ensure(Capability::EditPosts).is_ok())
}

/// The custom types whose entries `principal` may read, as a listing
/// filter ([`vyasa_db::repo::PostFilter::readable_types`]): `None` (no
/// restriction) for those who edit content; otherwise the custom types
/// served publicly right now — never a type declared or made non-public,
/// and never reusable blocks.
pub async fn readable_custom_types(
    state: &AppState,
    principal: Option<&Principal>,
) -> Option<Vec<String>> {
    if reads_every_type(principal) {
        return None;
    }
    Some(
        PublicTypes::load(&state.plugin_surface, &state.pool)
            .await
            .slugs(),
    )
}

/// Whether `principal` may read entries of `post_type` at all (on top of
/// the entry's own status rules): posts and pages, and custom types
/// served publicly — or anything, for those who edit content.
pub async fn may_read_type(
    state: &AppState,
    principal: Option<&Principal>,
    post_type: PostType,
) -> bool {
    reads_every_type(principal)
        || crate::entry_fields::publicly_served(&state.plugin_surface, &state.pool, post_type).await
}

/// Whether `principal` may edit `post` — for details only an editor of
/// the entry is shown (which references point at something gone).
#[must_use]
pub fn may_edit(principal: Option<&Principal>, post: &PostRow) -> bool {
    principal.is_some_and(|p| may_edit_post(p, post).is_ok())
}

// ---------------------------------------------------------------- entries

/// The editing rule on an entry already loaded: your own with `EditPosts`,
/// anyone's with `EditOthers` as well.
fn may_edit_post(principal: &Principal, post: &PostRow) -> Result<(), AppError> {
    principal.ensure(Capability::EditPosts)?;
    if post.author_id != principal.user().id {
        principal.ensure(Capability::EditOthers)?;
    }
    Ok(())
}

/// The entry `id`, if `principal` may edit it: their own entry with
/// `EditPosts`, anyone's with `EditOthers`.
///
/// Everything that belongs to whoever may edit an entry uses this: its
/// revisions and autosaves, preview tokens, terms, SEO signals and link
/// checks, language, taking the editing lock, and the AI actions on it.
///
/// # Errors
/// `NotFound` when there is no such entry; `Forbidden` otherwise.
pub async fn post_for_edit(state: &AppState, principal: &Principal, id: i64) -> ApiResult<PostRow> {
    let post = state.posts.get(id).await?;
    may_edit_post(principal, &post)?;
    Ok(post)
}

/// The entry `id`, if `principal` may trash it (the editing rule) or, with
/// `force`, delete it for good (the editing rule and `DeletePosts`).
///
/// # Errors
/// `NotFound` when there is no such entry; `Forbidden` otherwise.
pub async fn post_for_delete(
    state: &AppState,
    principal: &Principal,
    id: i64,
    force: bool,
) -> ApiResult<PostRow> {
    let post = post_for_edit(state, principal, id).await?;
    if force {
        principal.ensure(Capability::DeletePosts)?;
    }
    Ok(post)
}

/// Whether `principal` sees `post` without a read token: anything
/// published and not password-protected, their own entries when they hold
/// `EditPosts`, and everything with `EditOthers`.
///
/// This is [`post_for_view`] (with no token) for a row already loaded:
/// use it to filter rows that come back alongside something else, such as
/// the members of a translation group.
#[must_use]
pub fn sees_post(principal: &Principal, post: &PostRow) -> bool {
    (post.status == PostStatus::Published && post.password_hash.is_none())
        || (post.author_id == principal.user().id
            && principal.ensure(Capability::EditPosts).is_ok())
        || principal.ensure(Capability::EditOthers).is_ok()
}

/// The viewing rule on an entry already loaded. A password-protected
/// published or private entry can also be read with a signed `token`
/// (from `POST /posts/{id}/verify-password`).
fn may_view_post(
    state: &AppState,
    principal: &Principal,
    post: &PostRow,
    token: Option<&str>,
) -> Result<(), AppError> {
    if sees_post(principal, post) {
        return Ok(());
    }
    if !matches!(post.status, PostStatus::Private | PostStatus::Published)
        || post.password_hash.is_none()
    {
        return Err(AppError::forbidden(
            "you do not have permission to view this post",
        ));
    }
    let token = token.ok_or_else(|| {
        AppError::forbidden(
            "private post requires a valid token; POST /posts/{id}/verify-password with the password",
        )
    })?;
    state
        .posts
        .verify_token(post.id, token, &state.private_token_secret)
}

/// The entry `id`, if `principal` may read it: published and open, their
/// own (with `EditPosts`), anyone's with `EditOthers`, or
/// password-protected with a valid read `token`.
///
/// The entry a translation link points at must pass this with no token:
/// a link is for entries the caller can see.
///
/// # Errors
/// `NotFound` when there is no such entry; `Forbidden` when it is not
/// visible to the caller; `Auth` for a bad or expired token.
pub async fn post_for_view(
    state: &AppState,
    principal: &Principal,
    id: i64,
    token: Option<&str>,
) -> ApiResult<PostRow> {
    let post = state.posts.get(id).await?;
    may_view_post(state, principal, &post, token)?;
    // An entry of a type the caller may not read does not exist for them.
    if !may_read_type(state, Some(principal), post.post_type).await {
        return Err(AppError::not_found("post", id).into());
    }
    Ok(post)
}

/// [`post_for_view`] for an entry addressed by type and slug.
///
/// # Errors
/// As [`post_for_view`]; a trashed entry is `NotFound` by slug.
pub async fn post_for_view_by_slug(
    state: &AppState,
    principal: &Principal,
    post_type: PostType,
    slug: &str,
    token: Option<&str>,
) -> ApiResult<PostRow> {
    let post = state.posts.get_by_slug(post_type, slug).await?;
    may_view_post(state, principal, &post, token)?;
    if !may_read_type(state, Some(principal), post.post_type).await {
        return Err(AppError::not_found("post", slug).into());
    }
    Ok(post)
}

/// Which entries a listing shows `principal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostListScope {
    /// Every entry in every status (`EditOthers`).
    pub all: bool,
    /// The author whose unpublished entries are shown too: the caller,
    /// when they hold `EditPosts`.
    pub own_author: Option<i64>,
}

/// The listing counterpart of [`post_for_view`].
#[must_use]
pub fn post_list_scope(principal: &Principal) -> PostListScope {
    PostListScope {
        all: principal.ensure(Capability::EditOthers).is_ok(),
        own_author: principal
            .ensure(Capability::EditPosts)
            .ok()
            .map(|()| principal.user().id),
    }
}

// ------------------------------------------------------------------ media

/// Reading the library takes `UploadMedia` or `EditPosts`: whoever adds
/// media to content. A subscriber's login has no business in it.
///
/// # Errors
/// `Forbidden` without either capability.
pub fn media_reader(principal: &Principal) -> Result<(), AppError> {
    principal
        .ensure(Capability::UploadMedia)
        .or_else(|_| principal.ensure(Capability::EditPosts))
}

/// The file `id`, if `principal` may change or remove it: their own upload
/// with `UploadMedia`, anyone's with `EditOthers` as well.
///
/// # Errors
/// `NotFound` when there is no such file; `Forbidden` otherwise.
pub async fn media_for_edit(
    state: &AppState,
    principal: &Principal,
    id: i64,
) -> ApiResult<MediaRow> {
    let row = state.media.get(id).await?;
    principal.ensure(Capability::UploadMedia)?;
    if row.owner_id != principal.user().id {
        principal.ensure(Capability::EditOthers)?;
    }
    Ok(row)
}

/// The file `id`'s metadata, for any library reader ([`media_reader`]),
/// whoever uploaded it: the editor reads the alt text of every image an
/// entry embeds, and the bytes are public at `/raw` already.
///
/// # Errors
/// `Forbidden` for a caller who is not a library reader, checked first;
/// then `NotFound` when there is no such file.
pub async fn media_for_view(
    state: &AppState,
    principal: &Principal,
    id: i64,
) -> ApiResult<MediaRow> {
    media_reader(principal)?;
    Ok(state.media.get(id).await?)
}

/// The file `id`, if `principal` may see where it is used. Usage names the
/// entries (drafts included) that embed the file, so a library reader sees
/// it for their own upload, and for anyone's only with `EditOthers`.
///
/// Unlike [`media_for_edit`] this does not take `UploadMedia`: a caller
/// with `EditPosts` alone sees the usage of a file they own.
///
/// # Errors
/// `Forbidden` for a caller who is not a library reader, checked first;
/// then `NotFound` when there is no such file; `Forbidden` for someone
/// else's file without `EditOthers`.
pub async fn media_for_usage(
    state: &AppState,
    principal: &Principal,
    id: i64,
) -> ApiResult<MediaRow> {
    media_reader(principal)?;
    let row = state.media.get(id).await?;
    if row.owner_id != principal.user().id {
        principal.ensure(Capability::EditOthers)?;
    }
    Ok(row)
}

/// Whose uploads a library listing or total covers: whatever `requested`
/// asks for with `EditOthers` (`None` being everyone's), and the caller's
/// own otherwise, whatever was asked.
#[must_use]
pub fn media_owner_scope(principal: &Principal, requested: Option<i64>) -> Option<i64> {
    if principal.ensure(Capability::EditOthers).is_ok() {
        requested
    } else {
        Some(principal.user().id)
    }
}

// ------------------------------------------------------------------ users

/// An account is its holder's to read; anyone else's takes `ManageUsers`.
/// Decided before the account is looked up, so a caller without the
/// capability cannot probe which ids exist.
///
/// Reading only: what comes back is what the users list already shows a
/// user manager. Acting on an account is [`account_for_management`].
///
/// These routes are session-only, so there are no key grants to apply.
///
/// # Errors
/// `Forbidden` for another account without `ManageUsers`.
pub fn account_as_self_or_manager(current: &CurrentUser, id: i64) -> Result<(), AppError> {
    if current.user.id != id {
        vyasa_core::user::ensure(&current.user, Capability::ManageUsers)?;
    }
    Ok(())
}

/// Acting on someone else's account: editing it, changing its role,
/// suspending or deleting it, resetting its password or second factor,
/// ending its sessions.
///
/// `ManageUsers` is not enough on its own. With custom roles it no longer
/// means "administrator", and a user manager who could reset an
/// administrator's password would own that account. So the account must
/// hold nothing `actor` does not ([`within_reach`]); an administrator
/// holds everything and is bounded by nobody.
///
/// The capability is decided before the account is looked up, so a caller
/// without it cannot probe which ids exist.
///
/// These routes are session-only, so there are no key grants to apply.
///
/// # Errors
/// `Forbidden` without `ManageUsers`, or for an account beyond the
/// caller's reach; `NotFound` when there is no such account.
pub async fn account_for_management(
    state: &AppState,
    current: &CurrentUser,
    id: i64,
) -> ApiResult<UserRow> {
    vyasa_core::user::ensure(&current.user, Capability::ManageUsers)?;
    let target = state.users.get(id).await?;
    within_reach(current, &target)?;
    Ok(target)
}

/// [`account_for_management`], except that one's own account needs no
/// capability: for what an account's holder may do to it themselves
/// (sign it out everywhere).
///
/// # Errors
/// As [`account_for_management`] for someone else's account.
pub async fn account_as_self_or_managed(
    state: &AppState,
    current: &CurrentUser,
    id: i64,
) -> ApiResult<()> {
    if current.user.id != id {
        account_for_management(state, current, id).await?;
    }
    Ok(())
}

/// Personal data is kept by email address. Exporting it shows the
/// account's unpublished titles, and erasing it deletes the account, so
/// the account with that address, when there is one, must be within
/// `principal`'s reach. An address with no account is nobody's to guard.
///
/// # Errors
/// `Forbidden` for an account beyond the caller's reach.
pub async fn personal_data_within_reach(
    state: &AppState,
    principal: &Principal,
    email: &str,
) -> ApiResult<()> {
    let email = email.trim().to_lowercase();
    let users = vyasa_db::repo::UsersRepo::new(state.pool.clone());
    match users.get_by_email(&email).await {
        Ok(target) => Ok(within_reach(principal, &target)?),
        Err(AppError::NotFound { .. }) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// The reach rule on an account already loaded: `target` holds no
/// capability `actor` lacks. Capabilities are the effective ones, so a
/// per-user override counts on either side. One's own account is always
/// within reach.
///
/// # Errors
/// `Forbidden`, naming the capabilities `actor` lacks.
pub fn within_reach(actor: &impl Holder, target: &UserRow) -> Result<(), AppError> {
    let missing = beyond(actor, &vyasa_core::user::effective_caps(target));
    if missing.is_empty() {
        Ok(())
    } else {
        Err(AppError::forbidden(format!(
            "this account holds capabilities you do not have: {}",
            missing.join(", ")
        )))
    }
}

// ---------------------------------------------------- full administrator

/// A full administrator holds every capability. `action` finishes the
/// sentence "only a full administrator can …".
///
/// With custom roles, `manage_options` can be held on its own, and some
/// of what it opens is a way to every other capability: whoever decides
/// where the site's mail goes, or which address its links carry, receives
/// an administrator's password-reset link; whoever picks the update
/// channel, the marketplace or their trusted keys chooses the code the
/// site runs; an archive brings in content and accounts wholesale. Those
/// operations ask for everything, so holding one capability never turns
/// into holding them all. A built-in administrator always passes.
///
/// Decided through [`Principal::ensure`]: an API key is a full
/// administrator only when it was granted every capability too.
///
/// # Errors
/// `Forbidden` when `principal` lacks any capability.
pub fn full_administrator(principal: &Principal, action: &str) -> Result<(), AppError> {
    if Capability::ALL
        .iter()
        .all(|cap| principal.ensure(*cap).is_ok())
    {
        Ok(())
    } else {
        Err(AppError::forbidden(format!(
            "only a full administrator can {action}; that takes every capability, \
             not `manage_options` alone"
        )))
    }
}

/// Writing the options named by `keys`. The ones that redirect mail,
/// links, updates or package trust
/// ([`vyasa_core::options::FULL_ADMINISTRATOR_OPTION_KEYS`]) take a
/// [`full_administrator`]; the rest take what the route already asked
/// for. One such key refuses the whole set, so a bulk write is never
/// half applied.
///
/// # Errors
/// `Forbidden`, naming the options that take a full administrator.
pub fn option_write<'a>(
    principal: &Principal,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), AppError> {
    let guarded: Vec<String> = keys
        .into_iter()
        .filter(|key| vyasa_core::options::FULL_ADMINISTRATOR_OPTION_KEYS.contains(key))
        .map(|key| format!("\"{key}\""))
        .collect();
    if guarded.is_empty() {
        return Ok(());
    }
    full_administrator(
        principal,
        &format!(
            "change the option{} {}",
            if guarded.len() == 1 { "" } else { "s" },
            guarded.join(", ")
        ),
    )
}

// ------------------------------------------------------------------ roles

/// Whoever is asking, as far as capabilities go: a session's user, or a
/// principal (where an API key holds only what it was granted).
pub trait Holder {
    /// Whether the caller holds `cap`.
    fn holds(&self, cap: Capability) -> bool;
}

impl Holder for CurrentUser {
    fn holds(&self, cap: Capability) -> bool {
        vyasa_core::user::can(&self.user, cap)
    }
}

impl Holder for Principal {
    fn holds(&self, cap: Capability) -> bool {
        self.ensure(cap).is_ok()
    }
}

/// The names of the capabilities in `caps` that `actor` does not hold.
fn beyond(actor: &impl Holder, caps: &[Capability]) -> Vec<&'static str> {
    caps.iter()
        .filter(|cap| !actor.holds(**cap))
        .map(|cap| cap_name(*cap))
        .collect()
}

/// Nobody hands out what they do not hold: `actor` may create, or give
/// someone, a role with `caps` only when they hold every one of them.
///
/// # Errors
/// `Forbidden`, naming the capabilities `actor` lacks.
pub fn may_grant(actor: &impl Holder, caps: &[Capability]) -> Result<(), AppError> {
    let missing = beyond(actor, caps);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(AppError::forbidden(format!(
            "this role holds capabilities you do not have: {}",
            missing.join(", ")
        )))
    }
}

/// The role a request names (a built-in role's name or a custom role's
/// slug), if `actor` may give it to someone: [`may_grant`] over what the
/// role holds. Every way of giving a user a role goes through this,
/// built-in roles included: a user manager who is not an administrator
/// cannot make one.
///
/// # Errors
/// `Validation` when there is no such role; `Forbidden`, naming the
/// capabilities `actor` lacks.
pub async fn may_grant_role(
    state: &AppState,
    actor: &impl Holder,
    name: &str,
) -> ApiResult<RoleChoice> {
    let choice = RoleChoice::resolve(state, name).await?;
    let caps = choice.capabilities();
    demo_grant(state, &caps)?;
    may_grant(actor, &caps)?;
    Ok(choice)
}

// ------------------------------------------------------------------- demo

/// The error every demo refusal carries.
fn demo_refusal(what: &str) -> AppError {
    AppError::forbidden(format!(
        "{what} is disabled in the demo. Install Vyasa to try it on your own site."
    ))
}

/// Routes a demo refuses outright, by method and matched path: running
/// code (plugins, theme packages), reaching other servers (updates, the
/// marketplace, AI providers, mail), storing credentials (API keys,
/// two-factor), re-running setup, importing a site, and actions that would
/// lock the next visitor out of the shared account.
pub const DEMO_REFUSED_ROUTES: &[(&str, &str, &str)] = &[
    ("POST", "/api/v1/updates/apply", "Updating the site"),
    (
        "GET",
        "/api/v1/setup/verify-url",
        "Fetching other addresses",
    ),
    ("POST", "/api/v1/webhooks", "Using webhooks"),
    ("PATCH", "/api/v1/webhooks/{id}", "Using webhooks"),
    ("POST", "/api/v1/webhooks/{id}/test", "Using webhooks"),
    (
        "POST",
        "/api/v1/webhooks/{id}/deliveries/{delivery_id}/redeliver",
        "Webhooks",
    ),
    (
        "POST",
        "/api/v1/webhooks/{id}/rotate-secret",
        "Using webhooks",
    ),
    (
        "POST",
        "/api/v1/posts/{id}/check-links",
        "Checking links on other sites",
    ),
    ("GET", "/api/v1/export", "Exporting the whole site"),
    (
        "PUT",
        "/api/v1/ai/providers/{provider}",
        "Storing AI provider keys",
    ),
    (
        "DELETE",
        "/api/v1/ai/providers/{provider}",
        "Removing AI providers",
    ),
    (
        "POST",
        "/api/v1/ai/providers/{provider}/test",
        "Testing AI providers",
    ),
    ("PUT", "/api/v1/mail/settings", "Changing the mail relay"),
    ("POST", "/api/v1/mail/test", "Sending mail"),
    ("POST", "/api/v1/import", "Importing a site"),
    ("POST", "/api/v1/plugins", "Installing plugins"),
    ("POST", "/api/v1/plugins/inspect", "Uploading plugins"),
    (
        "POST",
        "/api/v1/plugins/{id}/rollback",
        "Rolling plugins back",
    ),
    (
        "POST",
        "/api/v1/registry/install",
        "Installing from the marketplace",
    ),
    ("POST", "/api/v1/themes", "Uploading theme packages"),
    ("POST", "/api/v1/api-keys", "Creating API keys"),
    ("POST", "/api/v1/auth/mfa/setup", "Two-factor sign-in"),
    ("POST", "/api/v1/auth/mfa/confirm", "Two-factor sign-in"),
    (
        "POST",
        "/api/v1/users/{id}/sessions/revoke",
        "Signing people out everywhere",
    ),
    (
        "POST",
        "/api/v1/users/{id}/reset-link",
        "Sending password reset links",
    ),
    (
        "DELETE",
        "/api/v1/users/{id}/mfa",
        "Resetting two-factor sign-in",
    ),
    ("POST", "/api/v1/setup/account", "Re-running setup"),
    ("POST", "/api/v1/setup/assistants", "Re-running setup"),
    ("POST", "/api/v1/setup/content", "Re-running setup"),
    ("POST", "/api/v1/setup/delivery", "Re-running setup"),
    ("POST", "/api/v1/setup/finish", "Re-running setup"),
    ("POST", "/api/v1/setup/keypair", "Re-running setup"),
    ("POST", "/api/v1/setup/mail", "Re-running setup"),
    ("POST", "/api/v1/setup/mail/test", "Re-running setup"),
    ("POST", "/api/v1/setup/site", "Re-running setup"),
    ("POST", "/api/v1/setup/updates", "Re-running setup"),
];

/// Whether a demo refuses `method` on the route matched as `matched_path`.
///
/// # Errors
/// `Forbidden` with the demo message when it does.
pub fn demo_route(state: &AppState, method: &str, matched_path: &str) -> Result<(), AppError> {
    if !state.config.demo.enabled {
        return Ok(());
    }
    match DEMO_REFUSED_ROUTES
        .iter()
        .find(|(m, p, _)| *m == method && *p == matched_path)
    {
        Some((_, _, what)) => Err(demo_refusal(what)),
        None => Ok(()),
    }
}

/// A demo refuses writing the options that redirect mail, links, updates
/// or package trust ([`vyasa_core::options::FULL_ADMINISTRATOR_OPTION_KEYS`]).
///
/// # Errors
/// `Forbidden` with the demo message.
pub fn demo_options<'a>(
    state: &AppState,
    keys: impl IntoIterator<Item = &'a str>,
) -> Result<(), AppError> {
    if !state.config.demo.enabled {
        return Ok(());
    }
    match keys
        .into_iter()
        .find(|key| vyasa_core::options::FULL_ADMINISTRATOR_OPTION_KEYS.contains(key))
    {
        Some(key) => Err(demo_refusal(&format!("Changing \"{key}\""))),
        None => Ok(()),
    }
}

/// A demo's shared account stays usable for the next visitor: it cannot
/// be deleted, suspended, demoted or have its sign-in details changed.
///
/// # Errors
/// `Forbidden` with the demo message when `target` is the shared account.
pub fn demo_account(state: &AppState, target: &UserRow, what: &str) -> Result<(), AppError> {
    if state.config.demo.enabled && target.email.eq_ignore_ascii_case(&state.config.demo.email) {
        Err(demo_refusal(&format!("{what} the demo account")))
    } else {
        Ok(())
    }
}

/// A demo never hands out account management: no role that holds
/// `manage_users` (the built-in administrator included) is granted, so the
/// shared account stays the only one that can manage others.
///
/// # Errors
/// `Forbidden` with the demo message.
pub fn demo_grant(state: &AppState, caps: &[Capability]) -> Result<(), AppError> {
    if state.config.demo.enabled && caps.contains(&Capability::ManageUsers) {
        Err(demo_refusal("Granting account management"))
    } else {
        Ok(())
    }
}

/// A demo's shared password stays the one on the sign-in page.
///
/// # Errors
/// `Forbidden` with the demo message when a password change is asked for.
pub fn demo_password_change(state: &AppState, changing: bool) -> Result<(), AppError> {
    if state.config.demo.enabled && changing {
        Err(demo_refusal("Changing the demo password"))
    } else {
        Ok(())
    }
}

/// The largest upload a demo takes: enough for a photo, not enough to
/// make the demo a free file host.
pub const DEMO_UPLOAD_LIMIT: u64 = 2 * 1024 * 1024;

/// The upload routes the demo cap applies to.
pub(crate) const DEMO_UPLOAD_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/v1/media"),
    ("POST", "/api/v1/media/{id}/replace"),
    ("POST", "/api/v1/ai/images"),
];

/// A demo refuses uploads over [`DEMO_UPLOAD_LIMIT`], judged by the
/// declared length (a body that declares none is refused too).
///
/// # Errors
/// `TooLarge` with the demo message.
pub fn demo_upload(
    state: &AppState,
    method: &str,
    matched_path: &str,
    content_length: Option<u64>,
) -> Result<(), AppError> {
    if !state.config.demo.enabled
        || !DEMO_UPLOAD_ROUTES
            .iter()
            .any(|(m, p)| *m == method && *p == matched_path)
    {
        return Ok(());
    }
    match content_length {
        Some(n) if n <= DEMO_UPLOAD_LIMIT => Ok(()),
        _ => Err(AppError::too_large(format!(
            "Uploads over {} MiB are disabled in the demo. Install Vyasa to try it on your own site.",
            DEMO_UPLOAD_LIMIT / (1024 * 1024)
        ))),
    }
}

/// A demo's shared account cannot have its personal data erased (which
/// would also remove the account).
///
/// # Errors
/// `Forbidden` with the demo message when `email` is the shared account's.
pub fn demo_erasure(state: &AppState, email: &str) -> Result<(), AppError> {
    if state.config.demo.enabled && email.trim().eq_ignore_ascii_case(&state.config.demo.email) {
        Err(demo_refusal("Erasing the demo account"))
    } else {
        Ok(())
    }
}

/// A demo never creates or widens a custom role into account management:
/// such a role, given to a new account, would be a private manager login.
///
/// # Errors
/// `Forbidden` with the demo message when `capabilities` names
/// `manage_users`.
pub fn demo_role_capabilities(state: &AppState, capabilities: &[String]) -> Result<(), AppError> {
    if state.config.demo.enabled
        && capabilities
            .iter()
            .any(|c| c == cap_name(Capability::ManageUsers))
    {
        Err(demo_refusal("Granting account management"))
    } else {
        Ok(())
    }
}

/// Whether background work that reaches other servers on its own (webhook
/// delivery, the link-check sweep, IndexNow pings) should start. Off in a
/// demo, where anyone can make the site point at anything.
#[must_use]
pub fn outbound_jobs_allowed(state: &AppState) -> bool {
    !state.config.demo.enabled
}
