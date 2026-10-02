//! Page render pipeline: (theme tokens, layout, block payloads) → HTML.
//!
//! Walks the layout for a template, renders static chrome inline, resolves
//! dynamic blocks through the registry, then hands a fully assembled JSON
//! context to the sandboxed Tera engine.

use crate::css::tokens_to_css;
use crate::dynblocks::{BlockPayload, ContentQueries, DynBlockRegistry, ResolveContext};
use crate::engine::{Engine, EngineError};
use crate::layout::{Layout, Section, TemplateType};
use crate::tokens::TokenSet;

/// Site-wide metadata injected into every page.
#[derive(Debug, Clone)]
pub struct SiteMeta {
    /// Site title.
    pub name: String,
    /// One-line description.
    pub tagline: String,
    /// BCP-47 language tag for `<html lang>`.
    pub lang: String,
    /// URL of the site logo, when one is configured.
    pub logo_url: Option<String>,
    /// `<link>`/`<script>` tags for the theme's own CSS and JS.
    ///
    /// On the site rather than the page because a theme's assets belong to
    /// every page it renders, and because a preview has to link the
    /// *draft's* assets rather than the live theme's.
    pub asset_head: String,
}

impl Default for SiteMeta {
    fn default() -> Self {
        Self {
            name: "Vyasa".to_owned(),
            tagline: String::new(),
            lang: "en".to_owned(),
            logo_url: None,
            asset_head: String::new(),
        }
    }
}

/// Per-page inputs to [`render_page`].
#[derive(Debug, Clone, Default)]
pub struct PageContext {
    /// Page `<title>` (already display-safe; escaped by Tera).
    pub title: String,
    /// For single/page templates: the entry title.
    pub post_title: Option<String>,
    /// Display name of the author.
    pub author: Option<String>,
    /// Formatted publication date.
    pub date: Option<String>,
    /// For archive/search templates: what is being listed.
    pub archive_title: Option<String>,
    /// A line under the archive title: a term description, an author's bio.
    pub archive_description: Option<String>,
    /// An image beside the archive title: an author's avatar.
    pub archive_image: Option<String>,
    /// The entry's own language, when it differs from the site's.
    pub lang: Option<String>,
    /// Post cards for listing templates.
    pub posts: Vec<PostCard>,
    /// 1-based current page of the listing.
    pub pagination_page: u32,
    /// HREF of the newer page, if any.
    pub pagination_prev: Option<String>,
    /// HREF of the older page, if any.
    pub pagination_next: Option<String>,
    /// Pre-rendered HTML injected into named regions (overrides layout
    /// blocks with the same id), e.g. the single-post body.
    pub regions_extra: Vec<(String, String)>,
    /// RFC 3339 publication timestamp for `<time datetime>`.
    pub iso_date: Option<String>,
    /// Formatted "last updated" date, when it differs from publication.
    pub updated: Option<String>,
    /// Categories and tags of the current entry.
    pub terms: Vec<TermLink>,
    /// Ready-to-emit `<head>` markup: description, canonical, social
    /// cards, feed links, icons. Built by the caller because only it
    /// knows the site URL and the request path.
    pub head: String,
    /// A message to show above the content (e.g. the outcome of posting
    /// a comment).
    pub notice: Option<String>,
    /// The entry's own section tree, when it composes itself.
    ///
    /// Empty for everything a theme lays out. A non-empty tree replaces the
    /// template's body — chrome still comes from the theme, because a page
    /// composing its own middle should not have to re-declare the site's
    /// header.
    pub sections: Vec<crate::layout::Section>,
    /// The post type's slug when rendering a custom type's entry or
    /// archive, so `single-book.html` can override `single.html` the way
    /// WordPress's hierarchy would. `None` everywhere else.
    pub post_type_slug: Option<String>,
    /// The entry on a single page, as templates read it: `entry.id`,
    /// `entry.title`, `entry.type` and `entry.fields.<key>` (custom field
    /// values, prepared by the caller for display). Autoescaped like every
    /// other value. `None` on listings.
    pub entry: Option<serde_json::Value>,
}

