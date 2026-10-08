//! Everything a plugin contributes to the running site besides its hooks:
//! routes, front-end assets, admin settings forms, custom post types and
//! scheduled work.
//!
//! Collected once at boot from the plugin's declaration exports and held
//! in memory, the same way hook registrations are. Nothing here trusts a
//! declaration: sizes are capped, names are checked, and a plugin cannot
//! claim a slug or a route another plugin already holds.

use std::collections::BTreeMap;

use sha2::{Digest as _, Sha256};
use vyasa_plugins::host::{Declaration, HostEnv, V2Outcome};

use crate::state::AppState;

/// Largest stylesheet a plugin may inject.
const MAX_CSS: usize = 256 * 1024;
/// Largest script a plugin may inject.
const MAX_JS: usize = 512 * 1024;
/// Most routes one plugin may declare.
const MAX_ROUTES: usize = 32;
/// Most settings fields one form may have.
const MAX_FIELDS: usize = 64;
/// Shortest interval a scheduled task may ask for.
pub const MIN_TASK_SECONDS: i32 = 60;

/// One declared route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteDecl {
    /// Owning plugin.
    pub plugin_id: i64,
    /// Owning plugin's name, which is also its URL mount point.
    pub plugin_name: String,
    /// Uppercase HTTP method.
    pub method: String,
    /// Path below the mount point, without a leading slash. `*` matches
    /// the rest of the path.
    pub path: String,
}

/// A served asset: its bytes and the hash in its URL.
#[derive(Clone, Debug)]
pub struct Asset {
    /// File contents.
    pub body: String,
    /// Content hash, which is what makes the URL immutable.
    pub hash: String,
    /// MIME type.
    pub content_type: &'static str,
}

/// One plugin's front-end assets.
#[derive(Clone, Debug, Default)]
pub struct AssetBundle {
    /// Stylesheet, when declared.
    pub css: Option<Asset>,
    /// Script, when declared.
    pub js: Option<Asset>,
}

/// One declared settings form, already validated into a shape the admin
/// can render without re-checking anything.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AdminForm {
    /// Owning plugin.
    pub plugin_id: i64,
    /// Owning plugin's name.
    pub plugin_name: String,
    /// Heading for the form.
    pub title: String,
    /// Optional explanatory paragraph.
    pub description: Option<String>,
    /// The fields, in declaration order.
    pub fields: Vec<AdminField>,
}

/// One settings field.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AdminField {
    /// Settings key this field reads and writes.
    pub key: String,
    /// Label shown to the operator.
    pub label: String,
    /// One of `text`, `textarea`, `number`, `boolean`, `select`.
    pub kind: String,
    /// Optional help text.
    pub help: Option<String>,
    /// Choices, for `select`.
    pub options: Vec<String>,
    /// Default value, as JSON.
    pub default: serde_json::Value,
}

/// A designer section a plugin registered through the
/// `register-sections` filter point.
///
/// Mirrors the themes crate's `Builtin` fields on purpose: description,
/// schema, sample, category and inline keys are what the insert library,
/// the inspector forms and both assistants read, so a plugin section
/// arrives in all of them with no host special cases.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SectionDecl {
    /// `namespace/name` — the slash is what stops a plugin shadowing a
    /// core kind, same rule as blocks.
    pub kind: String,
    /// Short human name ("Product grid").
    pub title: String,
    /// One-line description for the library card and the assistants.
    pub description: String,
    /// Insert-library grouping (`commerce`, `community`, …).
    pub category: String,
    /// JSON Schema for the section's settings.
    pub schema: serde_json::Value,
    /// Starter settings for a fresh instance.
    pub sample: serde_json::Value,
    /// Whether the host should resolve a `bind` and hand entries in.
    pub binds: bool,
    /// String settings the canvas edits in place.
    pub inline: Vec<String>,
    /// Owning plugin.
    pub plugin_id: i64,
    /// Owning plugin's name, for the studio's "plugin" badge.
    pub plugin_name: String,
}

/// A custom post type a plugin registered.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PostTypeDecl {
    /// URL and storage slug.
    pub slug: String,
    /// Singular label.
    pub singular: String,
    /// Plural label.
    pub plural: String,
    /// Whether entries get public URLs.
    pub public: bool,
    /// Whether `/{slug}/` lists them.
    pub has_archive: bool,
    /// Owning plugin.
    pub plugin_id: i64,
}

/// A custom taxonomy a plugin registered.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TaxonomyDecl {
    /// URL and storage slug.
    pub slug: String,
    /// Singular label.
    pub singular: String,
    /// Plural label.
    pub plural: String,
    /// Whether terms nest, like categories.
    pub hierarchical: bool,
    /// Whether `/{slug}/{term}` lists the posts in a term.
    pub public: bool,
    /// Owning plugin.
    pub plugin_id: i64,
}

/// One declared periodic task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskDecl {
    /// Owning plugin.
    pub plugin_id: i64,
    /// Task name, passed back to `run-task`.
    pub name: String,
    /// Requested interval, clamped to at least [`MIN_TASK_SECONDS`].
    pub every_seconds: i32,
}

/// The whole contributed surface.
#[derive(Default)]
struct Inner {
    routes: Vec<RouteDecl>,
    assets: BTreeMap<i64, AssetBundle>,
    admin: Vec<AdminForm>,
    post_types: BTreeMap<String, PostTypeDecl>,
    taxonomies: BTreeMap<String, TaxonomyDecl>,
    sections: BTreeMap<String, SectionDecl>,
    tasks: Vec<TaskDecl>,
    /// Plugins implementing the superset world, in id order.
    ///
    /// Cached because the named-hook dispatcher asks on every filtered
    /// title and every event, and the answer only changes at boot.
    v2_plugins: Vec<i64>,
    /// What each of those asked to be called for.
    interests: BTreeMap<i64, crate::plugin_hooks::FilterInterest>,
    /// Precomputed `<link>`/`<script>` tags, rebuilt whenever assets change.
    head_html: String,
}

