//! Named hook points.
//!
//! The original contract keyed filters and events by WIT enums, which is
//! why there were four of each: adding a case changes the type, and a
//! changed type invalidates every plugin binary in the field. So the
//! surface could not grow without a migration nobody would run.
//!
//! These are keyed by string. A new hook point is a new constant here and
//! a call at the place it describes — nothing recompiles, nothing breaks,
//! and a plugin that does not handle a point returns its payload
//! unchanged. That is how WordPress ended up with thousands of them.

use vyasa_plugins::host::{HostEnv, V2Outcome, Viewer};

use crate::state::AppState;

/// Filter points, in the order a page passes through them.
///
/// A payload is either the value itself (a title, an excerpt) or a JSON
/// object; `docs/plugin-api.md` records which, per point.
pub mod filters {
    /// The rendered `<title>` text of any public page.
    pub const SEO_TITLE: &str = "seo-title";
    /// The `<meta name="description">` text.
    pub const SEO_DESCRIPTION: &str = "seo-description";
    /// A post's title, wherever it is shown to a visitor.
    pub const POST_TITLE: &str = "post-title";
    /// A post's excerpt, in listings and in `<head>`.
    pub const EXCERPT: &str = "excerpt";
    /// A visitor's search terms, before the index is queried.
    pub const SEARCH_QUERY: &str = "search-query";
    /// Asked once at surface build: the designer sections this plugin
    /// declares, as a JSON array. A plugin with none returns the payload
    /// unchanged — string-keyed points are how new surfaces arrive
    /// without invalidating installed binaries (see host.wit).
    pub const REGISTER_SECTIONS: &str = "register-sections";
    /// Renders one instance of a declared section. The payload is
    /// `{kind, settings, bound, editor}`; the host resolves the binding
    /// and sanitizes the reply. Unchanged payload = cannot render.
    pub const RENDER_SECTION: &str = "render-section";
}

/// Event names, fired after the fact.
pub mod events {
    /// A person signed in. `{"user_id", "role", "custom_role"}`
    /// ([`super::account_payload`]).
    pub const LOGIN_SUCCEEDED: &str = "login-succeeded";
    /// A sign-in attempt was refused. `{}` — never the address tried,
    /// which would hand every listening plugin a list of accounts, and
    /// never why (an unconfirmed account is refused like a wrong password).
    pub const LOGIN_FAILED: &str = "login-failed";
    /// An account was created. `{"user_id", "role", "custom_role"}`
    /// ([`super::account_payload`]).
    pub const USER_CREATED: &str = "user-created";
    /// A visitor registered an account, not yet confirmed (phase 98).
    /// `{"user_id", "role", "custom_role"}` ([`super::account_payload`]).
    /// Not followed by `user-created`: the account may never be
    /// confirmed, and one left unconfirmed is deleted after a week.
    pub const USER_REGISTERED: &str = "user-registered";
    /// An account's role changed. `{"user_id", "role", "custom_role"}`:
    /// the role it has now ([`super::role_payload`]).
    pub const USER_ROLE_CHANGED: &str = "user-role-changed";
    /// An account was deleted. `{"user_id"}`.
    pub const USER_DELETED: &str = "user-deleted";
    /// A file was added to the library. `{"media_id", "mime", "bytes"}`.
    pub const MEDIA_UPLOADED: &str = "media-uploaded";
    /// A post was saved in any status. `{"post_id", "status", "post_type"}`.
    pub const POST_SAVED: &str = "post-saved";
    /// A comment was approved by a moderator. `{"comment_id", "post_id"}`.
    pub const COMMENT_APPROVED: &str = "comment-approved";
    /// A term was created. `{"term_id", "taxonomy", "slug"}`.
    pub const TERM_CREATED: &str = "term-created";
    /// A theme went live. `{"theme_id", "name"}`.
    pub const THEME_ACTIVATED: &str = "theme-activated";
    /// A site option was written. `{"key"}` — never the value, which may
    /// be a secret.
    pub const OPTION_CHANGED: &str = "option-changed";
}