/// One taxonomy term as a template renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermLink {
    /// Display name.
    pub name: String,
    /// Archive URL.
    pub url: String,
    /// `category` or `tag`.
    pub taxonomy: String,
}

/// One list item rendered by index/archive templates.
#[derive(Debug, Clone)]
pub struct PostCard {
    /// Entry title.
    pub title: String,
    /// Canonical URL of the entry.
    pub url: String,
    /// Short sanitized excerpt (may contain inline HTML).
    pub excerpt: String,
    /// Search-result snippet with `<mark>` around the matched terms.
    ///
    /// Empty outside search results. Templates render this unescaped, so it
    /// may only ever be set from the search index, whose snippet generator
    /// escapes the source text before inserting its own markup. Never assign
    /// author-supplied text here.
    pub snippet: String,
    /// Author display name.
    pub author: String,
    /// Formatted date.
    pub date: String,
    /// The entry's custom field values for display (`p.fields.<key>`).
    pub fields: serde_json::Map<String, serde_json::Value>,
}

fn static_chrome(kind: &str, site: &SiteMeta) -> Option<String> {
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    Some(match kind {
        "header" => {
            // A configured logo stands in for the wordmark, with the site
            // name as its alt text so the header still reads without images.
            // Without one, the brand mark sits beside the name.
            let brand = site.logo_url.as_ref().map_or_else(
                || {
                    format!(
                        "<img class=\"vy-logo-mark\" src=\"/brand/mark.svg\" alt=\"\" \
                         width=\"64\" height=\"64\">{}",
                        esc(&site.name)
                    )
                },
                |url| {
                    format!(
                        "<img class=\"vy-logo\" src=\"{}\" alt=\"{}\">",
                        esc(url),
                        esc(&site.name)
                    )
                },
            );
            let tagline = if site.tagline.is_empty() {
                String::new()
            } else {
                format!("<p class=\"vy-tagline\">{}</p>", esc(&site.tagline))
            };
            format!(
                "<header class=\"vy-header\">\
                 <a class=\"vy-site-name\" href=\"/\">{brand}</a>{tagline}</header>"
            )
        }
        "nav" => "<nav class=\"vy-nav\"><ul><li><a href=\"/\">Home</a></li></ul></nav>".to_owned(),
        "footer" => format!("<div class=\"vy-footer-inner\">© {}</div>", esc(&site.name)),
        "sidebar-left" => "<aside class=\"vy-sidebar vy-sidebar-left\"></aside>".to_owned(),
        "sidebar-right" => "<aside class=\"vy-sidebar vy-sidebar-right\"></aside>".to_owned(),
        _ => return None,
    })
}

/// Everything one page render needs.
pub struct RenderRequest<'a> {
    /// Compiled engine (built-ins plus any theme overrides).
    pub engine: &'a Engine,
    /// Theme layout composition.
    pub layout: &'a Layout,
    /// Dynamic block registry.
    pub registry: &'a DynBlockRegistry,
    /// Active design tokens.
    pub tokens: &'a TokenSet,
    /// Which template to render.
    pub template: TemplateType,
    /// Site metadata.
    pub site: &'a SiteMeta,
    /// Page inputs.
    pub page: &'a PageContext,
    /// Data seam for dynamic blocks.
    pub queries: &'a dyn ContentQueries,
    /// Post/page id when rendering a single entry.
    pub post_id: Option<i64>,
    /// Structured content of the current entry (drives the `toc` block).
    pub content_blocks: Option<&'a [vyasa_core::block::Block]>,
    /// Comment being replied to, from `?reply_to=`.
    pub reply_to: Option<i64>,
    /// Studio preview: resolvers add `data-vy-edit` markers. Public
    /// renders pass `false`; a test asserts their output carries none.
    pub editor: bool,
}