/// Shared, read-mostly registry of contributed surface.
#[derive(Default)]
pub struct PluginSurface {
    inner: tokio::sync::RwLock<Inner>,
}

impl PluginSurface {
    /// Empty surface.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Routes, for matching an incoming request.
    pub async fn routes(&self) -> Vec<RouteDecl> {
        self.inner.read().await.routes.clone()
    }

    /// The tags to inject into every public page's `<head>`.
    pub async fn head_html(&self) -> String {
        self.inner.read().await.head_html.clone()
    }

    /// One served asset, by the content hash in its URL.
    pub async fn asset(&self, hash: &str) -> Option<Asset> {
        let inner = self.inner.read().await;
        for bundle in inner.assets.values() {
            for asset in [bundle.css.as_ref(), bundle.js.as_ref()]
                .into_iter()
                .flatten()
            {
                if asset.hash == hash {
                    return Some(asset.clone());
                }
            }
        }
        None
    }

    /// Every declared settings form.
    pub async fn admin_forms(&self) -> Vec<AdminForm> {
        self.inner.read().await.admin.clone()
    }

    /// Every registered custom post type.
    pub async fn post_types(&self) -> Vec<PostTypeDecl> {
        let inner = self.inner.read().await;
        inner.post_types.values().cloned().collect()
    }

    /// One registered custom post type.
    pub async fn post_type(&self, slug: &str) -> Option<PostTypeDecl> {
        self.inner.read().await.post_types.get(slug).cloned()
    }

    /// Every registered custom taxonomy.
    pub async fn taxonomies(&self) -> Vec<TaxonomyDecl> {
        let inner = self.inner.read().await;
        inner.taxonomies.values().cloned().collect()
    }

    /// One registered custom taxonomy.
    pub async fn taxonomy(&self, slug: &str) -> Option<TaxonomyDecl> {
        self.inner.read().await.taxonomies.get(slug).cloned()
    }

    /// Every designer section enabled plugins declare.
    pub async fn sections(&self) -> Vec<SectionDecl> {
        let inner = self.inner.read().await;
        inner.sections.values().cloned().collect()
    }

    /// Plugins implementing the superset world, in id order.
    pub async fn v2_plugins(&self) -> Vec<i64> {
        self.inner.read().await.v2_plugins.clone()
    }

    /// Records that a plugin implements the superset world, and what it
    /// asked to be called for.
    pub async fn note_v2(&self, plugin_id: i64, interest: crate::plugin_hooks::FilterInterest) {
        let mut inner = self.inner.write().await;
        if !inner.v2_plugins.contains(&plugin_id) {
            inner.v2_plugins.push(plugin_id);
            inner.v2_plugins.sort_unstable();
        }
        inner.interests.insert(plugin_id, interest);
    }

    /// The plugins that want to hear about `point`, in id order.
    ///
    /// A plugin that declared its points is skipped for the rest, which is
    /// what makes a filter free on a site whose plugins do not use it.
    pub async fn filter_audience(&self, point: &str) -> Vec<i64> {
        let inner = self.inner.read().await;
        inner
            .v2_plugins
            .iter()
            .copied()
            .filter(|id| {
                inner
                    .interests
                    .get(id)
                    .is_none_or(|interest| interest.wants(point))
            })
            .collect()
    }