/// Every filter point a plugin may see, for the admin and the docs.
pub const FILTER_POINTS: &[&str] = &[
    filters::SEO_TITLE,
    filters::SEO_DESCRIPTION,
    filters::POST_TITLE,
    filters::EXCERPT,
    filters::SEARCH_QUERY,
];

/// Every event name a plugin may see.
pub const EVENT_NAMES: &[&str] = &[
    events::LOGIN_SUCCEEDED,
    events::LOGIN_FAILED,
    events::USER_CREATED,
    events::USER_REGISTERED,
    events::USER_ROLE_CHANGED,
    events::USER_DELETED,
    events::MEDIA_UPLOADED,
    events::POST_SAVED,
    events::COMMENT_APPROVED,
    events::TERM_CREATED,
    events::THEME_ACTIVATED,
    events::OPTION_CHANGED,
];

/// What the account events (`login-succeeded`, `user-created`,
/// `user-role-changed`) say about an account's role: `role` is always the
/// built-in role (`subscriber` under a custom role), and `custom_role` the
/// custom role's slug, or null without one. A plugin that only reads
/// `role` sees what it always did; one that cares about custom roles
/// reads `custom_role` first.
#[must_use]
pub fn role_payload(
    user_id: i64,
    role: vyasa_db::models::Role,
    custom_role: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "user_id": user_id,
        "role": role.as_str(),
        "custom_role": custom_role,
    })
}

/// [`role_payload`] for an account as it is stored.
#[must_use]
pub fn account_payload(user: &vyasa_db::models::UserRow) -> serde_json::Value {
    role_payload(user.id, user.role, user.custom_role.as_deref())
}

/// The point a plugin is asked about itself.
///
/// A protocol-level extension rather than a new export: `filter-at` is
/// keyed by string precisely so the vocabulary can grow without changing
/// the contract, and that applies to questions about the plugin as much
/// as to content. A plugin that does not recognise it returns the payload
/// unchanged, which is read as "no declaration" — and the host then calls
/// it for everything, exactly as before.
///
/// The reply is `{"points": ["post-title", …], "batch": true}`.
pub const CAPABILITIES_POINT: &str = "vyasa:capabilities";

/// The point that carries a whole page's worth of values at once.
///
/// Payload `{"point": "post-title", "values": [...]}`, reply
/// `{"values": [...]}` of the same length. A listing of ten posts filters
/// twenty strings; doing that one sandbox call at a time cost about
/// 2.5 ms per card, which is most of the render.
pub const BATCH_POINT: &str = "vyasa:batch";

/// What a plugin says it wants to be called for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilterInterest {
    /// Points it handles, or `None` for "did not say" — call it for all.
    pub points: Option<Vec<String>>,
    /// Whether it understands [`BATCH_POINT`].
    pub batch: bool,
}

impl FilterInterest {
    /// Whether this plugin should be called for `point`.
    #[must_use]
    pub fn wants(&self, point: &str) -> bool {
        self.points
            .as_ref()
            .is_none_or(|list| list.iter().any(|p| p == point))
    }

    /// Parses a `vyasa:capabilities` reply. Anything unrecognised means
    /// "did not say", which is the safe answer: call it for everything.
    #[must_use]
    pub fn parse(reply: &str) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(reply) else {
            return Self::default();
        };
        let points = value
            .get("points")
            .and_then(serde_json::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            });
        Self {
            points,
            batch: value
                .get("batch")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        }
    }
}

/// Longest value a named filter will carry into a plugin.
///
/// These points are titles and excerpts, not page bodies; anything larger
/// is a caller mistake and is passed through untouched rather than paid
/// for on every render.
const MAX_PAYLOAD: usize = 64 * 1024;