impl<'a> RenderRequest<'a> {
    /// Builder-style setter for the current entry's blocks.
    #[must_use]
    pub const fn with_content_blocks(mut self, blocks: &'a [vyasa_core::block::Block]) -> Self {
        self.content_blocks = Some(blocks);
        self
    }
}

/// Base rules plus the section stylesheet, so a composed layout has the
/// grid and band rules its markup references; scope rules per theme; and
/// the highlight palette only when the page actually holds highlighted
/// code (it is a few hundred generated rules).
fn page_base_css(tokens: &TokenSet, layout: &Layout, page: &PageContext) -> String {
    format!(
        "{}{}{}{}",
        crate::base_css::base_css(tokens),
        crate::sections::sections_css(tokens),
        crate::css::scopes_css(layout, &page.sections, tokens),
        if page
            .regions_extra
            .iter()
            .any(|(_, html)| html.contains("vy-hl-"))
        {
            crate::renderer::highlight::highlight_css()
        } else {
            ""
        }
    )
}

/// Renders one template type end-to-end.
///
/// # Errors
/// Returns [`EngineError`] on unknown template names or Tera failures;
/// dynamic-block resolution failures surface as
/// [`EngineError`] too (they are install/theme bugs, not content issues).
#[allow(clippy::too_many_lines)]
pub async fn render_page(req: RenderRequest<'_>) -> Result<String, EngineError> {
    let RenderRequest {
        engine,
        layout,
        registry,
        tokens,
        template,
        site,
        page,
        queries,
        post_id,
        content_blocks,
        reply_to,
        editor,
    } = req;
    let ctx_resolve = ResolveContext {
        queries,
        post_id,
        content_blocks,
        reply_to,
        editor,
    };
    let default_name = match template {
        TemplateType::Index => "index.html",
        TemplateType::Single => "single.html",
        TemplateType::Archive => "archive.html",
        TemplateType::Page => "page.html",
        TemplateType::Search => "search.html",
        TemplateType::NotFound => "not-found.html",
    };
    let specific = type_template(page, default_name);
    let template_name: &str = match &specific {
        Some(name) if engine.template_names().contains(&name.as_str()) => name,
        _ => default_name,
    };

    let mut regions =
        resolve_sections(layout, template, template_name, registry, &ctx_resolve).await?;
    // Caller-provided region HTML wins over layout-resolved content.
    for (id, html) in &page.regions_extra {
        regions.insert(id.clone(), serde_json::Value::String(html.clone()));
    }

    let page_body = render_page_sections(registry, &ctx_resolve, page).await?;

    let mut ctx = serde_json::json!({
        "site": { "name": site.name, "tagline": site.tagline, "lang": site.lang },
        "page": {
            "title": page.title,
            "direction": tokens.direction.css_value(),
            "template": template.as_str(),
            "post_title": page.post_title,
            "author": page.author,
            "date": page.date,
            // The bundled and starter single-post templates read these
            // names; publishing only `author`/`date` left every byline
            // rendering as a bare separator.
            "post_author": page.author,
            "post_date": page.date,
            "post_iso_date": page.iso_date,
            "post_updated": page.updated,
            "terms": page.terms.iter().map(|t| serde_json::json!({
                "name": t.name, "url": t.url, "taxonomy": t.taxonomy,
            })).collect::<Vec<_>>(),
            "head": page.head,
            "theme_assets": site.asset_head,
            "notice": page.notice,
            "archive_title": page.archive_title,
            "archive_description": page.archive_description,
            "archive_image": page.archive_image,
            "lang": page.lang,
            "tokens_css": tokens_to_css(tokens),
            // The rules that consume those custom properties; without
            // them the page defines tokens nothing ever reads.
            // Base rules plus the section stylesheet, so a composed layout
            // has the grid and band rules its markup references. Scope rules
            // are appended per-theme below.
            "base_css": page_base_css(tokens, layout, page),
        },
        "posts": page.posts.iter().map(|p| serde_json::json!({
            "title": p.title, "url": p.url, "excerpt": p.excerpt,
            "snippet": p.snippet,
            "author": p.author, "date": p.date,
            "fields": p.fields,
        })).collect::<Vec<_>>(),
        "entry": page.entry,
        "pagination": {
            "page": page.pagination_page,
            "prev": page.pagination_prev,
            "next": page.pagination_next,
        },
        "regions": regions,
        // An entry composing itself wins; otherwise a theme that nests or
        // uses a marketing section gets its body rendered here. Flat
        // chrome-only themes get an empty string and keep placing regions
        // themselves, exactly as before.
        "sections": if page_body.is_empty() {
            composed_body(
                layout,
                template,
                &regions,
                &engine.region_ids(template_name),
                editor,
            )
        } else {
            page_body
        },
    });
    let body_placed = body_already_placed(engine, template_name, &ctx);
    finish_regions(
        &mut ctx,
        layout,
        template,
        site,
        &engine.region_ids(template_name),
        editor,
        body_placed,
    );
    fill_field_refs(&mut ctx, &engine.field_refs(template_name));
    engine.render_json(template_name, &ctx)
}

