//! Plugin sections: the designer's extension contract (phase 54).
//!
//! A plugin declares sections through the `register-sections` filter point
//! and renders them through `render-section` — string-keyed points, not
//! new exports, because that is how `host.wit` says new surfaces arrive
//! without invalidating installed binaries. This module is the bridge from
//! those declarations into the themes crate's `DynBlockRegistry`: an
//! adapter per declared kind, assembled beside the built-ins whenever a
//! page renders or a layout validates.
//!
//! The host resolves bindings and hands entries in; the plugin never
//! queries. A section renderer therefore needs no database capability, and
//! visibility rules stay enforced in exactly one place (the phase-53 query
//! path).

use std::sync::Arc;

use vyasa_plugins::host::{HostEnv, V2Outcome};
use vyasa_themes::{
    Binding, BlockPayload, DynBlockRegistry, DynamicBlock, MapSettings, ResolveContext,
};

use crate::plugin_surface::SectionDecl;
use crate::state::AppState;

/// Interned copies of plugin-supplied strings.
///
/// [`DynamicBlock`] hands out `&'static str`, which built-ins satisfy with
/// literals. Plugin kinds are runtime strings, so they are interned the
/// way `PostType::register` interns type names: each distinct string leaks
/// once, bounded, which is what buys the lifetime. The cap is generous —
/// kinds and descriptions across every plugin a site will ever enable —
/// and hitting it degrades to a placeholder rather than growing forever.
static INTERNED: std::sync::RwLock<Option<std::collections::BTreeSet<&'static str>>> =
    std::sync::RwLock::new(None);

const MAX_INTERNED: usize = 1024;

fn intern(value: &str) -> Option<&'static str> {
    let mut guard = INTERNED.write().ok()?;
    let set = guard.get_or_insert_with(std::collections::BTreeSet::new);
    if let Some(found) = set.get(value) {
        return Some(found);
    }
    if set.len() >= MAX_INTERNED {
        return None;
    }
    let leaked: &'static str = Box::leak(value.to_owned().into_boxed_str());
    set.insert(leaked);
    Some(leaked)
}

/// Interned inline-key slices, keyed by kind so re-enabling a plugin
/// never leaks a second copy.
static SLICES: std::sync::RwLock<
    Option<std::collections::BTreeMap<String, &'static [&'static str]>>,
> = std::sync::RwLock::new(None);

fn intern_slice(kind: &str, refs: Vec<&'static str>) -> Option<&'static [&'static str]> {
    let mut guard = SLICES.write().ok()?;
    let map = guard.get_or_insert_with(std::collections::BTreeMap::new);
    if let Some(found) = map.get(kind) {
        return Some(found);
    }
    if map.len() >= 256 {
        return None;
    }
    let leaked: &'static [&'static str] = Box::leak(refs.into_boxed_slice());
    map.insert(kind.to_owned(), leaked);
    Some(leaked)
}

/// The insert-library category, mapped to the closed set the admin knows.
/// Anything unrecognised lands in `content`, which keeps it reachable.
fn category_static(raw: &str) -> &'static str {
    match raw {
        "structure" => "structure",
        "marketing" => "marketing",
        "navigation" => "navigation",
        "commerce" => "commerce",
        "community" => "community",
        "media" => "media",
        _ => "content",
    }
}

/// One plugin section as the registry sees it.
pub struct PluginSection {
    kind: &'static str,
    description: &'static str,
    category: &'static str,
    inline: &'static [&'static str],
    decl: SectionDecl,
    host: Arc<vyasa_plugins::host::WasmtimeHost>,
    broker: Arc<vyasa_plugins::broker::Broker>,
    pool: sqlx::PgPool,
}

impl PluginSection {
    /// Builds the adapter, or `None` when the intern table is full — in
    /// which case the kind renders as unresolved rather than half-works.
    fn new(decl: SectionDecl, state: &AppState) -> Option<Self> {
        let kind = intern(&decl.kind)?;
        let description = intern(&decl.description)?;
        let category = category_static(&decl.category);
        let inline_refs: Option<Vec<&'static str>> =
            decl.inline.iter().map(|k| intern(k)).collect();
        let inline: &'static [&'static str] = match inline_refs {
            Some(v) if v.is_empty() => &[],
            Some(v) => intern_slice(&decl.kind, v)?,
            None => return None,
        };
        Some(Self {
            kind,
            description,
            category,
            inline,
            decl,
            host: Arc::clone(&state.plugin_host),
            broker: Arc::clone(&state.broker),
            pool: state.pool.clone(),
        })
    }
}