fn env(state: &AppState, viewer: Option<Viewer>) -> HostEnv {
    HostEnv {
        broker: std::sync::Arc::clone(&state.broker),
        dest: state.pool.clone(),
        viewer,
    }
}

/// Chains `value` through every plugin that implements the superset world.
///
/// Returns `value` untouched when no plugin does, which is the common case
/// and costs one lock-free read of a cached list.
pub async fn filter(state: &AppState, point: &str, value: String) -> String {
    filter_as(state, point, value, None).await
}

/// The same, telling plugins who is looking.
pub async fn filter_as(
    state: &AppState,
    point: &str,
    mut value: String,
    viewer: Option<Viewer>,
) -> String {
    let plugins = state.plugin_surface.filter_audience(point).await;
    if plugins.is_empty() || value.len() > MAX_PAYLOAD {
        return value;
    }
    let env = env(state, viewer);
    for plugin_id in plugins {
        match state
            .plugin_host
            .call_filter_at(plugin_id, point, &value, &env)
            .await
        {
            V2Outcome::Ok(next) if next.len() <= MAX_PAYLOAD => value = next,
            // An oversized reply is dropped rather than propagated: a
            // filter on a title has no business returning a megabyte.
            V2Outcome::Ok(_) => {
                tracing::warn!(plugin_id, point, "filter reply too large; ignored");
            }
            V2Outcome::Unsupported => {}
            V2Outcome::Failed(reason) => {
                tracing::warn!(plugin_id, point, "filter failed: {reason}");
                state.metrics.inc_plugin_failure();
            }
        }
    }
    value
}

/// Filters a whole page's worth of values at one point.
///
/// A listing filters a title and an excerpt per card. One sandbox call
/// each measured about 2.5 ms per card — most of the render — so a plugin
/// that understands [`BATCH_POINT`] gets them all in a single call, and
/// one that does not is looped over as before.
pub async fn filter_many(state: &AppState, point: &str, values: Vec<String>) -> Vec<String> {
    let plugins = state.plugin_surface.filter_audience(point).await;
    if plugins.is_empty() || values.is_empty() {
        return values;
    }
    let env = env(state, None);
    let mut values = values;
    for plugin_id in plugins {
        let batched = state.plugin_surface.filter_interest(plugin_id).await.batch;
        values = if batched {
            match batch_once(state, plugin_id, point, &values, &env).await {
                Some(next) => next,
                // A reply of the wrong shape or length is not usable, and
                // guessing which value went where would be worse than
                // asking again one at a time.
                None => one_by_one(state, plugin_id, point, values, &env).await,
            }
        } else {
            one_by_one(state, plugin_id, point, values, &env).await
        };
    }
    values
}

/// One batched call, or `None` when the reply is unusable.
async fn batch_once(
    state: &AppState,
    plugin_id: i64,
    point: &str,
    values: &[String],
    env: &HostEnv,
) -> Option<Vec<String>> {
    let payload = serde_json::json!({ "point": point, "values": values }).to_string();
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let reply = match state
        .plugin_host
        .call_filter_at(plugin_id, BATCH_POINT, &payload, env)
        .await
    {
        V2Outcome::Ok(reply) => reply,
        V2Outcome::Unsupported => return None,
        V2Outcome::Failed(reason) => {
            tracing::warn!(plugin_id, point, "batched filter failed: {reason}");
            state.metrics.inc_plugin_failure();
            return None;
        }
    };
    let out: Vec<String> = serde_json::from_str::<serde_json::Value>(&reply)
        .ok()?
        .get("values")?
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect();
    // Same length or nothing: a short reply would silently drop a card's
    // title, and a long one has no home to go to.
    (out.len() == values.len()).then_some(out)
}