/// Gives every field a template names a value, so a field that was
/// deleted, never filled in, or whose reference no longer resolves renders
/// as nothing instead of failing the page on an undefined variable. A
/// reference read with a sub-key (`fields.cover.url`) gets an object
/// carrying that sub-key.
fn fill_field_refs(
    ctx: &mut serde_json::Value,
    refs: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) {
    use serde_json::Value;
    if refs.is_empty() {
        return;
    }
    let fill = |fields: &mut serde_json::Map<String, Value>| {
        for (key, subs) in refs {
            let slot = fields
                .entry(key.clone())
                .or_insert_with(|| Value::String(String::new()));
            if subs.is_empty() {
                continue;
            }
            if !slot.is_object() {
                // Nothing to read sub-keys from (no value, or a reference
                // that no longer resolves): an empty object instead.
                if slot.as_str().is_some_and(str::is_empty) || slot.is_null() {
                    *slot = Value::Object(serde_json::Map::new());
                } else {
                    continue;
                }
            }
            if let Some(obj) = slot.as_object_mut() {
                for sub in subs {
                    obj.entry(sub.clone())
                        .or_insert_with(|| Value::String(String::new()));
                }
            }
        }
    };
    let Some(root) = ctx.as_object_mut() else {
        return;
    };
    // Only an entry that is there: a listing has none, and inventing one
    // would make `{% if entry %}` true over an object holding only
    // `fields`. Cards in `posts` are filled only where they carry fields.
    if let Some(obj) = root.get_mut("entry").and_then(Value::as_object_mut) {
        let fields = obj
            .entry("fields")
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(map) = fields.as_object_mut() {
            fill(map);
        }
    }
    if let Some(posts) = root.get_mut("posts").and_then(Value::as_array_mut) {
        for post in posts {
            if let Some(map) = post.get_mut("fields").and_then(Value::as_object_mut) {
                fill(map);
            }
        }
    }
}