    /// What one plugin declared.
    pub async fn filter_interest(&self, plugin_id: i64) -> crate::plugin_hooks::FilterInterest {
        self.inner
            .read()
            .await
            .interests
            .get(&plugin_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Every declared task.
    pub async fn tasks(&self) -> Vec<TaskDecl> {
        self.inner.read().await.tasks.clone()
    }

    /// Drops everything a plugin contributed except its post types' place
    /// in the registry, for a refresh that collects them again: between
    /// the two, no administrator can create a content type with one of
    /// those slugs. Returns the slugs kept; pass them to
    /// [`Self::release_stale_types`] once the plugin has been collected
    /// again (or not).
    pub async fn forget_for_refresh(&self, plugin_id: i64) -> Vec<String> {
        let kept: Vec<String> = {
            let inner = self.inner.read().await;
            inner
                .post_types
                .values()
                .filter(|t| t.plugin_id == plugin_id)
                .map(|t| t.slug.clone())
                .collect()
        };
        self.forget_keeping_types(plugin_id).await;
        kept
    }

    /// Takes out of the registry the slugs among `kept` that no plugin
    /// declares after this plugin's refresh (all of this plugin's, when it
    /// was not collected again: disabled, uninstalled, refused). A slug
    /// another plugin claimed in the meantime stays registered for it; one
    /// an administrator holds is never touched.
    pub async fn release_stale_types(&self, kept: &[String]) {
        let inner = self.inner.read().await;
        let stale: Vec<&str> = kept
            .iter()
            .filter(|slug| !inner.post_types.contains_key(slug.as_str()))
            .map(String::as_str)
            .collect();
        vyasa_db::content_models::PostType::unregister_plugin_types(&stale);
    }

    /// Drops everything a plugin contributed.
    pub async fn forget(&self, plugin_id: i64) {
        let inner = self.inner.read().await;
        // Its post types leave the registry too, so a disabled plugin's
        // slug no longer parses and an administrator may take it.
        let types: Vec<&str> = inner
            .post_types
            .values()
            .filter(|t| t.plugin_id == plugin_id)
            .map(|t| t.slug.as_str())
            .collect();
        vyasa_db::content_models::PostType::unregister_plugin_types(&types);
        drop(inner);
        self.forget_keeping_types(plugin_id).await;
    }

    /// [`Self::forget`] without touching the post type registry.
    async fn forget_keeping_types(&self, plugin_id: i64) {
        let mut inner = self.inner.write().await;
        inner.routes.retain(|r| r.plugin_id != plugin_id);
        inner.assets.remove(&plugin_id);
        inner.admin.retain(|f| f.plugin_id != plugin_id);
        inner.post_types.retain(|_, t| t.plugin_id != plugin_id);
        inner.taxonomies.retain(|_, t| t.plugin_id != plugin_id);
        inner.tasks.retain(|t| t.plugin_id != plugin_id);
        inner.v2_plugins.retain(|id| *id != plugin_id);
        inner.interests.remove(&plugin_id);
        inner.head_html = head_tags(&inner.assets);
    }
}

/// Builds the `<head>` fragment for the current asset set.
///
/// External files rather than inline `<style>`/`<script>`: it keeps the
/// rendered page cacheable, keeps plugin code out of every cached HTML
/// body, and leaves a content-security policy something to name.
fn head_tags(assets: &BTreeMap<i64, AssetBundle>) -> String {
    let mut out = String::new();
    for bundle in assets.values() {
        if let Some(css) = &bundle.css {
            use std::fmt::Write as _;
            let _ = write!(
                out,
                "<link rel=\"stylesheet\" href=\"/plugin-assets/{}.css\">",
                css.hash
            );
        }
    }
    for bundle in assets.values() {
        if let Some(js) = &bundle.js {
            use std::fmt::Write as _;
            let _ = write!(
                out,
                "<script src=\"/plugin-assets/{}.js\" defer></script>",
                js.hash
            );
        }
    }
    out
}

fn asset_of(body: &str, content_type: &'static str) -> Asset {
    let hash = hex::encode(&Sha256::digest(body.as_bytes())[..8]);
    Asset {
        body: body.to_owned(),
        hash,
        content_type,
    }
}

/// Reads one declaration export, treating "not implemented" as "declares
/// nothing" and a failure as a logged warning rather than a boot error.
async fn declaration(
    state: &AppState,
    plugin_id: i64,
    name: &str,
    which: Declaration,
) -> Option<serde_json::Value> {
    let env = HostEnv::background(std::sync::Arc::clone(&state.broker), state.pool.clone());
    match state
        .plugin_host
        .call_declaration(plugin_id, which, &env)
        .await
    {
        V2Outcome::Ok(json) => match serde_json::from_str(&json) {
            Ok(value) => Some(value),
            Err(e) => {
                tracing::warn!(plugin = name, "{}: not JSON: {e}", which.as_str());
                None
            }
        },
        V2Outcome::Unsupported => None,
        V2Outcome::Failed(reason) => {
            tracing::warn!(plugin = name, "{}: {reason}", which.as_str());
            None
        }
    }
}

/// How a plugin's declarations meet an administrator's content types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collect {
    /// Server start: a post type or taxonomy whose slug is an
    /// administrator's content type is skipped and reported; the rest of
    /// the plugin loads.
    Boot,
    /// Enabling (or upgrading) at runtime: such a declaration refuses the
    /// whole plugin, and nothing it declares is kept.
    Enable,
}

/// Collects everything one plugin contributes.
///
/// Returns what was skipped at boot because an administrator's content
/// type holds the slug (one sentence each, for the plugin's health).
///
/// # Errors
/// With [`Collect::Enable`], a Conflict naming the slug an administrator's
/// content type holds; the caller withdraws everything collected.
pub async fn collect(
    state: &AppState,
    plugin_id: i64,
    name: &str,
    routes_json: &str,
    mode: Collect,
) -> Result<Vec<String>, vyasa_common::AppError> {
    let mut skipped = Vec::new();
    let mut inner = state.plugin_surface.inner.write().await;
    collect_routes(&mut inner, plugin_id, name, routes_json);
    drop(inner);

    // Named hooks are dispatched to superset-world plugins only, so the
    // dispatcher needs the list. Asking the host once here beats asking it
    // on every filtered title.
    let env = HostEnv::background(std::sync::Arc::clone(&state.broker), state.pool.clone());
    if state
        .plugin_host
        .supports_v2(plugin_id, &env)
        .await
        .unwrap_or(false)
    {
        // Ask once what it wants to be called for. A plugin that does not
        // recognise the question returns the payload unchanged, which
        // reads as "did not say" and means it is called for everything.
        let interest = match state
            .plugin_host
            .call_filter_at(plugin_id, crate::plugin_hooks::CAPABILITIES_POINT, "", &env)
            .await
        {
            V2Outcome::Ok(reply) => crate::plugin_hooks::FilterInterest::parse(&reply),
            _ => crate::plugin_hooks::FilterInterest::default(),
        };
        if let Some(points) = &interest.points {
            tracing::info!(plugin = name, "filters: {}", points.join(", "));
        }
        state.plugin_surface.note_v2(plugin_id, interest).await;
    }

    if let Some(v) = declaration(state, plugin_id, name, Declaration::Assets).await {
        // A script runs in every visitor's browser on every public page,
        // so declaring one is not enough: the operator has to have granted
        // `assets:script`. Stylesheets stay ungated.
        let may_script = state
            .broker
            .holds(
                plugin_id,
                &vyasa_plugins::capabilities::Capability::AssetsScript,
            )
            .await;
        let mut inner = state.plugin_surface.inner.write().await;
        collect_assets(&mut inner, plugin_id, name, &v, may_script);
        inner.head_html = head_tags(&inner.assets);
    }
    if let Some(v) = declaration(state, plugin_id, name, Declaration::Admin).await {
        let mut inner = state.plugin_surface.inner.write().await;
        collect_admin(&mut inner, plugin_id, name, &v);
    }
    if let Some(v) = declaration(state, plugin_id, name, Declaration::PostTypes).await {
        let mut inner = state.plugin_surface.inner.write().await;
        skipped.extend(collect_post_types(&mut inner, plugin_id, name, &v, mode)?);
    }
    if let Some(v) = declaration(state, plugin_id, name, Declaration::Taxonomies).await {
        let mut inner = state.plugin_surface.inner.write().await;
        skipped.extend(collect_taxonomies(&mut inner, plugin_id, name, &v, mode)?);
    }
    // Sections arrive through the string-keyed surface rather than a new
    // export: a point a plugin does not handle returns the payload
    // unchanged, so every installed binary keeps working and one that
    // declares sections simply answers. host.wit records this as the way
    // everything new arrives.
    {
        let env = HostEnv::background(std::sync::Arc::clone(&state.broker), state.pool.clone());
        if let V2Outcome::Ok(reply) = state
            .plugin_host
            .call_filter_at(
                plugin_id,
                crate::plugin_hooks::filters::REGISTER_SECTIONS,
                "",
                &env,
            )
            .await
        {
            if !reply.trim().is_empty() {
                match serde_json::from_str::<serde_json::Value>(&reply) {
                    Ok(v) => {
                        let mut inner = state.plugin_surface.inner.write().await;
                        collect_sections(&mut inner, plugin_id, name, &v);
                    }
                    Err(e) => {
                        tracing::warn!(plugin = name, "register-sections: not JSON: {e}");
                    }
                }
            }
        }
    }
    if let Some(v) = declaration(state, plugin_id, name, Declaration::Schedule).await {
        let mut inner = state.plugin_surface.inner.write().await;
        collect_tasks(&mut inner, plugin_id, name, &v);
    }
    Ok(skipped)
}

/// The refusal or the skip for a slug an administrator's content type
/// holds.
fn admin_type_clash(
    name: &str,
    what: &str,
    slug: &str,
    mode: Collect,
) -> Result<String, vyasa_common::AppError> {
    let message = format!(
        "the plugin {name:?} declares the {what} {slug:?}, but {slug:?} is a content type \
         created by an administrator; delete or rename that content type, or use a version \
         of the plugin with another slug"
    );
    match mode {
        Collect::Enable => Err(vyasa_common::AppError::conflict(message)),
        Collect::Boot => {
            tracing::error!(plugin = name, "{message}; skipped");
            Ok(format!(
                "{what} {slug:?} not loaded: an administrator's content type holds the slug"
            ))
        }
    }
}

/// The skip for a declaration whose slug is malformed or one the site
/// reserves (a built-in kind or a first path segment of its own). The
/// rest of the plugin loads; the plugin shows degraded with this reason,
/// so the author sees why the type never appeared.
fn unusable_slug(name: &str, what: &str, slug: &str) -> String {
    let slug: String = slug.chars().take(64).collect();
    tracing::error!(
        plugin = name,
        "{what} {slug:?}: reserved or malformed; skipped"
    );
    format!(
        "{what} {slug:?} not loaded: the slug is reserved by the site or malformed \
         (lowercase letters, digits and dashes, starting with a letter, at most 32)"
    )
}

/// `namespace/name`, lowercase, exactly one slash — the grammar that
/// makes shadowing a core kind unspellable.
fn is_valid_section_kind(kind: &str) -> bool {
    let Some((ns, rest)) = kind.split_once('/') else {
        return false;
    };
    let part = |p: &str| {
        (1..=32).contains(&p.len())
            && p.starts_with(|c: char| c.is_ascii_lowercase())
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    part(ns) && part(rest) && !rest.contains('/')
}

fn collect_sections(inner: &mut Inner, plugin_id: i64, name: &str, value: &serde_json::Value) {
    let Some(items) = value.as_array() else {
        tracing::warn!(plugin = name, "register-sections: expected an array");
        return;
    };
    for item in items.iter().take(32) {
        let Some(kind) = item.get("kind").and_then(serde_json::Value::as_str) else {
            continue;
        };
        // First declaration wins, across plugins: installing a second
        // plugin must not capture sections already sitting in published
        // layouts — the same rule blocks follow.
        if !is_valid_section_kind(kind) || inner.sections.contains_key(kind) {
            tracing::warn!(
                plugin = name,
                "ignoring section {kind:?}: malformed or already declared"
            );
            continue;
        }
        let schema = item
            .get("settings-schema")
            .or_else(|| item.get("settings_schema"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"type": "object"}));
        if !schema.is_object() {
            tracing::warn!(
                plugin = name,
                "section {kind:?}: settings-schema not an object"
            );
            continue;
        }
        let title = item
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(kind);
        inner.sections.insert(
            kind.to_owned(),
            SectionDecl {
                kind: kind.to_owned(),
                title: title.to_owned(),
                description: item
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                category: item
                    .get("category")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("content")
                    .to_owned(),
                schema,
                sample: item
                    .get("sample")
                    .cloned()
                    .filter(serde_json::Value::is_object)
                    .unwrap_or_else(|| serde_json::json!({})),
                binds: item
                    .get("binds")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                inline: item
                    .get("inline")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                plugin_id,
                plugin_name: name.to_owned(),
            },
        );
    }
}

fn collect_taxonomies(
    inner: &mut Inner,
    plugin_id: i64,
    name: &str,
    value: &serde_json::Value,
    mode: Collect,
) -> Result<Vec<String>, vyasa_common::AppError> {
    let mut skipped = Vec::new();
    let Some(items) = value.as_array() else {
        return Ok(skipped);
    };
    // An administrator's content type owns `/{slug}/…`; checked for every
    // declaration before any is registered, so a refused enable registers
    // none.
    let mut clashes = Vec::new();
    for item in items.iter().take(16) {
        if let Some(slug) = item.get("slug").and_then(serde_json::Value::as_str) {
            if vyasa_db::content_models::PostType::owner(slug)
                == Some(vyasa_db::content_models::TypeOwner::Admin)
            {
                skipped.push(admin_type_clash(name, "taxonomy", slug, mode)?);
                clashes.push(slug);
            }
        }
    }
    for item in items.iter().take(16) {
        let Some(slug) = item.get("slug").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if clashes.contains(&slug) {
            continue;
        }
        if !is_valid_taxonomy_slug(slug) {
            skipped.push(unusable_slug(name, "taxonomy", slug));
            continue;
        }
        // A taxonomy and a post type both claim `/{slug}/…`, so one may
        // never shadow the other.
        if inner.taxonomies.contains_key(slug) || inner.post_types.contains_key(slug) {
            tracing::warn!(plugin = name, "ignoring taxonomy {slug:?}: taken");
            continue;
        }
        if let Err(e) = vyasa_db::content_models::Taxonomy::register(slug) {
            tracing::warn!(plugin = name, "taxonomy {slug:?}: {e}");
            continue;
        }
        let singular = item
            .get("singular")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(slug);
        inner.taxonomies.insert(
            slug.to_owned(),
            TaxonomyDecl {
                slug: slug.to_owned(),
                singular: singular.to_owned(),
                plural: item
                    .get("plural")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(singular)
                    .to_owned(),
                hierarchical: item
                    .get("hierarchical")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                public: item
                    .get("public")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                plugin_id,
            },
        );
    }
    Ok(skipped)
}

fn collect_routes(inner: &mut Inner, plugin_id: i64, name: &str, json: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    let Some(items) = value.as_array() else {
        return;
    };
    for item in items.iter().take(MAX_ROUTES) {
        let (Some(method), Some(path)) = (
            item.get("method").and_then(serde_json::Value::as_str),
            item.get("path").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let method = method.to_ascii_uppercase();
        if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
            tracing::warn!(plugin = name, "ignoring route with method {method:?}");
            continue;
        }
        // `..` would let a declared path climb out of the plugin's own
        // mount point once it is joined onto one.
        let path = path.trim_start_matches('/').to_owned();
        if path.split('/').any(|seg| seg == "..") {
            tracing::warn!(plugin = name, "ignoring route path {path:?}");
            continue;
        }
        inner.routes.push(RouteDecl {
            plugin_id,
            plugin_name: name.to_owned(),
            method,
            path,
        });
    }
}

fn collect_assets(
    inner: &mut Inner,
    plugin_id: i64,
    name: &str,
    value: &serde_json::Value,
    may_script: bool,
) {
    let mut bundle = AssetBundle::default();
    if let Some(css) = value.get("css").and_then(serde_json::Value::as_str) {
        if css.len() > MAX_CSS {
            tracing::warn!(plugin = name, "css exceeds {MAX_CSS} bytes; dropped");
        } else if !css.trim().is_empty() {
            bundle.css = Some(asset_of(css, "text/css; charset=utf-8"));
        }
    }
    if let Some(js) = value.get("js").and_then(serde_json::Value::as_str) {
        if !may_script {
            if !js.trim().is_empty() {
                tracing::warn!(
                    plugin = name,
                    "declares a script but was not granted assets:script; dropped"
                );
            }
        } else if js.len() > MAX_JS {
            tracing::warn!(plugin = name, "js exceeds {MAX_JS} bytes; dropped");
        } else if !js.trim().is_empty() {
            bundle.js = Some(asset_of(js, "text/javascript; charset=utf-8"));
        }
    }
    if bundle.css.is_some() || bundle.js.is_some() {
        inner.assets.insert(plugin_id, bundle);
    }
}

fn collect_admin(inner: &mut Inner, plugin_id: i64, name: &str, value: &serde_json::Value) {
    let Some(raw_fields) = value.get("fields").and_then(serde_json::Value::as_array) else {
        return;
    };
    let mut fields = Vec::new();
    for field in raw_fields.iter().take(MAX_FIELDS) {
        let Some(key) = field.get("key").and_then(serde_json::Value::as_str) else {
            continue;
        };
        // The key becomes a settings key for this plugin, so it has to be
        // something an operator can read in an audit line.
        if key.is_empty()
            || key.len() > 64
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            tracing::warn!(plugin = name, "ignoring settings field {key:?}");
            continue;
        }
        let kind = field
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("text");
        let kind = match kind {
            "text" | "textarea" | "number" | "boolean" | "select" => kind,
            other => {
                tracing::warn!(
                    plugin = name,
                    "settings field {key:?}: unknown type {other:?}"
                );
                "text"
            }
        };
        fields.push(AdminField {
            key: key.to_owned(),
            label: field
                .get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(key)
                .to_owned(),
            kind: kind.to_owned(),
            help: field
                .get("help")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            options: field
                .get("options")
                .and_then(serde_json::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            default: field
                .get("default")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        });
    }
    if fields.is_empty() {
        return;
    }
    inner.admin.push(AdminForm {
        plugin_id,
        plugin_name: name.to_owned(),
        title: value
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(name)
            .to_owned(),
        description: value
            .get("description")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        fields,
    });
}

/// Slugs no plugin may claim, for a post type or a taxonomy.
///
/// Not decoration: `post`, `page`, `block`, `category` and `tag` are core
/// types and taxonomies, and the rest are public URL prefixes the router
/// already owns, so a plugin taking one would shadow the site's own pages.
///
/// Post types and taxonomies share the `/{slug}/…` namespace, so they
/// share this list too — keeping two of them let `archive` and `preview`
/// be refused for one and accepted for the other.
const RESERVED_SLUGS: &[&str] = &[
    "post",
    "page",
    "block",
    "category",
    "tag",
    "search",
    "feed",
    "comment",
    "api",
    "admin",
    "sitemap",
    "robots",
    "assets",
    "plugin-assets",
    "date",
    "archive",
    "preview",
    "logo",
    "favicon",
    "healthz",
    "readyz",
];

/// Whether a custom post type may use this slug.
#[must_use]
pub fn is_valid_post_type_slug(slug: &str) -> bool {
    vyasa_db::content_models::is_valid_custom_type(slug)
        && !RESERVED_SLUGS.contains(&slug)
        // The site's own addresses, which content types created in the
        // admin may not take either (derived from the router; see the
        // drift test in `reserved_slugs`).
        && !vyasa_core::content::RESERVED_TYPE_SLUGS.contains(&slug)
}

/// Whether a custom taxonomy may use this slug. The same rule: they share
/// the URL namespace.
#[must_use]
pub fn is_valid_taxonomy_slug(slug: &str) -> bool {
    is_valid_post_type_slug(slug)
}

fn collect_post_types(
    inner: &mut Inner,
    plugin_id: i64,
    name: &str,
    value: &serde_json::Value,
    mode: Collect,
) -> Result<Vec<String>, vyasa_common::AppError> {
    use vyasa_db::content_models::{PostType, TypeOwner};
    let mut skipped = Vec::new();
    let Some(items) = value.as_array() else {
        return Ok(skipped);
    };
    let mut decls: Vec<PostTypeDecl> = Vec::new();
    for item in items.iter().take(16) {
        let Some(slug) = item.get("slug").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !is_valid_post_type_slug(slug) {
            skipped.push(unusable_slug(name, "post type", slug));
            continue;
        }
        if inner.post_types.contains_key(slug)
            || inner.taxonomies.contains_key(slug)
            || decls.iter().any(|d| d.slug == slug)
        {
            tracing::warn!(plugin = name, "post type {slug:?} is already taken");
            continue;
        }
        let singular = item
            .get("singular")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(slug);
        decls.push(PostTypeDecl {
            slug: slug.to_owned(),
            singular: singular.to_owned(),
            plural: item
                .get("plural")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(singular)
                .to_owned(),
            public: item
                .get("public")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            has_archive: item
                .get("has-archive")
                .or_else(|| item.get("has_archive"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            plugin_id,
        });
    }
    // An administrator's content type wins its slug (it was loaded
    // first). Enabling refuses the plugin; boot skips the declaration.
    let mut kept = Vec::with_capacity(decls.len());
    for decl in decls {
        if PostType::owner(&decl.slug) == Some(TypeOwner::Admin) {
            skipped.push(admin_type_clash(name, "post type", &decl.slug, mode)?);
        } else {
            kept.push(decl);
        }
    }
    // Interning is what makes the slug a `PostType` the repositories and
    // the router can carry; without it the declaration would be admin
    // decoration over a type nothing could store. All of them or none,
    // so nothing is half-registered.
    let slugs: Vec<&str> = kept.iter().map(|d| d.slug.as_str()).collect();
    let registered: Vec<bool> = match PostType::register_plugin_types(&slugs) {
        Ok(_) => vec![true; kept.len()],
        Err(e) if mode == Collect::Enable => {
            return Err(vyasa_common::AppError::conflict(format!(
                "the plugin {name:?} cannot register its post types: {e}"
            )))
        }
        // At boot one bad name (the cap, a race) costs only itself.
        Err(_) => kept
            .iter()
            .map(|d| match PostType::register(&d.slug) {
                Ok(_) => true,
                Err(e) => {
                    tracing::error!(plugin = name, "post type {:?}: {e}; skipped", d.slug);
                    skipped.push(format!("post type {:?} not loaded: {e}", d.slug));
                    false
                }
            })
            .collect(),
    };
    for (decl, ok) in kept.into_iter().zip(registered) {
        if ok {
            inner.post_types.insert(decl.slug.clone(), decl);
        }
    }
    Ok(skipped)
}

fn collect_tasks(inner: &mut Inner, plugin_id: i64, name: &str, value: &serde_json::Value) {
    let Some(items) = value.as_array() else {
        return;
    };
    for item in items.iter().take(16) {
        let Some(task) = item.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if task.is_empty() || task.len() > 64 {
            tracing::warn!(plugin = name, "ignoring task {task:?}");
            continue;
        }
        let every = item
            .get("every-seconds")
            .or_else(|| item.get("every_seconds"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        // Clamped rather than rejected: a plugin asking to run every second
        // is asking for something reasonable at the wrong granularity, and
        // silently never running is a worse answer than running hourly.
        let every_seconds = i32::try_from(every)
            .unwrap_or(MIN_TASK_SECONDS)
            .clamp(MIN_TASK_SECONDS, 24 * 60 * 60);
        inner.tasks.push(TaskDecl {
            plugin_id,
            name: task.to_owned(),
            every_seconds,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_and_malformed_post_type_slugs_are_refused() {
        assert!(is_valid_post_type_slug("product"));
        assert!(is_valid_post_type_slug("case-study"));
        for bad in [
            "post", "page", "api", "admin", "feed", "x", "-x", "x-", "X", "x_y", "",
        ] {
            assert!(!is_valid_post_type_slug(bad), "{bad}");
        }
    }

    #[test]
    fn routes_cannot_climb_out_of_their_mount_point() {
        let mut inner = Inner::default();
        collect_routes(
            &mut inner,
            1,
            "p",
            r#"[{"method":"get","path":"/ok"},
                {"method":"GET","path":"a/../../etc"},
                {"method":"TRACE","path":"x"}]"#,
        );
        assert_eq!(inner.routes.len(), 1);
        assert_eq!(inner.routes[0].method, "GET");
        assert_eq!(inner.routes[0].path, "ok");
    }

    #[test]
    fn assets_are_content_addressed_and_bounded() {
        let mut inner = Inner::default();
        collect_assets(
            &mut inner,
            1,
            "p",
            &serde_json::json!({"css": ".a{color:red}", "js": "x".repeat(MAX_JS + 1)}),
            true,
        );
        let bundle = &inner.assets[&1];
        assert!(bundle.css.is_some());
        assert!(bundle.js.is_none(), "oversized js must be dropped");
        let head = head_tags(&inner.assets);
        assert!(head.contains("/plugin-assets/"), "{head}");
        assert!(!head.contains("<script"), "{head}");
    }

    #[test]
    fn a_script_needs_the_assets_script_grant() {
        // Declaring a script was enough to put it on every public page;
        // it now takes a grant the operator saw at install.
        let mut inner = Inner::default();
        collect_assets(
            &mut inner,
            1,
            "p",
            &serde_json::json!({"css": ".a{color:red}", "js": "alert(1)"}),
            false,
        );
        let bundle = &inner.assets[&1];
        assert!(bundle.css.is_some(), "stylesheets are not gated");
        assert!(bundle.js.is_none(), "an ungranted script is dropped");
        let head = head_tags(&inner.assets);
        assert!(!head.contains("<script"), "{head}");

        // A script-only plugin without the grant contributes nothing.
        collect_assets(
            &mut inner,
            2,
            "q",
            &serde_json::json!({"js": "alert(1)"}),
            false,
        );
        assert!(!inner.assets.contains_key(&2));
    }

    #[test]
    fn task_intervals_are_clamped_not_dropped() {
        let mut inner = Inner::default();
        collect_tasks(
            &mut inner,
            1,
            "p",
            &serde_json::json!([{"name": "sync", "every-seconds": 1},
                                {"name": "daily", "every-seconds": 999_999}]),
        );
        assert_eq!(inner.tasks[0].every_seconds, MIN_TASK_SECONDS);
        assert_eq!(inner.tasks[1].every_seconds, 24 * 60 * 60);
    }

    #[test]
    fn a_taxonomy_may_not_take_a_slug_a_post_type_holds() {
        let mut inner = Inner::default();
        collect_post_types(
            &mut inner,
            1,
            "p",
            &serde_json::json!([{"slug": "series", "singular": "Series"}]),
            Collect::Boot,
        )
        .expect("collected");
        collect_taxonomies(
            &mut inner,
            1,
            "p",
            &serde_json::json!([
                {"slug": "series", "singular": "Series"},
                {"slug": "mood", "singular": "Mood", "plural": "Moods"}
            ]),
            Collect::Boot,
        )
        .expect("collected");
        // Both claim `/{slug}/…`, so the second declaration loses.
        assert!(!inner.taxonomies.contains_key("series"));
        assert_eq!(inner.taxonomies["mood"].plural, "Moods");
        // And not the other way round either.
        collect_post_types(
            &mut inner,
            2,
            "q",
            &serde_json::json!([{"slug": "mood", "singular": "Mood"}]),
            Collect::Boot,
        )
        .expect("collected");
        assert_eq!(inner.post_types["series"].plugin_id, 1);
        assert!(!inner.post_types.contains_key("mood"));
    }

    /// A declaration under a reserved or malformed slug is not just
    /// logged: it is reported, so the plugin shows degraded with the
    /// reason in its health. The rest of what it declares still loads.
    #[test]
    fn a_reserved_or_malformed_slug_is_reported_not_just_logged() {
        let mut inner = Inner::default();
        let skipped = collect_post_types(
            &mut inner,
            7,
            "p",
            &serde_json::json!([
                {"slug": "feed", "singular": "Feed"},
                {"slug": "Not A Slug", "singular": "Bad"},
                {"slug": "gizmo-reserved-test", "singular": "Gizmo"}
            ]),
            Collect::Enable,
        )
        .expect("collected: a reserved slug degrades, it does not refuse");
        assert_eq!(skipped.len(), 2, "{skipped:?}");
        assert!(skipped[0].contains("\"feed\""), "{skipped:?}");
        assert!(skipped[0].contains("reserved"), "{skipped:?}");
        assert!(skipped[1].contains("Not A Slug"), "{skipped:?}");
        assert!(inner.post_types.contains_key("gizmo-reserved-test"));
        assert!(!inner.post_types.contains_key("feed"));

        let skipped = collect_taxonomies(
            &mut inner,
            7,
            "p",
            &serde_json::json!([
                {"slug": "search", "singular": "Search"},
                {"slug": "gizmo-reserved-tax", "singular": "Kind"}
            ]),
            Collect::Boot,
        )
        .expect("collected");
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].contains("\"search\""), "{skipped:?}");
        assert!(inner.taxonomies.contains_key("gizmo-reserved-tax"));
    }

    #[test]
    fn reserved_slugs_are_the_same_list_for_both_kinds() {
        // Two lists drifted apart once: `archive` and `preview` were
        // refused for a taxonomy and accepted for a post type.
        for reserved in ["archive", "preview", "post", "category", "api", "feed"] {
            assert!(!is_valid_post_type_slug(reserved), "post type {reserved}");
            assert!(!is_valid_taxonomy_slug(reserved), "taxonomy {reserved}");
        }
        assert!(is_valid_post_type_slug("product"));
        assert!(is_valid_taxonomy_slug("product"));
    }

    #[test]
    fn a_refresh_keeps_a_plugins_types_registered_until_it_is_collected_again() {
        use vyasa_db::content_models::{PostType, TypeOwner};
        let mut inner = Inner::default();
        collect_post_types(
            &mut inner,
            7,
            "p",
            &serde_json::json!([{"slug": "refresh-kept"}, {"slug": "refresh-dropped"}]),
            Collect::Boot,
        )
        .expect("collected");
        let surface = PluginSurface {
            inner: tokio::sync::RwLock::new(inner),
        };
        futures::executor::block_on(async {
            let kept = surface.forget_for_refresh(7).await;
            assert_eq!(kept.len(), 2);
            // Between forgetting and collecting again, no administrator
            // can take the slugs.
            for slug in ["refresh-kept", "refresh-dropped"] {
                assert_eq!(PostType::owner(slug), Some(TypeOwner::Plugin), "{slug}");
                assert!(PostType::check_admin(slug).is_err(), "{slug}");
            }
            // The new version declares only one of them.
            {
                let mut inner = surface.inner.write().await;
                collect_post_types(
                    &mut inner,
                    7,
                    "p",
                    &serde_json::json!([{"slug": "refresh-kept"}]),
                    Collect::Boot,
                )
                .expect("collected again");
            }
            surface.release_stale_types(&kept).await;
            assert_eq!(PostType::owner("refresh-kept"), Some(TypeOwner::Plugin));
            assert_eq!(PostType::owner("refresh-dropped"), None);

            // Another plugin claims a slug while the first is refreshed:
            // the first's release must not take it from the second.
            let kept = surface.forget_for_refresh(7).await;
            {
                let mut inner = surface.inner.write().await;
                collect_post_types(
                    &mut inner,
                    8,
                    "q",
                    &serde_json::json!([{"slug": "refresh-kept"}]),
                    Collect::Boot,
                )
                .expect("claimed by plugin 8");
            }
            surface.release_stale_types(&kept).await;
            assert_eq!(PostType::owner("refresh-kept"), Some(TypeOwner::Plugin));
            assert_eq!(
                surface.post_type("refresh-kept").await.map(|d| d.plugin_id),
                Some(8)
            );
        });
    }

    #[test]
    fn forgetting_a_plugin_removes_everything_it_contributed() {
        let mut inner = Inner::default();
        collect_routes(&mut inner, 1, "p", r#"[{"method":"GET","path":"a"}]"#);
        collect_post_types(
            &mut inner,
            1,
            "p",
            &serde_json::json!([{"slug": "thing"}]),
            Collect::Boot,
        )
        .expect("collected");
        collect_taxonomies(
            &mut inner,
            1,
            "p",
            &serde_json::json!([{"slug": "topic"}]),
            Collect::Boot,
        )
        .expect("collected");
        collect_tasks(
            &mut inner,
            1,
            "p",
            &serde_json::json!([{"name": "job", "every-seconds": 600}]),
        );
        collect_assets(
            &mut inner,
            1,
            "p",
            &serde_json::json!({"css": ".a{}"}),
            false,
        );
        collect_admin(
            &mut inner,
            1,
            "p",
            &serde_json::json!({"fields": [{"key": "k"}]}),
        );
        // A second plugin's contributions must survive the first's removal.
        collect_routes(&mut inner, 2, "q", r#"[{"method":"GET","path":"b"}]"#);

        let surface = PluginSurface {
            inner: tokio::sync::RwLock::new(inner),
        };
        futures::executor::block_on(async {
            surface
                .note_v2(1, crate::plugin_hooks::FilterInterest::default())
                .await;
            surface
                .note_v2(2, crate::plugin_hooks::FilterInterest::default())
                .await;
            assert_eq!(surface.v2_plugins().await, vec![1, 2]);

            surface.forget(1).await;
            assert_eq!(surface.routes().await.len(), 1, "plugin 2 keeps its route");
            assert!(surface.post_types().await.is_empty());
            assert!(surface.taxonomies().await.is_empty());
            assert!(surface.tasks().await.is_empty());
            assert!(surface.admin_forms().await.is_empty());
            assert!(surface.head_html().await.is_empty(), "assets go too");
            assert_eq!(surface.v2_plugins().await, vec![2]);
        });
    }

    #[test]
    fn assets_are_served_by_hash_and_stylesheets_precede_scripts() {
        let mut inner = Inner::default();
        collect_assets(
            &mut inner,
            1,
            "p",
            &serde_json::json!({"css": ".a{color:red}", "js": "console.log(1)"}),
            true,
        );
        inner.head_html = head_tags(&inner.assets);
        let head = inner.head_html.clone();
        let css_at = head.find("stylesheet").expect("a stylesheet");
        let js_at = head.find("<script").expect("a script");
        // Styles first so a page does not paint unstyled before the script
        // that might depend on them.
        assert!(css_at < js_at, "{head}");
        assert!(head.contains("defer"), "{head}");

        let surface = PluginSurface {
            inner: tokio::sync::RwLock::new(inner),
        };
        futures::executor::block_on(async {
            let hash = head
                .split("/plugin-assets/")
                .nth(1)
                .and_then(|r| r.split('.').next())
                .expect("a hash");
            let asset = surface.asset(hash).await.expect("served by its hash");
            assert_eq!(asset.body, ".a{color:red}");
            assert!(surface.asset("deadbeef").await.is_none());
        });
    }

    #[test]
    fn identical_asset_bytes_get_identical_urls() {
        // Content addressing: republishing the same CSS must not bust a
        // viewer's year-long cache entry.
        let a = asset_of(".a{}", "text/css");
        let b = asset_of(".a{}", "text/css");
        let c = asset_of(".b{}", "text/css");
        assert_eq!(a.hash, b.hash);
        assert_ne!(a.hash, c.hash);
        assert!(a.hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn settings_fields_are_checked_before_the_admin_sees_them() {
        let mut inner = Inner::default();
        collect_admin(
            &mut inner,
            1,
            "p",
            &serde_json::json!({
                "title": "Feeds",
                "fields": [
                    {"key": "url", "label": "Feed URL", "type": "text"},
                    {"key": "bad key!", "label": "nope"},
                    {"key": "mode", "type": "wat"}
                ]
            }),
        );
        let form = &inner.admin[0];
        assert_eq!(form.fields.len(), 2);
        assert_eq!(form.fields[0].key, "url");
        // An unknown widget type falls back to text rather than vanishing.
        assert_eq!(form.fields[1].kind, "text");
    }
}