async fn one_by_one(
    state: &AppState,
    plugin_id: i64,
    point: &str,
    values: Vec<String>,
    env: &HostEnv,
) -> Vec<String> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        let filtered = match state
            .plugin_host
            .call_filter_at(plugin_id, point, &value, env)
            .await
        {
            V2Outcome::Ok(next) if next.len() <= MAX_PAYLOAD => next,
            V2Outcome::Ok(_) | V2Outcome::Unsupported => value,
            V2Outcome::Failed(reason) => {
                tracing::warn!(plugin_id, point, "filter failed: {reason}");
                state.metrics.inc_plugin_failure();
                value
            }
        };
        out.push(filtered);
    }
    out
}

/// Fires a named event at every superset-world plugin, in the background.
///
/// Fire-and-forget by design: an event is a notification, and a plugin
/// reacting to one must not be able to slow down — or fail — the thing
/// that happened.
pub fn emit(state: &AppState, name: &'static str, payload: serde_json::Value) {
    let state = state.clone();
    tokio::spawn(async move {
        let plugins = state.plugin_surface.v2_plugins().await;
        if plugins.is_empty() {
            return;
        }
        let payload = payload.to_string();
        let env = env(&state, None);
        for plugin_id in plugins {
            match state
                .plugin_host
                .call_on_event(plugin_id, name, &payload, &env)
                .await
            {
                V2Outcome::Ok(()) | V2Outcome::Unsupported => {}
                V2Outcome::Failed(reason) => {
                    tracing::warn!(plugin_id, name, "event handler failed: {reason}");
                    state.metrics.inc_plugin_failure();
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{account_payload, role_payload, EVENT_NAMES, FILTER_POINTS};
    use vyasa_db::models::Role;

    #[test]
    fn every_name_is_a_stable_kebab_case_identifier() {
        // These strings are the contract. A rename is a breaking change to
        // every plugin that handles the point, so they are worth guarding.
        for name in FILTER_POINTS.iter().chain(EVENT_NAMES) {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{name}"
            );
            assert!(!name.starts_with('-') && !name.ends_with('-'), "{name}");
        }
    }

    #[test]
    fn names_are_unique_across_both_kinds() {
        let mut all: Vec<&str> = FILTER_POINTS.iter().chain(EVENT_NAMES).copied().collect();
        all.sort_unstable();
        let before = all.len();
        all.dedup();
        assert_eq!(all.len(), before, "duplicate hook name");
    }

    fn user(role: Role, custom: Option<(&str, &str)>) -> vyasa_db::models::UserRow {
        vyasa_db::models::UserRow {
            id: 42,
            email: "holder@example.com".into(),
            username: "holder".into(),
            display_name: "Holder".into(),
            role,
            custom_role: custom.map(|(slug, _)| slug.to_owned()),
            custom_role_name: custom.map(|(_, name)| name.to_owned()),
            custom_role_caps: custom.map(|_| vec!["edit_posts".to_owned()]),
            password_hash: None,
            bio: String::new(),
            avatar_media_id: None,
            meta: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_login_at: None,
            suspended_at: None,
            email_verified_at: Some(chrono::Utc::now()),
        }
    }

    /// `login-succeeded`, `user-created` and `user-role-changed` say the
    /// same thing about an account: its built-in role in `role`, and its
    /// custom role's slug (or null) in `custom_role`.
    #[test]
    fn account_events_carry_the_built_in_role_and_the_custom_role() {
        let custom = user(Role::Subscriber, Some(("drafter", "Draft writer")));
        let built_in = user(Role::Editor, None);
        assert_eq!(
            account_payload(&custom),
            serde_json::json!({ "user_id": 42, "role": "subscriber", "custom_role": "drafter" })
        );
        assert_eq!(
            account_payload(&built_in),
            serde_json::json!({ "user_id": 42, "role": "editor", "custom_role": null })
        );
        // A role change is announced in the same shape as the account it
        // produces, whichever kind of role was given.
        assert_eq!(
            role_payload(42, Role::Subscriber, Some("drafter")),
            account_payload(&custom)
        );
        assert_eq!(
            role_payload(42, Role::Editor, None),
            account_payload(&built_in)
        );
    }
}