/// Resolves every section in a template, nested ones included, to HTML.
async fn resolve_sections(
    layout: &Layout,
    template: TemplateType,
    template_name: &str,
    registry: &DynBlockRegistry,
    ctx: &ResolveContext<'_>,
) -> Result<serde_json::Map<String, serde_json::Value>, EngineError> {
    let mut regions = serde_json::Map::new();
    // Walk the tree: a nested section still needs resolving, and its id is
    // what the composed renderer looks it up by.
    let all: Vec<&crate::layout::Section> = layout
        .for_template(template)
        .iter()
        .flat_map(crate::layout::Section::walk)
        .collect();
    for section in all {
        let payload = if crate::layout::STATIC_REGIONS.contains(&section.kind.as_str()) {
            BlockPayload::Empty
        } else if let Some(spec) = registry.get(&section.kind) {
            spec.resolve(ctx, &section.settings)
                .await
                .map_err(|e| EngineError(format!("section \"{}\": {e}", section.id)))?
        } else if section.kind.contains('/') {
            // A plugin section whose plugin is disabled, uninstalled or
            // never installed here. The layout survives it — same rule as
            // stored entries — and a reader sees nothing, an author finds
            // the comment in the page source.
            BlockPayload::Html(unresolved_section_comment(&section.kind))
        } else {
            return Err(EngineError(format!(
                "template {template_name}: section {} ({}) is not a known kind",
                section.id, section.kind
            )));
        };
        let html = match payload {
            BlockPayload::Html(html) => html,
            BlockPayload::Empty => continue,
            BlockPayload::Data(_) => String::new(),
        };
        regions.insert(section.id.clone(), serde_json::Value::String(html));
    }
    Ok(regions)
}

/// Resolves and renders an entry's own section tree.
///
/// Kept apart from the theme's regions on purpose: both are keyed by id, and
/// merging them would let a page called its section `header` quietly replace
/// the site's. Nothing here can reach a theme slot.
///
/// The entry's written body is offered to any `content` or `post-content`
/// section in the tree, which is what lets an author put their prose between
/// a hero and a call-to-action instead of only above or below them.
async fn render_page_sections(
    registry: &DynBlockRegistry,
    ctx: &ResolveContext<'_>,
    page: &PageContext,
) -> Result<String, EngineError> {
    let sections = &page.sections;
    if sections.is_empty() {
        return Ok(String::new());
    }
    let body = page
        .regions_extra
        .iter()
        .find(|(id, _)| id == "content" || id == "body")
        .map(|(_, html)| html.clone());

    let mut resolved = crate::sections::ResolvedSections::new();
    for section in sections.iter().flat_map(crate::layout::Section::walk) {
        let html = if matches!(section.kind.as_str(), "content" | "post-content") {
            body.clone().unwrap_or_default()
        } else if crate::layout::STATIC_REGIONS.contains(&section.kind.as_str()) {
            // Chrome inside a composed body would duplicate the header the
            // template already places; leave it to the theme.
            continue;
        } else if let Some(spec) = registry.get(&section.kind) {
            match spec
                .resolve(ctx, &section.settings)
                .await
                .map_err(|e| EngineError(format!("page section \"{}\": {e}", section.id)))?
            {
                BlockPayload::Html(html) => html,
                BlockPayload::Empty | BlockPayload::Data(_) => String::new(),
            }
        } else if section.kind.contains('/') {
            unresolved_section_comment(&section.kind)
        } else {
            return Err(EngineError(format!(
                "page section {} ({}) is not a known kind",
                section.id, section.kind
            )));
        };
        resolved.insert(section.id.clone(), html);
    }
    Ok(crate::sections::render_tree_with(
        sections, &resolved, ctx.editor,
    ))
}

/// What an unserved plugin section leaves on the page: nothing a reader
/// sees, something an author can find in the source.
fn unresolved_section_comment(kind: &str) -> String {
    format!(
        "<!-- vyasa: unresolved plugin section {} -->",
        crate::renderer::esc(kind)
    )
}