impl DynamicBlock for PluginSection {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn description(&self) -> &'static str {
        self.description
    }
    fn settings_schema(&self) -> serde_json::Value {
        self.decl.schema.clone()
    }
    fn category(&self) -> &'static str {
        self.category
    }
    fn sample(&self) -> serde_json::Value {
        self.decl.sample.clone()
    }
    fn inline_editable(&self) -> &'static [&'static str] {
        self.inline
    }
    fn validate_settings(
        &self,
        settings: &MapSettings,
    ) -> Vec<vyasa_themes::tokens::TokenDiagnostic> {
        // The plugin owns its schema; the host owns the binding contract.
        // Full JSON-Schema enforcement is no more the host's job here than
        // it is for editor blocks — but a malformed `bind` is host
        // territory, because the host is what resolves it.
        let mut out = Vec::new();
        if self.decl.binds {
            if let Err(errors) = Binding::from_settings(settings) {
                for message in errors {
                    out.push(vyasa_themes::tokens::TokenDiagnostic::Error {
                        path: format!("{}.settings", self.kind),
                        message,
                    });
                }
            }
        }
        out
    }
    fn resolve<'a>(
        &'a self,
        ctx: &'a ResolveContext<'a>,
        settings: &'a MapSettings,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<BlockPayload, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            // The host resolves the binding with the same query path and
            // the same visibility rules every core section uses.
            let bound = if self.decl.binds {
                match Binding::from_settings(settings) {
                    Ok(Some(bind)) => ctx.queries.bound_entries(&bind).await.unwrap_or_default(),
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };
            let bound_json: Vec<serde_json::Value> = bound
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "id": p.id,
                        "title": p.title,
                        "url": p.url,
                        "excerpt": p.excerpt,
                        "date": p.date,
                        "thumb_url": p.thumb_url,
                    })
                })
                .collect();
            let payload = serde_json::json!({
                "kind": self.decl.kind,
                "settings": serde_json::to_value(settings).unwrap_or_default(),
                "bound": bound_json,
                "editor": ctx.editor,
            })
            .to_string();

            let env = HostEnv::background(Arc::clone(&self.broker), self.pool.clone());
            let reply = self
                .host
                .call_filter_at(
                    self.decl.plugin_id,
                    crate::plugin_hooks::filters::RENDER_SECTION,
                    &payload,
                    &env,
                )
                .await;
            // An unchanged payload is the contract's way of saying "not
            // mine"; empty is a render that chose to show nothing; a trap
            // or an old binary reads the same. All of them degrade instead
            // of erroring — one broken section must not take the page down.
            let html = match reply {
                V2Outcome::Ok(html) if html != payload && !html.trim().is_empty() => html,
                V2Outcome::Ok(_) | V2Outcome::Unsupported | V2Outcome::Failed(_) => {
                    return Ok(BlockPayload::Html(format!(
                        "<!-- vyasa: unresolved plugin section {} -->",
                        vyasa_themes::renderer::esc(&self.decl.kind)
                    )));
                }
            };
            // Same widened allowlist plugin blocks pass through, wrapped so
            // the theme's custom properties cascade into the markup. The
            // wrapper attribute is `data-plugin`, not `data-section` —
            // that name belongs to section *ids* and the studio's canvas
            // resolves clicks by it.
            Ok(BlockPayload::Html(format!(
                "<div class=\"vy-plugin-section\" data-plugin=\"{}\">{}</div>",
                vyasa_themes::renderer::esc(&self.decl.kind),
                vyasa_themes::renderer::sanitize_plugin_html(&html)
            )))
        })
    }
}

/// The live registry: built-ins plus every enabled plugin's sections.
///
/// Assembled per use rather than cached — the surface changes when plugins
/// toggle, and the built-in half is already cheap to build.
pub async fn registry_with_plugins(state: &AppState) -> DynBlockRegistry {
    let mut registry = vyasa_themes::builtin_registry();
    for decl in state.plugin_surface.sections().await {
        // A core kind can never be shadowed: the grammar requires a slash
        // and the registry refuses redefinition anyway.
        if let Some(section) = PluginSection::new(decl, state) {
            registry.register(Arc::new(section));
        }
    }
    registry
}