/// Renders the section tree when the template composes, else empty.
fn composed_body(
    layout: &Layout,
    template: TemplateType,
    regions: &serde_json::Map<String, serde_json::Value>,
    printed_regions: &std::collections::HashSet<String>,
    editor: bool,
) -> String {
    let sections = layout.for_template(template);
    if !crate::sections::composes(sections) {
        return String::new();
    }
    let resolved = regions
        .iter()
        .filter_map(|(id, v)| v.as_str().map(|h| (id.clone(), h.to_owned())))
        .collect();
    // The shell prints one header, nav, sidebar pair and footer from the
    // region slots; the first section of each chrome kind fills that slot
    // (see `finish_regions`), so the tree must not print it again — the
    // menu rendered twice, once in the masthead and once in the body. A
    // second menu claims no slot and stays where the layout put it — and
    // so does the first when the shell in use never reads that slot (a
    // theme's own base template may leave the nav out), because then the
    // tree is the only place the menu could appear.
    let mut claimed: Vec<&'static str> = Vec::new();
    let body: Vec<Section> = sections
        .iter()
        .filter(|s| {
            let Some(slot) = chrome_slot(&s.kind) else {
                return true;
            };
            if claimed.contains(&slot) {
                return true;
            }
            claimed.push(slot);
            !printed_regions.contains(slot)
        })
        .cloned()
        .collect();
    crate::sections::render_tree_with(&body, &resolved, editor)
}

/// Fills chrome, keys every slot by kind as well as by id, and collects
/// the blocks the template does not place itself into `page.extras`.
/// Per-type overrides, WordPress-style: `single-book.html` wins over
/// `single.html` when the theme ships one; a theme that does not stays
/// exactly as it was.
fn type_template(page: &PageContext, default_name: &str) -> Option<String> {
    page.post_type_slug
        .as_deref()
        .map(|slug| format!("{}-{slug}.html", default_name.trim_end_matches(".html")))
}

/// A template that prints the composed body has placed every layout
/// block already; the loose-block rescue must stand down or the page
/// carries everything twice.
fn body_already_placed(engine: &Engine, template_name: &str, ctx: &serde_json::Value) -> bool {
    engine.prints_sections(template_name)
        && ctx
            .get("sections")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|s| !s.is_empty())
}

/// Studio canvas only: wraps every slot-placed region so chrome and
/// flat-theme blocks are selectable. Placement stays the template's
/// business — the wrapper is marked `data-vy-slot` and the canvas offers
/// select without drag there, because dragging something the template
/// positions would be a lie. Alias copies (`regions.nav` for a `menu`
/// id) carry the same wrapper, so a click through any alias still
/// selects the one true section.
fn wrap_slot_regions(ctx: &mut serde_json::Value, layout: &Layout, template: TemplateType) {
    if let Some(obj) = ctx
        .get_mut("regions")
        .and_then(serde_json::Value::as_object_mut)
    {
        let ids: Vec<String> = layout
            .for_template(template)
            .iter()
            .map(|b| b.id.clone())
            .collect();
        for (key, value) in obj.iter_mut() {
            let Some(html) = value.as_str() else { continue };
            if html.starts_with("<div data-section=") {
                continue;
            }
            // An empty slot section is invisible to a visitor and used to
            // be invisible to the author too — unselectable, undeletable.
            // On the canvas it becomes the same labelled placeholder the
            // composed path renders. Primary ids only: aliases of an
            // empty section stay empty rather than doubling the box.
            if html.trim().is_empty() {
                if let Some(section) = layout
                    .for_template(template)
                    .iter()
                    .find(|b| b.id == *key)
                    .filter(|b| !crate::layout::STATIC_REGIONS.contains(&b.kind.as_str()))
                {
                    *value = serde_json::Value::String(format!(
                        "<div data-section=\"{}\" data-vy-slot=\"\" data-vy-empty=\"{}\"></div>",
                        crate::renderer::esc(&section.id),
                        crate::renderer::esc(&section.kind),
                    ));
                }
                continue;
            }
            // The wrapper names the section the slot came from: the id
            // itself when the key is one, else the id whose kind aliased
            // into this key — best effort, chrome keys only.
            let section_id = if ids.contains(key) {
                key.clone()
            } else {
                let aliased = layout.for_template(template).iter().find(|b| {
                    b.kind.replace('-', "_") == *key
                        || chrome_slot(&b.kind).is_some_and(|s| s == key)
                });
                match aliased {
                    Some(b) => b.id.clone(),
                    None => continue,
                }
            };
            *value = serde_json::Value::String(format!(
                "<div data-section=\"{}\" data-vy-slot=\"\">{html}</div>",
                crate::renderer::esc(&section_id)
            ));
        }
    }
}

#[allow(clippy::fn_params_excessive_bools)]
fn finish_regions(
    ctx: &mut serde_json::Value,
    layout: &Layout,
    template: TemplateType,
    site: &SiteMeta,
    rendered_ids: &std::collections::HashSet<String>,
    editor: bool,
    body_placed: bool,
) {
    // Static chrome fills any region the layout references but no dynamic
    // block produced (header/nav/sidebar/footer).
    if let Some(obj) = ctx
        .get_mut("regions")
        .and_then(serde_json::Value::as_object_mut)
    {
        for b in layout.for_template(template) {
            if obj.contains_key(&b.id) {
                continue;
            }
            if let Some(chrome) = static_chrome(&b.kind, site) {
                obj.insert(b.id.clone(), serde_json::Value::String(chrome));
            } else {
                obj.insert(b.id.clone(), serde_json::Value::String(String::new()));
            }
        }
        // Address every slot by *what it is* as well as by the id this
        // particular layout chose. A template asking for
        // `regions.sidebar_right` must get the sidebar whether the layout
        // called that block `aside`, `rail` or `sidebar-right`; before
        // this, only a layout whose ids happened to match the built-in
        // template's names rendered at all, so most of a theme's layout
        // was resolved and then thrown away.
        let mut by_kind: Vec<(String, String)> = Vec::new();
        let mut claimed: Vec<&'static str> = Vec::new();
        for b in layout.for_template(template) {
            let mut keys: Vec<String> = vec![b.kind.replace('-', "_")];
            // A chrome kind also answers to the slot it belongs in, so a
            // `menu` block fills `regions.nav`. Only the first of a kind
            // claims the slot; a second menu is a loose block.
            if let Some(slot) = chrome_slot(&b.kind) {
                if !claimed.contains(&slot) {
                    claimed.push(slot);
                    keys.push(slot.to_owned());
                }
            }
            for key in keys {
                if obj.contains_key(&key) || by_kind.iter().any(|(k, _)| *k == key) {
                    continue;
                }
                if let Some(html) = obj.get(&b.id).and_then(serde_json::Value::as_str) {
                    by_kind.push((key, html.to_owned()));
                }
            }
        }
        for (key, html) in by_kind {
            obj.insert(key, serde_json::Value::String(html));
        }
    }
    if editor {
        wrap_slot_regions(ctx, layout, template);
    }
    // Blocks the template does not place itself, in layout order, so a
    // block added to a layout appears somewhere rather than vanishing —
    // without rendering twice anything the template already positioned.
    // A composed body that the template prints IS the placement, so
    // nothing is loose.
    let mut claimed: Vec<&'static str> = Vec::new();
    let mut extras = String::new();
    for b in layout.for_template(template) {
        if body_placed {
            break;
        }
        let slot = chrome_slot(&b.kind).filter(|s| {
            let first = !claimed.contains(s);
            if first {
                claimed.push(s);
            }
            first
        });
        let placed = placed_by_template(template, &b.kind)
            || rendered_ids.contains(&b.id)
            || rendered_ids.contains(&b.kind.replace('-', "_"))
            || slot.is_some_and(|s| rendered_ids.contains(s));
        if placed {
            continue;
        }
        if let Some(html) = ctx
            .get("regions")
            .and_then(|r| r.get(&b.id))
            .and_then(serde_json::Value::as_str)
        {
            extras.push_str(html);
        }
    }
    if let Some(page_obj) = ctx
        .get_mut("page")
        .and_then(serde_json::Value::as_object_mut)
    {
        page_obj.insert("extras".to_owned(), serde_json::Value::String(extras));
    }
}

/// The chrome slot a block kind belongs in, when it has one. `menu` is
/// the navigation slot: a layout that calls its menu block `primary` must
/// still get it in the header, not appended below the content.
fn chrome_slot(kind: &str) -> Option<&'static str> {
    match kind {
        "header" => Some("header"),
        "nav" | "menu" => Some("nav"),
        "sidebar-left" => Some("sidebar_left"),
        "sidebar-right" => Some("sidebar_right"),
        "footer" => Some("footer"),
        _ => None,
    }
}

/// Whether the built-in template for `template` renders `kind` in a slot of
/// its own. Anything else is loose and goes to `page.extras`.
fn placed_by_template(template: TemplateType, kind: &str) -> bool {
    match template {
        // The listing templates render `posts` and `pagination`
        // themselves from the page context, which honours the current
        // filter; the block's own query would show something else.
        TemplateType::Index | TemplateType::Archive | TemplateType::Search => {
            matches!(kind, "latest-posts" | "pagination" | "content")
        }
        // The entry templates render the body and the thread.
        TemplateType::Single | TemplateType::Page => {
            matches!(kind, "post-content" | "content" | "comments")
        }
        TemplateType::NotFound => matches!(kind, "content"),
    }
}

#[cfg(test)]
mod field_fill_tests {
    use super::*;

    /// A macro that prints a field no type defines any more renders
    /// empty: the page does not fail on an undefined variable.
    #[test]
    fn a_macro_printing_a_deleted_field_renders_empty() {
        let engine = Engine::from_raw_for_test(&[
            (
                "m.html",
                "{% macro show(e) %}[{{ e.fields.gone }}|{{ e.fields.cover.url }}]{% endmacro %}",
            ),
            ("t.html", "{% import 'm.html' as m %}{{ m::show(e=entry) }}"),
        ]);
        let mut ctx = serde_json::json!({ "entry": { "fields": {} } });
        assert!(
            engine.render_json("t.html", &ctx).is_err(),
            "without the fill, the undefined field fails the page"
        );
        fill_field_refs(&mut ctx, &engine.field_refs("t.html"));
        assert_eq!(engine.render_json("t.html", &ctx).expect("renders"), "[|]");
    }

    /// A listing has no `entry`. Filling fields must not invent one: a
    /// layout guarding on `{% if entry %}` would then read `entry.title`
    /// from an object that has only `fields` and fail the page.
    #[test]
    fn a_listing_without_an_entry_does_not_get_one() {
        let engine = Engine::from_raw_for_test(&[(
            "t.html",
            "{% if entry %}{{ entry.title }}|{{ entry.fields.price }}{% endif %}\
             {% for p in posts %}[{{ p.title }}:{{ p.fields.price }}]{% endfor %}",
        )]);
        let mut ctx = serde_json::json!({
            "posts": [{ "title": "A", "fields": {} }, { "title": "B" }],
        });
        fill_field_refs(&mut ctx, &engine.field_refs("t.html"));
        assert!(ctx.get("entry").is_none(), "{ctx}");
        assert!(ctx["posts"][1].get("fields").is_none(), "{ctx}");
        // The card without fields is not given any either; the one with
        // them gets the missing key filled.
        assert_eq!(ctx["posts"][0]["fields"]["price"], "");
        let mut one = serde_json::json!({
            "posts": [{ "title": "A", "fields": {} }],
        });
        fill_field_refs(&mut one, &engine.field_refs("t.html"));
        assert_eq!(engine.render_json("t.html", &one).expect("renders"), "[A:]");
        // A null `entry` stays null.
        let mut null = serde_json::json!({ "entry": null, "posts": [] });
        fill_field_refs(&mut null, &engine.field_refs("t.html"));
        assert!(null["entry"].is_null(), "{null}");
    }
}
