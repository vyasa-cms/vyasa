//! Public route handlers: home, singles, archives, search, preview.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};

use serde::Deserialize;

use vyasa_db::content_models::PostRow;
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::PostFilter;
use vyasa_themes::{
    cache, keys, CachedPage, Engine, Layout, PageContext, PostCard, RenderRequest, SiteMeta,
    TemplateType, TokenSet,
};

use super::page_meta;
use crate::state::AppState;

/// Public listing page size (option-driven later).
pub const PAGE_SIZE: i64 = 10;

/// A loaded theme bundle ready to render.
pub struct ThemeBundle {
    engine: Engine,
    layout: Layout,
    tokens: TokenSet,
    version: String,
    /// The theme's own CSS and JS, as `<link>`/`<script>` tags.
    ///
    /// Content-addressed like plugin assets, so republishing identical
    /// bytes does not bust a visitor's cache, and the URL changes the
    /// moment they do.
    asset_head: String,
}

type R = Response;

/// Builds a bundle from stored documents. Templates are installed as one
/// set, so an override that `include`s a sibling is checked against that
/// sibling — the same atomic rule the package installer applies.
fn bundle_from_docs(
    tokens: &serde_json::Value,
    layout: &serde_json::Value,
    templates: Option<&serde_json::Value>,
    assets: Option<&serde_json::Value>,
    version: String,
) -> Result<ThemeBundle, String> {
    let mut engine = Engine::builtin().map_err(|e| e.to_string())?;
    let pairs: Vec<(&str, &str)> = templates
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, src)| src.as_str().map(|src| (name.as_str(), src)))
        .collect();
    engine
        .install_theme_templates(&pairs)
        .map_err(|e| e.to_string())?;
    Ok(ThemeBundle {
        layout: serde_json::from_value(layout.clone()).map_err(|e| format!("layout: {e}"))?,
        tokens: serde_json::from_value(tokens.clone()).map_err(|e| format!("tokens: {e}"))?,
        asset_head: crate::theme_assets::head_tags(assets),
        version,
        engine,
    })
}

/// The bundle built from the active theme, kept for as long as that theme
/// stays active.
///
/// Building one means fetching the theme's documents, parsing every
/// built-in template into a fresh engine, installing the theme's own
/// templates over them and deserializing the layout and tokens. Every
/// public request did all of that before it so much as looked in the
/// render cache, so a cache hit cost nearly as much as a miss.
///
/// A theme row is never edited in place: a change is a new version, an
/// activation flips the flag. So `(name, version)` identifies a bundle
/// completely, and a request only has to ask the database for those two
/// columns to know whether the bundle it has is the one to serve.
#[derive(Clone, Default)]
pub struct BundleCache(std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<ThemeBundle>>>>);

impl BundleCache {
    fn get(&self, version: &str) -> Option<std::sync::Arc<ThemeBundle>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .filter(|b| b.version == version)
            .cloned()
    }

    fn put(&self, bundle: std::sync::Arc<ThemeBundle>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bundle);
    }
}

async fn load_bundle(state: &AppState) -> Result<std::sync::Arc<ThemeBundle>, R> {
    let identity = match state.themes.active_identity().await {
        Ok(a) => a,
        Err(e) => return Err(internal(e)),
    };
    let Some((name, version)) = identity else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no theme activated").into_response());
    };
    let namespace = crate::render_cache::theme_namespace(&name, version);
    if let Some(bundle) = state.theme_bundle.get(&namespace) {
        return Ok(bundle);
    }
    // Two requests arriving on a fresh theme may both build; both results
    // are identical and the second simply replaces the first.
    let active = match state.themes.get_active().await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return Err((StatusCode::SERVICE_UNAVAILABLE, "no theme activated").into_response())
        }
        Err(e) => return Err(internal(e)),
    };
    let bundle = std::sync::Arc::new(
        bundle_from_docs(
            &active.tokens,
            &active.layout,
            active.templates.as_ref(),
            active.assets.as_ref(),
            crate::render_cache::theme_namespace(&active.name, active.version),
        )
        .map_err(internal)?,
    );
    state.theme_bundle.put(bundle.clone());
    Ok(bundle)
}

fn internal(e: impl std::fmt::Display) -> R {
    tracing::error!("public render failed: {e}");
    crate::public::errors::internal_error()
}

fn not_found() -> R {
    crate::public::errors::not_found()
}

/// The 404 a visitor should see: rendered through the active theme.
pub(crate) async fn missing(state: &AppState) -> R {
    crate::public::errors::not_found_themed(state).await
}

/// Site identity from options, with safe defaults.
async fn site_meta(state: &AppState, asset_head: &str) -> SiteMeta {
    match state.options_service.site_identity().await {
        Ok(identity) => SiteMeta {
            asset_head: asset_head.to_owned(),
            name: if identity.title.is_empty() {
                "Vyasa".to_owned()
            } else {
                identity.title
            },
            tagline: identity.tagline,
            lang: "en".to_owned(),
            logo_url: identity
                .logo_media_id
                .map(|id| format!("/api/v1/media/{id}/raw")),
        },
        Err(_) => SiteMeta {
            asset_head: asset_head.to_owned(),
            ..SiteMeta::default()
        },
    }
}

/// Post cards for a listing: real author names, the site's date format,
/// and an excerpt derived from the entry when the author left it blank.
///
/// The excerpt is escaped here because the index template renders it
/// unescaped (a stored excerpt may legitimately contain emphasis, but it
/// is author-supplied text and must not be able to close a tag).
async fn cards(state: &AppState, site: &page_meta::SiteContext, rows: &[PostRow]) -> Vec<PostCard> {
    let authors = page_meta::author_names(state, rows).await;
    // Titles and excerpts pass through the named filter points, so a
    // plugin can badge, translate or rewrite them wherever they appear
    // rather than only on the single-post page.
    //
    // Filtered a column at a time, not a row at a time: a plugin that
    // batches sees all ten titles in one sandbox call instead of ten, and
    // a plugin that declared no interest in the point is not called at
    // all. Per-card calls measured about 2.5 ms each — most of a listing
    // render.
    let titles = crate::plugin_hooks::filter_many(
        state,
        crate::plugin_hooks::filters::POST_TITLE,
        rows.iter().map(|p| p.title.clone()).collect(),
    )
    .await;
    let excerpts = crate::plugin_hooks::filter_many(
        state,
        crate::plugin_hooks::filters::EXCERPT,
        rows.iter().map(page_meta::excerpt_for).collect(),
    )
    .await;
    // Each card carries its entry's field values for display, so an
    // archive template can show `p.fields.price`. Definitions are read
    // once per type, and every reference on the page in one query each.
    let mut defs = crate::entry_fields::Definitions::default();
    for p in rows {
        defs.of(&state.pool, p.post_type).await;
    }
    let items: Vec<(&[vyasa_core::content::FieldDef], &serde_json::Value)> = rows
        .iter()
        .map(|p| (defs.cached(p.post_type), &p.meta))
        .collect();
    let resolved =
        crate::entry_fields::Resolved::load(&state.plugin_surface, &state.pool, &items).await;
    let fields: Vec<serde_json::Map<String, serde_json::Value>> = items
        .iter()
        .map(|(d, meta)| resolved.render(&site.permalinks, d, meta))
        .collect();
    rows.iter()
        .zip(titles)
        .zip(excerpts)
        .zip(fields)
        .map(|(((p, title), excerpt), fields)| PostCard {
            title,
            url: site.permalinks.path_for(p),
            excerpt: escape_text(&excerpt),
            snippet: String::new(),
            author: authors.get(&p.author_id).cloned().unwrap_or_default(),
            date: site.date(p.published_at.unwrap_or(p.created_at)),
            fields,
        })
        .collect()
}

/// The `entry` object a single template reads: id, title, type and the
/// display form of its field values.
async fn entry_context(state: &AppState, post: &PostRow) -> serde_json::Value {
    let pattern = crate::permalinks::pattern(state).await;
    let mut defs = crate::entry_fields::Definitions::default();
    let fields = crate::entry_fields::render_values(
        &state.plugin_surface,
        &state.pool,
        &pattern,
        defs.of(&state.pool, post.post_type).await,
        &post.meta,
    )
    .await;
    serde_json::json!({
        "id": post.id.to_string(),
        "title": post.title,
        "type": post.post_type.as_str(),
        "fields": fields,
    })
}

/// Percent-encodes a query-string value.
fn urlencode(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => String::from("+"),
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Escapes text destined for a template that renders it unescaped.
fn escape_text(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// How a public page may be cached by a browser or a CDN.
///
/// `max-age=0, must-revalidate` with an ETag is the honest setting for a
/// CMS: a visitor revalidates and gets a 304 with no body, so the bytes
/// are saved without anyone serving content that has since changed. There
/// was no `Cache-Control` at all before, which meant every visit
/// re-downloaded the whole page and no CDN would hold anything.
///
/// An operator who wants edge caching sets `s-maxage` at the CDN, where
/// they also control purging.
const HTML_CACHE_CONTROL: &str = "public, max-age=0, must-revalidate";

/// The same, plus an edge lifetime when the site has one configured.
///
/// `s-maxage` applies to shared caches only, so a browser still
/// revalidates while a CDN may serve from its own copy — which is the
/// whole point: a 4 ms origin render becomes a 0 ms edge hit. Safe only
/// because a publish purges the edge; with no purger configured the
/// option defaults to zero and this returns the plain policy.
fn cache_control_for(edge_seconds: u64) -> String {
    if edge_seconds == 0 {
        return HTML_CACHE_CONTROL.to_owned();
    }
    // `stale-while-revalidate` lets the edge answer instantly from a copy
    // it is already replacing, so a purge costs one slow request rather
    // than a thundering herd.
    format!(
        "{HTML_CACHE_CONTROL}, s-maxage={edge_seconds}, stale-while-revalidate={}",
        edge_seconds.saturating_mul(10).min(86_400)
    )
}

/// The policy for a page that belongs to one visitor.
///
/// A password-protected post is unlocked by a cookie. Serving the unlocked
/// body under the shared policy above told every cache between here and
/// the visitor that it could keep the body and hand it to the next person
/// — who had no cookie and had never seen the password. With
/// `edge_cache_seconds` set, a CDN would do exactly that for up to a day.
/// `no-store` forbids keeping it; `Vary: Cookie` says why.
const PRIVATE_CACHE_CONTROL: &str = "private, no-store";

/// Renders a page that must never be stored or shared.
///
/// Does not touch the render cache, in either direction: a page that
/// differs by cookie has no key that is safe to look up.
async fn respond_private(render: impl std::future::Future<Output = Result<String, R>>) -> R {
    match render.await {
        Ok(html) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, PRIVATE_CACHE_CONTROL),
                (header::VARY, "Cookie"),
            ],
            html,
        )
            .into_response(),
        Err(resp) => resp,
    }
}

/// Wraps rendered HTML with the headers every public page carries.
fn html_response(page: &CachedPage, cache_control: &str) -> R {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, cache_control),
            (header::ETAG, page.etag.as_str()),
        ],
        page.html.clone(),
    )
        .into_response()
}

async fn respond_cached(
    state: &AppState,
    headers: &HeaderMap,
    key: Option<String>,
    render: impl std::future::Future<Output = Result<String, R>>,
) -> R {
    // The canonical URL falls back to the request's own origin when the
    // site has not been told its address, so a page genuinely differs by
    // host. Keying on it means a forged `Host` gets its own entry instead
    // of poisoning the one every other visitor is served.
    //
    // Appended, not prefixed: purges match on a `t{version}:single:` style
    // prefix, so putting the origin in front silently stopped every
    // invalidation — approving a comment left the old page in the cache.
    //
    // The plugin-surface epoch rides along for the same reason it exists:
    // enabling a plugin changes what a page may contain, so old entries
    // should become unreachable. Keying on it beats purging everything,
    // which threw away every page including the ones no plugin touches.
    let epoch = state
        .surface_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    let key = key.map(|k| {
        format!(
            "{k}|{}|e{epoch}",
            crate::feeds::origin_from_headers(headers)
        )
    });
    let cache_control = cache_control_for(
        state
            .options_service
            .edge_cache_seconds()
            .await
            .unwrap_or(0),
    );
    let Some(cache_svc) = &state.render_cache else {
        return match render.await {
            // Still ETagged: a site running without the render cache
            // should not also lose conditional requests.
            Ok(html) => html_response(&CachedPage::new(html), &cache_control),
            Err(resp) => resp,
        };
    };
    if let Some(key) = &key {
        if let Some(page) = cache_svc.get(key) {
            return etag_response(headers, &page, &cache_control);
        }
    }
    match render.await {
        Ok(html) => {
            let page = CachedPage::new(html);
            if let Some(key) = key {
                cache_svc.insert(&key, page.clone());
            }
            html_response(&page, &cache_control)
        }
        Err(resp) => resp,
    }
}

fn etag_response(headers: &HeaderMap, page: &CachedPage, cache_control: &str) -> R {
    match cache::evaluate_if_none_match(
        headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok()),
        &page.etag,
    ) {
        // The revalidation policy travels with the 304 too, or a browser
        // that got one has nothing telling it to revalidate next time.
        cache::ConditionalGet::NotModified => (
            StatusCode::NOT_MODIFIED,
            [
                (header::CACHE_CONTROL, cache_control),
                (header::ETAG, page.etag.as_str()),
            ],
        )
            .into_response(),
        cache::ConditionalGet::Fresh(_) => html_response(page, cache_control),
    }
}

#[derive(Deserialize, Default)]
pub struct ListParams {
    #[serde(rename = "s", default)]
    pub search: Option<String>,
    #[serde(default)]
    pub page: Option<u32>,
}

/// `GET /` — home index with pagination and basic search (`?s=`).
pub async fn home(
    State(state): State<AppState>,
    Query(q): Query<ListParams>,
    headers: HeaderMap,
) -> R {
    let state_ref = &state;
    let Ok(bundle) = load_bundle(state_ref).await else {
        return crate::public::errors::internal_error();
    };
    let origin = crate::feeds::origin_from_headers(&headers);
    let site = page_meta::SiteContext::load(state_ref, &origin).await;
    let page_no = q.page.unwrap_or(1).max(1);
    let search_term = q.search.clone().filter(|s| !s.is_empty());
    let key = if search_term.is_some() || page_no > 1 {
        format!(
            "{}:home:{page_no}:{}",
            bundle.version,
            search_term.as_deref().unwrap_or("")
        )
    } else {
        keys::home(&bundle.version)
    };
    respond_cached(state_ref, &headers, Some(key), async move {
        let per_page = u32::try_from(site.per_page).unwrap_or(10);
        let filter = PostFilter {
            status: Some(PostStatus::Published),
            search: search_term.clone(),
            // One extra row answers "is there a next page?" without a
            // second COUNT query — and without offering an "Older" link
            // that leads to an empty listing.
            limit: per_page + 1,
            offset: (page_no - 1).saturating_mul(per_page),
            sticky_first: true,
            readable_types: crate::policy::readable_custom_types(state_ref, None).await,
            ..PostFilter::default()
        };
        let mut rows = state_ref.posts.list(&filter).await.map_err(internal)?;
        let has_more = rows.len() > per_page as usize;
        rows.truncate(per_page as usize);
        let query = search_term
            .as_deref()
            .map_or_else(String::new, |t| format!("&s={}", urlencode(t)));
        let page_ctx = PageContext {
            title: "Home".into(),
            posts: cards(state_ref, &site, &rows).await,
            pagination_page: page_no,
            pagination_next: has_more.then(|| format!("/?page={}{query}", page_no + 1)),
            pagination_prev: (page_no > 1).then(|| format!("/?page={}{query}", page_no - 1)),
            head: page_meta::Head::page("/", &site.title, &site.tagline)
                .filtered(state_ref)
                .await
                .render(&site),
            ..PageContext::default()
        };
        do_render(state_ref, &bundle, TemplateType::Index, &page_ctx, None).await
    })
    .await
}

/// The page with its titles passed through their named filter points, or
/// `None` when no plugin changed anything.
///
/// Shared by both render paths on purpose. `<title>` renders `page.title`
/// while `Head` only feeds `og:title`, so filtering the head alone left
/// the browser tab and the search result untouched — and there are *two*
/// renderers, so filtering in one of them left out the entry page, which
/// is the page that matters most. One helper, both callers.
///
/// Returning `None` for "unchanged" keeps the common path allocation-free:
/// with no plugin installed the filters return their input and this costs
/// two string comparisons.
async fn filtered_page(state: &AppState, page: &PageContext) -> Option<PageContext> {
    use crate::plugin_hooks::{filter, filters};

    let title = filter(state, filters::SEO_TITLE, page.title.clone()).await;
    let post_title = match &page.post_title {
        Some(raw) => Some(filter(state, filters::POST_TITLE, raw.clone()).await),
        None => None,
    };
    if title == page.title && post_title == page.post_title {
        return None;
    }
    Some(PageContext {
        title,
        post_title,
        ..page.clone()
    })
}

async fn do_render(
    state: &AppState,
    bundle: &ThemeBundle,
    template: TemplateType,
    page: &PageContext,
    post_id: Option<i64>,
) -> Result<String, R> {
    render_raw(state, bundle, template, page, post_id, false, None)
        .await
        .map_err(internal)
}

/// The render itself, with the failure as a message: the public routes
/// turn it into a 500, previews show it to the author.
///
/// `editor` marks studio previews (canvas editing markers); every public
/// route passes `false` through [`do_render`].
async fn render_raw(
    state: &AppState,
    bundle: &ThemeBundle,
    template: TemplateType,
    page: &PageContext,
    post_id: Option<i64>,
    editor: bool,
    content_blocks: Option<&[vyasa_core::block::Block]>,
) -> Result<String, String> {
    let site = site_meta(state, &bundle.asset_head).await;
    let owned = filtered_page(state, page).await;
    let page = owned.as_ref().unwrap_or(page);
    let registry = crate::plugin_sections::registry_with_plugins(state).await;
    let html = vyasa_themes::render_page(RenderRequest {
        engine: &bundle.engine,
        layout: &bundle.layout,
        registry: &registry,
        tokens: &bundle.tokens,
        template,
        site: &site,
        page,
        post_id,
        queries: state.content_queries.as_ref(),
        content_blocks,
        reply_to: None,
        editor,
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(filter_page_html(state, html).await)
}

/// `page-html` filter stage (phase 33): chained through registered
/// plugins before caching.
async fn filter_page_html(state: &AppState, html: String) -> String {
    filter_through_plugins(state, vyasa_plugins::hooks::HookPoint::PageHtml, html).await
}

/// Runs `content` through every plugin registered on `hook`.
///
/// `PageHtml` was the only stage with a call site; `CommentBody`,
/// `FeedItem` and `SeoTitle` existed in the contract, were advertised to
/// plugin authors, and were dispatched from nowhere.
pub async fn filter_through_plugins(
    state: &AppState,
    hook: vyasa_plugins::hooks::HookPoint,
    content: String,
) -> String {
    if state.hook_registry.is_empty(hook).await {
        return content;
    }
    state
        .hook_registry
        .dispatch_filter(
            hook,
            content,
            &FilterBridge { state },
            &DegradedBridge { state },
        )
        .await
}

/// Bridges [`AppState`] into the plugin-side filter executor trait.
struct FilterBridge<'a> {
    state: &'a AppState,
}

impl vyasa_plugins::hooks::FilterExecutor for FilterBridge<'_> {
    fn invoke(
        &self,
        plugin_id: i64,
        hook: vyasa_plugins::hooks::HookPoint,
        content: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>> {
        // SeoTitle reuses the content stage (no dedicated WIT variant v1).
        let stage = match hook {
            vyasa_plugins::hooks::HookPoint::PageHtml => vyasa_plugins::host::FilterStage::PageHtml,
            vyasa_plugins::hooks::HookPoint::FeedItem => vyasa_plugins::host::FilterStage::FeedItem,
            vyasa_plugins::hooks::HookPoint::CommentBody => {
                vyasa_plugins::host::FilterStage::CommentBody
            }
            // Content, SeoTitle and AuthChallenge all ride the content
            // stage: WIT v1 has no dedicated variant for them, and the
            // payload itself carries the discriminator.
            vyasa_plugins::hooks::HookPoint::Content
            | vyasa_plugins::hooks::HookPoint::SeoTitle
            | vyasa_plugins::hooks::HookPoint::AuthChallenge => {
                vyasa_plugins::host::FilterStage::Content
            }
        };
        let broker = std::sync::Arc::clone(&self.state.broker);
        let dest = self.state.pool.clone();
        let host = std::sync::Arc::clone(&self.state.plugin_host);
        Box::pin(async move {
            let env = vyasa_plugins::host::HostEnv::background(broker, dest);
            match host.invoke_filter(plugin_id, stage, content, &env).await {
                vyasa_plugins::host::InvokeOutcome::Ok(html) => Ok(html),
                vyasa_plugins::host::InvokeOutcome::Failed(reason) => Err(reason),
            }
        })
    }
}

/// Bridges degraded-plugin notifications to DB status updates.
struct DegradedBridge<'a> {
    state: &'a AppState,
}

impl vyasa_plugins::hooks::DegradedSink for DegradedBridge<'_> {
    fn mark_degraded(&self, plugin_id: i64, reason: &str) {
        let repo = vyasa_db::repo::PluginsRepo::new(self.state.pool.clone());
        let reason = reason.to_owned();
        tokio::spawn(async move {
            repo.set_status(plugin_id, "degraded").await.ok();
            tracing::warn!(plugin_id, "plugin marked degraded: {reason}");
        });
    }
}

/// Query values a single entry reads: the comment outcome and the
/// comment being replied to.
#[derive(Deserialize, Default)]
pub struct EntryParams {
    /// Outcome of a just-posted comment (`published`/`pending`/`rejected`).
    #[serde(default)]
    pub comment: Option<String>,
    /// Comment being replied to, so the form nests under it.
    #[serde(default)]
    pub reply_to: Option<i64>,
    /// Listing page, when this path turns out to be a post-type archive.
    #[serde(default)]
    pub page: Option<u32>,
}

/// Shared single-post/page implementation.
#[allow(clippy::too_many_lines)]
pub(super) async fn single_impl(
    state: AppState,
    t: PostType,
    slug: String,
    headers: HeaderMap,
    params: EntryParams,
) -> R {
    let Ok(bundle) = load_bundle(&state).await else {
        return crate::public::errors::internal_error();
    };
    let Ok(post) = state.posts.get_by_slug(t, &slug).await else {
        return missing(&state).await;
    };
    // Public visibility: drafts/scheduled/trashed are 404 here; previews go
    // through the signed /preview route.
    if !matches!(post.status, PostStatus::Published) {
        return missing(&state).await;
    }

    // Password gate for protected posts.
    let protected = post.password_hash.is_some();
    if protected && !unlocked_by_cookie(&state, &headers, post.id) {
        return password_form(post.id);
    }

    // A page carrying a personal notice, or a form pre-filled for a
    // reply, is specific to this visitor: never cache it.
    // An entry with a timed block about to open or close is rendered
    // fresh, or the cached page would show the old state until its TTL.
    let near_boundary = serde_json::from_value::<vyasa_core::BlockDocument>(post.content.clone())
        .is_ok_and(|d| vyasa_themes::timed_boundary_within(&d.blocks, 3600));
    let cacheable = params.comment.is_none() && params.reply_to.is_none() && !near_boundary;
    let key = cacheable.then(|| {
        keys::single(
            &bundle.version,
            &post.slug,
            post.updated_at.timestamp_millis(),
        )
    });
    let state_ref = &state;
    let origin = crate::feeds::origin_from_headers(&headers);
    let render = async move {
        let mut doc: vyasa_core::BlockDocument = serde_json::from_value(post.content.clone())
            .unwrap_or_else(|_| vyasa_core::BlockDocument::new(Vec::new()));
        crate::plugin_blocks::resolve(state_ref, &mut doc.blocks).await;
        crate::patterns::resolve(state_ref, &mut doc.blocks).await;
        crate::forms::resolve(state_ref, &mut doc.blocks).await;
        let content_html = vyasa_themes::render_blocks(&doc.blocks).map_err(internal)?;
        // `content` runs on the entry body before the template wraps it.
        // Nothing was ever registered on this stage, so post-published
        // and post-updated actions — addressed to the same audience —
        // reached no plugin either.
        let content_html = filter_through_plugins(
            state_ref,
            vyasa_plugins::hooks::HookPoint::Content,
            content_html,
        )
        .await;
        // Starter themes disagree on the post-body slot id (`body` vs
        // `content`); inject under both names so any theme's `post-content`
        // placeholder is replaced. `body` is the blog/portfolio default,
        // `content` is the docs default.
        let site = page_meta::SiteContext::load(state_ref, &origin).await;
        let published = post.published_at.unwrap_or(post.created_at);
        let author = page_meta::author_names(state_ref, std::slice::from_ref(&post))
            .await
            .get(&post.author_id)
            .cloned();
        let path = site.permalinks.path_for(&post);
        let notice = params
            .comment
            .as_deref()
            .and_then(crate::public::comment_form::Outcome::parse)
            .map(|o| o.message().to_owned());
        // Published, open translations, for hreflang links both ways: a
        // password-protected entry is not advertised.
        let site_lang = state_ref
            .options
            .get("site_language")
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| String::from("en"));
        let mut alternates = Vec::new();
        if let Some(group) = post.translation_group {
            let translations = vyasa_db::repo::PostsRepo::new(state_ref.pool.clone())
                .translations(group)
                .await
                .unwrap_or_default();
            for row in translations {
                if row.id == post.id
                    || row.status != PostStatus::Published
                    || row.password_hash.is_some()
                    || !crate::policy::publicly_openable(
                        &state_ref.plugin_surface,
                        &state_ref.pool,
                        row.post_type,
                    )
                    .await
                {
                    continue;
                }
                alternates.push((
                    if row.lang.is_empty() {
                        site_lang.clone()
                    } else {
                        row.lang.clone()
                    },
                    site.permalinks.path_for(&row),
                ));
            }
        }
        let page_ctx = PageContext {
            title: post.title.clone(),
            post_title: Some(post.title.clone()),
            author: author.clone(),
            date: Some(site.date(published)),
            iso_date: Some(published.to_rfc3339()),
            updated: (post.updated_at.date_naive() != published.date_naive())
                .then(|| site.date(post.updated_at)),
            terms: page_meta::terms_for(state_ref, post.id).await,
            lang: (!post.lang.is_empty()).then(|| post.lang.clone()),
            head: page_meta::Head::entry(&post, &path, author)
                .with_alternates(alternates)
                .filtered(state_ref)
                .await
                .render(&site),
            notice,
            regions_extra: vec![
                ("body".to_owned(), content_html.clone()),
                ("content".to_owned(), content_html),
            ],
            // A page that composes itself renders its own tree instead of
            // the theme's `page` body. Anything else leaves this empty and
            // the template lays the entry out exactly as before.
            sections: vyasa_themes::page_sections(post.layout.as_ref()),
            // Lets `single-book.html` override `single.html` for a theme
            // that ships one; a theme that does not renders as before.
            post_type_slug: match post.post_type {
                vyasa_db::content_models::PostType::Custom(t) => Some(t.to_owned()),
                _ => None,
            },
            entry: Some(entry_context(state_ref, &post).await),
            ..PageContext::default()
        };
        let mut page_ctx = page_ctx;
        page_ctx
            .head
            .push_str(&hreflang_links(state_ref, &site, post.id).await);
        let template = match post.post_type {
            PostType::Page => TemplateType::Page,
            _ => TemplateType::Single,
        };
        // The entry's own blocks drive the `toc` block, which resolved to
        // nothing on every public page before they were passed through.
        render_entry(
            state_ref,
            &bundle,
            template,
            &page_ctx,
            post.id,
            &doc.blocks,
            params.reply_to,
            false,
        )
        .await
    };
    if protected {
        respond_private(render).await
    } else {
        respond_cached(state_ref, &headers, key, render).await
    }
}

/// `hreflang` alternates for an entry that belongs to a translation
/// group with more than one published member. Absolute URLs only (the
/// spec requires them); an entry with no language story emits nothing.
/// `x-default` points at the group's first member, which sorts by
/// language tag — stable, if arbitrary; declaring one explicitly can
/// come later.
async fn hreflang_links(state: &AppState, site: &page_meta::SiteContext, post_id: i64) -> String {
    use std::fmt::Write as _;
    let Ok(all) = state.translations.alternates(post_id).await else {
        return String::new();
    };
    // Only members a visitor may open: a group from before translations
    // had to share a type may hold one of a non-public type.
    let mut alternates = Vec::with_capacity(all.len());
    for alt in all {
        let Ok(t) = PostType::parse(&alt.post_type) else {
            continue;
        };
        if crate::policy::publicly_openable(&state.plugin_surface, &state.pool, t).await {
            alternates.push(alt);
        }
    }
    if alternates.len() < 2 {
        return String::new();
    }
    let mut out = String::new();
    for alt in &alternates {
        let Ok(t) = PostType::parse(&alt.post_type) else {
            continue;
        };
        let Ok(row) = state.posts.get_by_slug(t, &alt.slug).await else {
            continue;
        };
        if let Some(url) = site.absolute(&site.permalinks.path_for(&row)) {
            let _ = write!(
                out,
                "\n<link rel=\"alternate\" hreflang=\"{}\" href=\"{}\">",
                alt.lang, url
            );
        }
    }
    if let Some(first) = alternates.first() {
        if let Ok(t) = PostType::parse(&first.post_type) {
            if let Ok(row) = state.posts.get_by_slug(t, &first.slug).await {
                if let Some(url) = site.absolute(&site.permalinks.path_for(&row)) {
                    let _ = write!(
                        out,
                        "\n<link rel=\"alternate\" hreflang=\"x-default\" href=\"{url}\">"
                    );
                }
            }
        }
    }
    out
}

/// Renders one entry, handing the renderer its structured content and the
/// comment being replied to.
#[allow(clippy::too_many_arguments)]
async fn render_entry(
    state: &AppState,
    bundle: &ThemeBundle,
    template: TemplateType,
    page: &PageContext,
    post_id: i64,
    blocks: &[vyasa_core::block::Block],
    reply_to: Option<i64>,
    editor: bool,
) -> Result<String, R> {
    let site = site_meta(state, &bundle.asset_head).await;
    let owned = filtered_page(state, page).await;
    let page = owned.as_ref().unwrap_or(page);
    let registry = crate::plugin_sections::registry_with_plugins(state).await;
    let html = vyasa_themes::render_page(RenderRequest {
        engine: &bundle.engine,
        layout: &bundle.layout,
        registry: &registry,
        tokens: &bundle.tokens,
        template,
        site: &site,
        page,
        post_id: Some(post_id),
        queries: state.content_queries.as_ref(),
        content_blocks: Some(blocks),
        reply_to,
        editor,
    })
    .await
    .map_err(internal)?;
    Ok(filter_page_html(state, html).await)
}

/// The markdown mirror of a published entry, when `slug` asks for one
/// with a `.md` suffix.
///
/// AI answer engines cite what they can read cleanly; this is the same
/// block document as the HTML page, serialized as markdown at a sibling
/// URL. Anything a visitor could not read stays unreadable here too:
/// unpublished and password-protected entries 404 rather than leak
/// through the mirror.
pub(super) async fn markdown_mirror(
    state: &AppState,
    t: PostType,
    slug: &str,
    headers: &HeaderMap,
) -> R {
    use std::fmt::Write as _;
    let Ok(post) = state.posts.get_by_slug(t, slug).await else {
        return missing(state).await;
    };
    if !matches!(post.status, PostStatus::Published) || post.password_hash.is_some() {
        return missing(state).await;
    }
    let doc: vyasa_core::BlockDocument = serde_json::from_value(post.content.clone())
        .unwrap_or_else(|_| vyasa_core::BlockDocument::new(Vec::new()));
    let body = vyasa_themes::blocks_to_markdown(&doc.blocks);
    let origin = crate::feeds::origin_from_headers(headers);
    let site = page_meta::SiteContext::load(state, &origin).await;
    let path = crate::permalinks::path(state, &post).await;
    let canonical = site
        .absolute(&path)
        .unwrap_or_else(|| format!("{origin}{path}"));
    let published = post.published_at.unwrap_or(post.created_at);
    let mut md = format!("# {}\n\n", post.title);
    let excerpt = post.excerpt.as_deref().unwrap_or("").trim();
    if !excerpt.is_empty() {
        let _ = writeln!(md, "> {excerpt}\n");
    }
    let _ = writeln!(
        md,
        "Published {} · Canonical: {canonical}\n\n---\n",
        published.format("%Y-%m-%d")
    );
    // Non-empty text fields after the body, labelled.
    let mut defs = crate::entry_fields::Definitions::default();
    let facts =
        crate::entry_fields::text_values(defs.of(&state.pool, post.post_type).await, &post.meta);
    md.push_str(&body);
    if !facts.is_empty() {
        md.push_str("\n\n---\n\n");
        for (label, value) in facts {
            let _ = writeln!(md, "**{label}:** {value}\n");
        }
    }
    crate::public::cacheable::cacheable(
        headers,
        "text/markdown; charset=utf-8",
        crate::public::cacheable::FEED_MAX_AGE,
        md,
    )
}

/// `GET /post/{slug}`.
pub async fn single_post(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(params): Query<EntryParams>,
    headers: HeaderMap,
) -> R {
    if let Some(bare) = slug.strip_suffix(".md") {
        return markdown_mirror(&state, PostType::Post, bare, &headers).await;
    }
    single_impl(state, PostType::Post, slug, headers, params).await
}

/// `GET /{slug}` — pages claim the root namespace.
pub async fn single_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(params): Query<EntryParams>,
    headers: HeaderMap,
) -> R {
    // `/{slug}` is the page namespace, but it is also where a registered
    // post type's archive lives. A page wins: pages predate any plugin,
    // and a plugin must not be able to hide one by picking its slug.
    //
    // The in-memory registry is consulted first on purpose. Asking the
    // database whether a page exists before consulting it put an extra
    // query on every single `/{slug}` request on every site, including the
    // overwhelming majority with no custom post types at all.
    if let Some(bare) = slug.strip_suffix(".md") {
        return markdown_mirror(&state, PostType::Page, bare, &headers).await;
    }
    if let Some(decl) =
        crate::entry_fields::served_type(&state.plugin_surface, &state.pool, &slug).await
    {
        if decl.public
            && decl.has_archive
            && state
                .posts
                .get_by_slug(PostType::Page, &slug)
                .await
                .is_err()
        {
            return custom_archive(&state, &decl, params.page.unwrap_or(1), &headers).await;
        }
    }
    single_impl(state, PostType::Page, slug, headers, params).await
}

/// `GET /{type}/{slug}` — one entry of a registered post type (a plugin's
/// or an administrator's).
pub async fn custom_single(
    State(state): State<AppState>,
    Path((first, slug)): Path<(String, String)>,
    Query(params): Query<EntryParams>,
    headers: HeaderMap,
) -> R {
    // `/{first}/{slug}` is claimed by two kinds of registration. Post
    // types win, and `collect_taxonomies` refuses a slug a post type
    // already holds, so the two can never both match.
    if let Some(decl) =
        crate::entry_fields::served_type(&state.plugin_surface, &state.pool, &first).await
    {
        let Ok(post_type) = PostType::parse(&first) else {
            return missing(&state).await;
        };
        if !decl.public {
            return missing(&state).await;
        }
        if let Some(bare) = slug.strip_suffix(".md") {
            return markdown_mirror(&state, post_type, bare, &headers).await;
        }
        return single_impl(state, post_type, slug, headers, params).await;
    }
    if let Some(decl) = state.plugin_surface.taxonomy(&first).await {
        if !decl.public {
            return missing(&state).await;
        }
        // The interned name, not the request's copy: `archive_impl` takes
        // a `&'static str` because it keys the cache with it.
        let Ok(tax) = vyasa_db::content_models::Taxonomy::parse(&first) else {
            return missing(&state).await;
        };
        return archive_impl(state, tax.as_str(), slug, params.page.unwrap_or(1), headers).await;
    }
    missing(&state).await
}

/// The listing for one registered post type.
async fn custom_archive(
    state: &AppState,
    decl: &crate::entry_fields::TypeDecl,
    page_no: u32,
    headers: &HeaderMap,
) -> R {
    let Ok(post_type) = PostType::parse(&decl.slug) else {
        return missing(state).await;
    };
    let Ok(bundle) = load_bundle(state).await else {
        return crate::public::errors::internal_error();
    };
    let page_no = page_no.max(1);
    // Through the keys module: written by hand this key lacked the "t"
    // namespace prefix the invalidator sweeps, so no content event ever
    // purged a custom-type archive.
    let key = keys::archive(&bundle.version, "type", &decl.slug, page_no);
    let plural = decl.plural.clone();
    let base = format!("/{}", decl.slug);
    let origin = crate::feeds::origin_from_headers(headers);
    respond_cached(state, headers, Some(key), async move {
        let site = page_meta::SiteContext::load(state, &origin).await;
        let per_page = u32::try_from(site.per_page).unwrap_or(10);
        let filter = PostFilter {
            status: Some(PostStatus::Published),
            post_type: Some(post_type),
            limit: per_page + 1,
            offset: (page_no - 1).saturating_mul(per_page),
            ..PostFilter::default()
        };
        let mut rows = state.posts.list(&filter).await.map_err(internal)?;
        let has_more = rows.len() > per_page as usize;
        rows.truncate(per_page as usize);
        let page_ctx = PageContext {
            title: plural.clone(),
            archive_title: Some(plural.clone()),
            posts: cards(state, &site, &rows).await,
            pagination_page: page_no,
            pagination_next: has_more.then(|| format!("{base}?page={}", page_no + 1)),
            pagination_prev: (page_no > 1).then(|| format!("{base}?page={}", page_no - 1)),
            head: page_meta::Head::page(&base, &plural, &format!("All {plural}."))
                .filtered(state)
                .await
                .render(&site),
            post_type_slug: Some(decl.slug.clone()),
            ..PageContext::default()
        };
        do_render(state, &bundle, TemplateType::Archive, &page_ctx, None).await
    })
    .await
}

fn password_form(post_id: i64) -> R {
    // The form used to post to "{permalink}/password", and only the post
    // permalink had such a route: a protected page posted to
    // /{slug}/password, which is the GET-only entry route (405), and a
    // custom-type entry to /{type}/{slug}/password, which is nothing (404).
    // Even the handler assumed PostType::Post. So a protected page or
    // custom entry could never be unlocked by anyone, and the Unlock
    // button just failed. One endpoint, the id in the form, no guessing.
    //
    // No Cache-Control at all let a browser keep the gate heuristically,
    // so a visitor who had just unlocked the post could be shown the form
    // again from their own cache.
    (
        [(header::CACHE_CONTROL, PRIVATE_CACHE_CONTROL)],
        Html(format!(
            "<!doctype html><html><head><title>Protected</title></head><body>\
         <form method=\"post\" action=\"/unlock\" class=\"vy-password\">\
         <input type=\"hidden\" name=\"post_id\" value=\"{post_id}\">\
         <p>This content is password protected.</p>\
         <input type=\"password\" name=\"password\" aria-label=\"Password\">\
         <button type=\"submit\">Unlock</button></form></body></html>"
        )),
    )
        .into_response()
}

/// The unlock token the request's cookie carries for this entry, if any
/// (not yet verified).
pub(crate) fn unlock_cookie(headers: &HeaderMap, post_id: i64) -> Option<&str> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| find_cookie(c, &format!("vy_post_{post_id}")))
}

/// Whether the request carries a valid unlock cookie for this post.
fn unlocked_by_cookie(state: &AppState, headers: &HeaderMap, post_id: i64) -> bool {
    unlock_cookie(headers, post_id).is_some_and(|token| {
        state
            .posts
            .verify_token(post_id, token, &state.private_token_secret)
            .is_ok()
    })
}

fn find_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').find_map(|pair| {
        let (k, v) = pair.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

/// `POST /post/{slug}/password` — verify and set short-lived unlock cookie.
pub async fn unlock_post(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    body: String,
) -> R {
    // Kept for links and forms that predate /unlock; posts only, as before.
    let Ok(post) = state.posts.get_by_slug(PostType::Post, &slug).await else {
        return missing(&state).await;
    };
    unlock_by_id(&state, post.id, &body).await
}

/// `POST /unlock` — the gate for every kind of entry, addressed by id.
pub async fn unlock(State(state): State<AppState>, body: String) -> R {
    let Some(post_id) = form_field(&body, "post_id").and_then(|v| v.parse::<i64>().ok()) else {
        return (StatusCode::BAD_REQUEST, "missing post_id field").into_response();
    };
    unlock_by_id(&state, post_id, &body).await
}

async fn unlock_by_id(state: &AppState, post_id: i64, body: &str) -> R {
    let Some(password) = form_field(body, "password") else {
        return (StatusCode::BAD_REQUEST, "missing password field").into_response();
    };
    let Ok(post) = state.posts.get(post_id).await else {
        return missing(state).await;
    };
    if post.password_hash.is_none() {
        return (StatusCode::BAD_REQUEST, "post is not protected").into_response();
    }
    match state
        .posts
        .verify_and_generate_token(post.id, &password, &state.private_token_secret)
        .await
    {
        Ok(token) => {
            let mut resp =
                Redirect::to(&crate::permalinks::path(state, &post).await).into_response();
            let cookie = format!(
                "vy_post_{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age=900",
                post.id
            );
            if let Ok(v) = header::HeaderValue::from_str(&cookie) {
                resp.headers_mut().append(header::SET_COOKIE, v);
            }
            resp
        }
        Err(_) => password_form(post.id),
    }
}

fn form_field(body: &str, name: &str) -> Option<String> {
    body.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| urldecode(v))
    })
}

fn urldecode(v: &str) -> String {
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'%'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[derive(Deserialize, Default)]
pub struct ArchiveParams {
    #[serde(default)]
    pub page: Option<u32>,
}

/// `GET /archive/{year}/{month}` — entries published in one month.
///
/// The `archives` block has always produced links like this; until now
/// there was no route behind them, so every month in a site's archive
/// list led to the 404 page.
pub async fn date_archive(
    State(state): State<AppState>,
    Path((year, month)): Path<(i32, u32)>,
    Query(q): Query<ArchiveParams>,
    headers: HeaderMap,
) -> R {
    if !(1..=12).contains(&month) || !(1970..=3000).contains(&year) {
        return missing(&state).await;
    }
    let state_ref = &state;
    let Ok(bundle) = load_bundle(state_ref).await else {
        return crate::public::errors::internal_error();
    };
    let page_no = q.page.unwrap_or(1).max(1);
    let key = keys::archive(
        &bundle.version,
        "date",
        &format!("{year}-{month:02}"),
        page_no,
    );
    let origin = crate::feeds::origin_from_headers(&headers);
    respond_cached(state_ref, &headers, Some(key), async move {
        let site = page_meta::SiteContext::load(state_ref, &origin).await;
        let per_page = u32::try_from(site.per_page).unwrap_or(10);
        let filter = PostFilter {
            status: Some(PostStatus::Published),
            post_type: Some(PostType::Post),
            published_month: Some((year, month)),
            limit: per_page + 1,
            offset: (page_no - 1).saturating_mul(per_page),
            ..PostFilter::default()
        };
        let mut rows = state_ref.posts.list(&filter).await.map_err(internal)?;
        let has_more = rows.len() > per_page as usize;
        rows.truncate(per_page as usize);
        let label = format!("{} {year}", MONTH_NAMES[(month as usize - 1).min(11)]);
        let base = format!("/archive/{year}/{month:02}");
        let page_ctx = PageContext {
            title: label.clone(),
            archive_title: Some(label.clone()),
            posts: cards(state_ref, &site, &rows).await,
            pagination_page: page_no,
            pagination_next: has_more.then(|| format!("{base}?page={}", page_no + 1)),
            pagination_prev: (page_no > 1).then(|| format!("{base}?page={}", page_no - 1)),
            head: page_meta::Head::page(&base, &label, &format!("Entries from {label}."))
                .filtered(state_ref)
                .await
                .render(&site),
            ..PageContext::default()
        };
        do_render(state_ref, &bundle, TemplateType::Archive, &page_ctx, None).await
    })
    .await
}

/// Month names for date-archive titles.
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `GET /category/{slug}` / `GET /tag/{slug}` share archive rendering.
macro_rules! archive_handler {
    ($name:ident, $tax:expr) => {
        pub async fn $name(
            State(state): State<AppState>,
            Path(slug): Path<String>,
            Query(q): Query<ArchiveParams>,
            headers: HeaderMap,
        ) -> R {
            archive_impl(state, $tax, slug, q.page.unwrap_or(1), headers).await
        }
    };
}

archive_handler!(category_archive, "category");
archive_handler!(tag_archive, "tag");

async fn archive_impl(
    state: AppState,
    taxonomy: &'static str,
    slug: String,
    page_no: u32,
    headers: HeaderMap,
) -> R {
    // Every other paginated handler clamps first; this one subtracted on
    // the raw value, so `?page=0` underflowed -- a panic and a dropped
    // connection in debug builds, OFFSET 4294967295 in release.
    let page_no = page_no.max(1);
    let state_ref = &state;
    let Ok(bundle) = load_bundle(state_ref).await else {
        return crate::public::errors::internal_error();
    };
    // A registered taxonomy's name is interned, so `parse` hands back a
    // `Taxonomy` that carries the same `&'static str` this function was
    // called with.
    let Ok(tax) = vyasa_db::content_models::Taxonomy::parse(taxonomy) else {
        return missing(&state).await;
    };
    let terms_repo = vyasa_db::repo::TermsRepo::new(state.pool.clone());
    let Ok(term) = terms_repo.get_by_slug(tax, &slug).await else {
        return missing(&state).await;
    };
    let key = keys::archive(&bundle.version, taxonomy, &slug, page_no.max(1));
    let origin = crate::feeds::origin_from_headers(&headers);
    respond_cached(state_ref, &headers, Some(key), async move {
        let site = page_meta::SiteContext::load(state_ref, &origin).await;
        let per_page = u32::try_from(site.per_page).unwrap_or(10);
        let filter = PostFilter {
            status: Some(PostStatus::Published),
            term_id: Some(term.id),
            limit: per_page + 1,
            offset: (page_no - 1).saturating_mul(per_page),
            readable_types: crate::policy::readable_custom_types(state_ref, None).await,
            ..PostFilter::default()
        };
        let mut rows = state_ref.posts.list(&filter).await.map_err(internal)?;
        let has_more = rows.len() > per_page as usize;
        rows.truncate(per_page as usize);
        let base = format!("/{taxonomy}/{slug}");
        let page_ctx = PageContext {
            title: format!("{} — {taxonomy}", term.name),
            archive_title: Some(term.name.clone()),
            posts: cards(state_ref, &site, &rows).await,
            pagination_page: page_no,
            pagination_next: has_more.then(|| format!("{base}?page={}", page_no + 1)),
            pagination_prev: (page_no > 1).then(|| format!("{base}?page={}", page_no - 1)),
            head: page_meta::Head::page(
                &base,
                &term.name,
                &format!("Posts filed under {}.", term.name),
            )
            .filtered(state_ref)
            .await
            .render(&site),
            ..PageContext::default()
        };
        do_render(state_ref, &bundle, TemplateType::Archive, &page_ctx, None).await
    })
    .await
}

/// `GET /author/{username}` — one person's published posts, with their
/// avatar and bio in the archive header.
pub async fn author_archive(
    State(state): State<AppState>,
    Path(username): Path<String>,
    Query(q): Query<ArchiveParams>,
    headers: HeaderMap,
) -> R {
    let page_no = q.page.unwrap_or(1).max(1);
    let state_ref = &state;
    let Ok(bundle) = load_bundle(state_ref).await else {
        return crate::public::errors::internal_error();
    };
    // One indexed lookup, not a scan of the first ten thousand accounts; and
    // an account that has not confirmed its address has no public page.
    let users = vyasa_db::repo::UsersRepo::new(state.pool.clone());
    let Ok(Some(author)) = users.public_author_by_username(&username).await else {
        return missing(&state).await;
    };
    let key = keys::archive(&bundle.version, "author", &author.username, page_no);
    let origin = crate::feeds::origin_from_headers(&headers);
    respond_cached(state_ref, &headers, Some(key), async move {
        let site = page_meta::SiteContext::load(state_ref, &origin).await;
        let per_page = u32::try_from(site.per_page).unwrap_or(10);
        let filter = PostFilter {
            status: Some(PostStatus::Published),
            post_type: Some(PostType::Post),
            author_id: Some(author.id),
            limit: per_page + 1,
            offset: (page_no - 1).saturating_mul(per_page),
            ..PostFilter::default()
        };
        let mut rows = state_ref.posts.list(&filter).await.map_err(internal)?;
        let has_more = rows.len() > per_page as usize;
        rows.truncate(per_page as usize);
        let base = format!("/author/{}", author.username);
        let description = if author.bio.trim().is_empty() {
            format!("Posts by {}.", author.display_name)
        } else {
            author.bio.clone()
        };
        let page_ctx = PageContext {
            title: format!("{} — author", author.display_name),
            archive_title: Some(author.display_name.clone()),
            archive_description: Some(description.clone()),
            archive_image: author
                .avatar_media_id
                .map(|id| format!("/api/v1/media/{id}/raw")),
            posts: cards(state_ref, &site, &rows).await,
            pagination_page: page_no,
            pagination_next: has_more.then(|| format!("{base}?page={}", page_no + 1)),
            pagination_prev: (page_no > 1).then(|| format!("{base}?page={}", page_no - 1)),
            head: page_meta::Head::page(&base, &author.display_name, &description)
                .filtered(state_ref)
                .await
                .render(&site),
            ..PageContext::default()
        };
        do_render(state_ref, &bundle, TemplateType::Archive, &page_ctx, None).await
    })
    .await
}

/// Re-reads search hits through the repository, so status and visibility
/// rules are applied by the same code path as everywhere else.
async fn published_by_id(state: &AppState, ids: &[i64]) -> Vec<PostRow> {
    let public_custom = crate::policy::PublicTypes::load(&state.plugin_surface, &state.pool).await;
    let mut found = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(row) = state.posts.get(*id).await {
            if row.status == PostStatus::Published
                && row.password_hash.is_none()
                && public_custom.opens(row.post_type)
            {
                found.push(row);
            }
        }
    }
    found
}

/// The ranked hits for a search term, with their snippets.
///
/// Prefers the full-text index and falls back to `None` (which the caller
/// turns into a SQL substring match) when the index is unavailable or has
/// nothing, so search still works on a deployment that has not been
/// reindexed. Snippets travel with the hits: they say *why* a post
/// matched, and were once computed and discarded.
async fn search_hits(state: &AppState, term: &str) -> Option<Vec<(i64, String)>> {
    let hits: Option<Vec<(i64, String)>> = match (&state.search_index, term.is_empty()) {
        (Some(index), false) => index
            .search(term, None, usize::try_from(PAGE_SIZE).unwrap_or(10), 0)
            .ok()
            .filter(|hits| !hits.is_empty())
            .map(|hits| {
                hits.into_iter()
                    .map(|h| (i64::try_from(h.id).unwrap_or_default(), h.snippet))
                    .collect()
            }),
        _ => None,
    };
    // Semantic search re-orders the full-text candidates by meaning when
    // the site has an embedding model and the switch on.
    let list = hits?;
    if !crate::ai_features::AiSettings::load(state)
        .await
        .semantic_search
    {
        return Some(list);
    }
    let order = crate::ai_features::semantic_rerank(
        state,
        None,
        term,
        list.iter().map(|(id, _)| *id).collect(),
    )
    .await;
    let mut list = list;
    list.sort_by_key(|(id, _)| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
    Some(list)
}

/// `GET /search?s=…` — full-text via Tantivy with DB fallback.
pub async fn search(
    State(state): State<AppState>,
    Query(q): Query<ListParams>,
    headers: HeaderMap,
) -> R {
    let state_ref = &state;
    let Ok(bundle) = load_bundle(state_ref).await else {
        return crate::public::errors::internal_error();
    };
    // Plugins get the terms before the index does, so a synonym expander
    // or a spelling corrector changes what is searched rather than only
    // how the results are displayed. The rewritten term is what the cache
    // key uses, so two spellings that resolve to the same query share a
    // cached page.
    let term = crate::plugin_hooks::filter(
        state_ref,
        crate::plugin_hooks::filters::SEARCH_QUERY,
        q.search.clone().unwrap_or_default(),
    )
    .await;
    // The page number belongs in the key: without it every page of a
    // search answered — and cached — as page one.
    let page_no = q.page.unwrap_or(1).max(1);
    let key = keys::search(&bundle.version, &term, page_no);
    let origin = crate::feeds::origin_from_headers(&headers);
    respond_cached(state_ref, &headers, Some(key), async move {
        let site = page_meta::SiteContext::load(state_ref, &origin).await;
        let per_page = u32::try_from(site.per_page).unwrap_or(10);
        let hits = search_hits(state_ref, &term).await;
        let indexed_ids: Option<Vec<i64>> =
            hits.as_ref().map(|h| h.iter().map(|(id, _)| *id).collect());

        let rows = if let Some(ids) = indexed_ids {
            published_by_id(state_ref, &ids).await
        } else {
            // Only types the public may see: no patterns, no custom type
            // that is not served publicly.
            let filter = PostFilter {
                status: Some(PostStatus::Published),
                search: Some(term.clone()).filter(|t| !t.is_empty()),
                limit: per_page + 1,
                offset: (page_no - 1).saturating_mul(per_page),
                readable_types: crate::policy::readable_custom_types(state_ref, None).await,
                ..PostFilter::default()
            };
            vyasa_db::repo::PostsRepo::new(state_ref.pool.clone())
                .list_visible(&filter, false, None)
                .await
                .map_err(internal)?
        };
        let has_more = rows.len() > per_page as usize;
        let rows: Vec<_> = rows.into_iter().take(per_page as usize).collect();
        let mut posts = cards(state_ref, &site, &rows).await;
        if let Some(hits) = &hits {
            for card in &mut posts {
                // Match by URL rather than index position: the repository
                // read drops anything no longer published, so the two lists
                // can differ in length.
                if let Some((_, snippet)) = rows
                    .iter()
                    .find(|row| site.permalinks.path_for(row) == card.url)
                    .and_then(|row| hits.iter().find(|(id, _)| *id == row.id))
                {
                    card.snippet.clone_from(snippet);
                }
            }
        }
        let query = urlencode(&term);
        let page_ctx = PageContext {
            title: format!("Search: {term}"),
            archive_title: Some(term.clone()),
            posts,
            pagination_page: page_no,
            pagination_next: has_more.then(|| format!("/search?s={query}&page={}", page_no + 1)),
            pagination_prev: (page_no > 1)
                .then(|| format!("/search?s={query}&page={}", page_no - 1)),
            // A search result page is thin content; keep it out of the index.
            head: {
                let mut head = page_meta::Head::page("/search", &format!("Search: {term}"), "");
                head.indexable = false;
                head.filtered(state_ref).await.render(&site)
            },
            ..PageContext::default()
        };
        do_render(state_ref, &bundle, TemplateType::Search, &page_ctx, None).await
    })
    .await
}

/// `GET /preview/{post_id}?token=…` — signed draft preview.
pub async fn preview(
    State(state): State<AppState>,
    Path(post_id): Path<i64>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> R {
    let Some(token) = params.get("token") else {
        return missing(&state).await;
    };
    if state
        .revisions
        .verify_preview_token(post_id, token, &state.private_token_secret)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "invalid or expired preview token").into_response();
    }
    let Ok(bundle) = load_bundle(&state).await else {
        return crate::public::errors::internal_error();
    };
    let Ok(post) = state.posts.get(post_id).await else {
        return missing(&state).await;
    };
    let mut doc: vyasa_core::BlockDocument = serde_json::from_value(post.content.clone())
        .unwrap_or_else(|_| vyasa_core::BlockDocument::new(Vec::new()));
    crate::plugin_blocks::resolve(&state, &mut doc.blocks).await;
    crate::patterns::resolve(&state, &mut doc.blocks).await;
    crate::forms::resolve(&state, &mut doc.blocks).await;
    let content_html = match vyasa_themes::render_blocks(&doc.blocks) {
        Ok(h) => h,
        Err(e) => return internal(e),
    };
    let page_ctx = PageContext {
        title: format!("Preview: {}", post.title),
        post_title: Some(post.title.clone()),
        regions_extra: vec![
            ("body".to_owned(), content_html.clone()),
            ("content".to_owned(), content_html),
        ],
        // A composed page previewed without its own sections showed the
        // theme's default entry layout -- the one thing the author was
        // trying to look at was the thing left out.
        sections: vyasa_themes::page_sections(post.layout.as_ref()),
        post_type_slug: match post.post_type {
            PostType::Custom(t) => Some(t.to_owned()),
            _ => None,
        },
        entry: Some(entry_context(&state, &post).await),
        ..PageContext::default()
    };
    // The template the entry will actually publish through, not always
    // `single`: a page previewed as a post got a byline and a date.
    let template = match post.post_type {
        PostType::Page => TemplateType::Page,
        _ => TemplateType::Single,
    };
    match do_render(&state, &bundle, template, &page_ctx, Some(post.id)).await {
        // Token-gated and unpublished: nothing between here and the
        // author may keep a copy.
        Ok(html) => (
            [
                (header::CACHE_CONTROL, PRIVATE_CACHE_CONTROL),
                (header::VARY, "Cookie"),
            ],
            Html(html),
        )
            .into_response(),
        Err(resp) => resp,
    }
}

/// The themed 404: the active theme's tokens, templates and chrome.
///
/// `render_standalone` builds a fresh built-in engine with no tokens, so
/// the "themed" 404 shared nothing with the site around it — no palette,
/// no header, no theme override.
pub async fn themed_not_found(state: &AppState) -> Option<String> {
    let bundle = load_bundle(state).await.ok()?;
    // Marked noindex below, so it needs no canonical and no origin.
    let site = page_meta::SiteContext::load(state, "").await;
    let page = PageContext {
        title: "Page not found".into(),
        head: {
            let mut head = page_meta::Head::page("/", "Page not found", "");
            head.indexable = false;
            head.filtered(state).await.render(&site)
        },
        ..PageContext::default()
    };
    render_raw(
        state,
        &bundle,
        TemplateType::NotFound,
        &page,
        None,
        false,
        None,
    )
    .await
    .ok()
}

/// Renders a standalone template (used as a last resort by the 404).
pub fn render_standalone(name: &str, title: &str) -> Result<String, String> {
    let engine = Engine::builtin().map_err(|e| e.to_string())?;
    let ctx = serde_json::json!({
        "site": {"name": "Vyasa", "tagline": "", "lang": "en"},
        "page": {"title": title, "direction": "ltr", "template": name, "tokens_css": "",
                 // No theme is loaded on this path — it is the last-resort
                 // 404 — so the default breakpoints are the right ones.
                 "base_css": vyasa_themes::base_css(&vyasa_themes::TokenSet::default())},
        "posts": [],
        "pagination": {},
        "regions": {"header": "", "nav": "", "sidebar_left": "", "sidebar_right": "", "footer": ""}
    });
    engine.render_json(name, &ctx).map_err(|e| e.to_string())
}

/// The Vyasa mark in brand red, the default identity of a site that has
/// not chosen its own logo or favicon yet.
pub const BRAND_MARK_SVG: &str = include_str!("../../../../assets/mark-light.svg");

/// `GET /brand/mark.svg` — the default mark.
pub async fn brand_mark() -> R {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        BRAND_MARK_SVG,
    )
        .into_response()
}

/// `GET /favicon.ico` — serves the media referenced by
/// `site_favicon_media_id`, with immutable-style cache headers; the
/// brand mark when none is set, so a tab is never blank.
pub async fn favicon(State(state): State<AppState>) -> R {
    let configured = state
        .options
        .get("site_favicon_media_id")
        .await
        .is_ok_and(|v| v.as_i64().is_some());
    if !configured {
        return brand_mark().await;
    }
    serve_media_option(&state, "site_favicon_media_id").await
}

/// `GET /logo` — serves `site_logo_media_id`.
pub async fn logo(State(state): State<AppState>) -> R {
    serve_media_option(&state, "site_logo_media_id").await
}

async fn serve_media_option(state: &AppState, key: &str) -> R {
    let Ok(Some(id)) = state.options.get(key).await.map(|v| v.as_i64()) else {
        return (StatusCode::NOT_FOUND, "not configured").into_response();
    };
    match state.media.get(id).await {
        Ok(row) => match state.media.get_bytes(&row).await {
            Ok(bytes) => (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, row.mime),
                    (header::CACHE_CONTROL, "public, max-age=3600".to_owned()),
                ],
                axum::body::Body::from(bytes),
            )
                .into_response(),
            Err(_) => not_found(),
        },
        Err(_) => not_found(),
    }
}

/// `GET /custom.css` — sanitized author CSS, namespaced under `#vy-site`.
///
/// Always returns 200 so browsers don't log noisy 404s for the `<link>`.
pub async fn custom_css_route(State(state): State<AppState>) -> R {
    let css = state.options_service.custom_css().await.unwrap_or_default();
    let body = if css.is_empty() {
        String::from("/* Vyasa custom CSS — configure via Settings → Appearance */\n")
    } else {
        vyasa_themes::namespace_custom_css(&css)
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        body,
    )
        .into_response()
}

/// Renders `path` with a theme that is not (necessarily) live — a draft,
/// a candidate the studio has not saved yet, or an installed version being
/// compared. Uncached; real published content.
///
/// # Errors
/// A message describing what failed (a template error names the
/// template), so the studio can show it in place.
pub async fn render_candidate(
    state: &AppState,
    tokens: &serde_json::Value,
    layout: &serde_json::Value,
    templates: Option<&serde_json::Value>,
    assets: Option<&serde_json::Value>,
    path: &str,
    base_theme_id: Option<i64>,
) -> Result<String, String> {
    // The preview links the draft's own assets, so an author sees the
    // stylesheet they are editing rather than the live theme's.
    let mut bundle = bundle_from_docs(tokens, layout, templates, assets, "preview".to_owned())?;
    bundle.asset_head = crate::theme_assets::preview_head(state, assets, base_theme_id)?;
    let html = render_path(state, &bundle, path).await?;
    Ok(crate::theme_assets::preview_urls(&html, base_theme_id))
}

/// Renders one entry as it would look with sections and content the editor
/// is holding but has not saved.
///
/// The theme is the live one — this previews the *page*, not a theme
/// candidate — so what comes back is what a visitor would see if the
/// author pressed save. Status is ignored on purpose: an unpublished draft
/// is exactly the thing worth looking at.
///
/// # Errors
/// A message when the post is missing or the theme cannot render it.
pub async fn render_entry_candidate(
    state: &AppState,
    post: &vyasa_db::content_models::PostRow,
    sections: Vec<vyasa_themes::Section>,
    content: &serde_json::Value,
) -> Result<String, String> {
    let bundle = load_bundle(state)
        .await
        .map_err(|_| "no theme activated — activate one to preview".to_owned())?;
    let mut doc: vyasa_core::BlockDocument = serde_json::from_value(content.clone())
        .unwrap_or_else(|_| vyasa_core::BlockDocument::new(Vec::new()));
    crate::plugin_blocks::resolve(state, &mut doc.blocks).await;
    crate::patterns::resolve(state, &mut doc.blocks).await;
    crate::forms::resolve(state, &mut doc.blocks).await;
    let content_html = vyasa_themes::render_blocks(&doc.blocks).map_err(|e| e.to_string())?;
    let page = PageContext {
        title: post.title.clone(),
        post_title: Some(post.title.clone()),
        date: Some(
            post.published_at
                .unwrap_or(post.created_at)
                .format("%b %e, %Y")
                .to_string(),
        ),
        regions_extra: vec![
            ("body".to_owned(), content_html.clone()),
            ("content".to_owned(), content_html),
        ],
        sections,
        ..PageContext::default()
    };
    let template = match post.post_type {
        PostType::Page => TemplateType::Page,
        _ => TemplateType::Single,
    };
    render_raw(
        state,
        &bundle,
        template,
        &page,
        Some(post.id),
        true,
        Some(&doc.blocks),
    )
    .await
}

/// Resolves a site path the way the public router does and renders it.
async fn render_path(state: &AppState, bundle: &ThemeBundle, path: &str) -> Result<String, String> {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    let segments: Vec<&str> = route.split('/').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        [] => preview_index(state, bundle).await,
        ["post", slug] => preview_single(state, bundle, PostType::Post, slug).await,
        ["category", slug] => preview_archive(state, bundle, "category", slug).await,
        ["tag", slug] => preview_archive(state, bundle, "tag", slug).await,
        ["search"] => {
            let term = form_field(query, "s").unwrap_or_default();
            preview_search(state, bundle, &term).await
        }
        [slug] => preview_single(state, bundle, PostType::Page, slug).await,
        // A registered custom type's entry, so `single-book.html` can be
        // designed against a real book. Anything else is the 404 template,
        // which is itself worth previewing.
        [first, slug] => {
            if crate::entry_fields::served_type(&state.plugin_surface, &state.pool, first)
                .await
                .is_some()
            {
                match PostType::parse(first) {
                    Ok(t) => preview_single(state, bundle, t, slug).await,
                    Err(_) => preview_not_found(state, bundle).await,
                }
            } else {
                preview_not_found(state, bundle).await
            }
        }
        _ => preview_not_found(state, bundle).await,
    }
}

async fn preview_index(state: &AppState, bundle: &ThemeBundle) -> Result<String, String> {
    let filter = PostFilter {
        status: Some(PostStatus::Published),
        limit: u32::try_from(PAGE_SIZE).unwrap_or(u32::MAX),
        readable_types: crate::policy::readable_custom_types(state, None).await,
        ..PostFilter::default()
    };
    let rows = state.posts.list(&filter).await.map_err(|e| e.to_string())?;
    let page = PageContext {
        title: "Home".into(),
        posts: cards(state, &page_meta::SiteContext::load(state, "").await, &rows).await,
        pagination_page: 1,
        pagination_next: (!rows.is_empty()).then(|| "/?page=2".to_owned()),
        ..PageContext::default()
    };
    render_raw(state, bundle, TemplateType::Index, &page, None, true, None).await
}

async fn preview_single(
    state: &AppState,
    bundle: &ThemeBundle,
    t: PostType,
    slug: &str,
) -> Result<String, String> {
    let Ok(post) = state.posts.get_by_slug(t, slug).await else {
        return preview_not_found(state, bundle).await;
    };
    if !matches!(post.status, PostStatus::Published) {
        return preview_not_found(state, bundle).await;
    }
    let mut doc: vyasa_core::BlockDocument = serde_json::from_value(post.content.clone())
        .unwrap_or_else(|_| vyasa_core::BlockDocument::new(Vec::new()));
    crate::plugin_blocks::resolve(state, &mut doc.blocks).await;
    crate::patterns::resolve(state, &mut doc.blocks).await;
    crate::forms::resolve(state, &mut doc.blocks).await;
    let content_html = vyasa_themes::render_blocks(&doc.blocks).map_err(|e| e.to_string())?;
    let page = PageContext {
        title: post.title.clone(),
        post_title: Some(post.title.clone()),
        date: Some(
            post.published_at
                .unwrap_or(post.created_at)
                .format("%b %e, %Y")
                .to_string(),
        ),
        regions_extra: vec![
            ("body".to_owned(), content_html.clone()),
            ("content".to_owned(), content_html),
        ],
        sections: vyasa_themes::page_sections(post.layout.as_ref()),
        post_type_slug: match post.post_type {
            PostType::Custom(t) => Some(t.to_owned()),
            _ => None,
        },
        entry: Some(entry_context(state, &post).await),
        ..PageContext::default()
    };
    let template = match post.post_type {
        PostType::Page => TemplateType::Page,
        _ => TemplateType::Single,
    };
    render_raw(
        state,
        bundle,
        template,
        &page,
        Some(post.id),
        true,
        Some(&doc.blocks),
    )
    .await
}

async fn preview_archive(
    state: &AppState,
    bundle: &ThemeBundle,
    taxonomy: &str,
    slug: &str,
) -> Result<String, String> {
    let tax = if taxonomy == "tag" {
        vyasa_db::content_models::Taxonomy::Tag
    } else {
        vyasa_db::content_models::Taxonomy::Category
    };
    let terms_repo = vyasa_db::repo::TermsRepo::new(state.pool.clone());
    let Ok(term) = terms_repo.get_by_slug(tax, slug).await else {
        return preview_not_found(state, bundle).await;
    };
    let filter = PostFilter {
        status: Some(PostStatus::Published),
        term_id: Some(term.id),
        limit: u32::try_from(PAGE_SIZE).unwrap_or(u32::MAX),
        readable_types: crate::policy::readable_custom_types(state, None).await,
        ..PostFilter::default()
    };
    let rows = state.posts.list(&filter).await.map_err(|e| e.to_string())?;
    let page = PageContext {
        title: format!("{} — {taxonomy}", term.name),
        archive_title: Some(term.name.clone()),
        posts: cards(state, &page_meta::SiteContext::load(state, "").await, &rows).await,
        pagination_page: 1,
        ..PageContext::default()
    };
    render_raw(
        state,
        bundle,
        TemplateType::Archive,
        &page,
        None,
        true,
        None,
    )
    .await
}

async fn preview_search(
    state: &AppState,
    bundle: &ThemeBundle,
    term: &str,
) -> Result<String, String> {
    let filter = PostFilter {
        status: Some(PostStatus::Published),
        search: Some(term.to_owned()).filter(|t| !t.is_empty()),
        limit: u32::try_from(PAGE_SIZE).unwrap_or(u32::MAX),
        readable_types: crate::policy::readable_custom_types(state, None).await,
        ..PostFilter::default()
    };
    let rows = state.posts.list(&filter).await.map_err(|e| e.to_string())?;
    let page = PageContext {
        title: format!("Search: {term}"),
        archive_title: Some(term.to_owned()),
        posts: cards(state, &page_meta::SiteContext::load(state, "").await, &rows).await,
        ..PageContext::default()
    };
    render_raw(state, bundle, TemplateType::Search, &page, None, true, None).await
}

async fn preview_not_found(state: &AppState, bundle: &ThemeBundle) -> Result<String, String> {
    let page = PageContext {
        title: "Not found".into(),
        ..PageContext::default()
    };
    render_raw(
        state,
        bundle,
        TemplateType::NotFound,
        &page,
        None,
        true,
        None,
    )
    .await
}

/// `GET /preview/theme/{id}?path=/post/hello` — renders any site path
/// with an installed-but-not-live theme version. Never cached.
///
/// Needs a session with `ManageThemes`; anyone else gets the same 404 an
/// unknown path would, so the route does not reveal which drafts exist.
pub async fn preview_theme(
    State(state): State<AppState>,
    Path(theme_id): Path<i64>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    crate::middleware::MaybePrincipal(principal): crate::middleware::MaybePrincipal,
) -> R {
    let allowed = principal
        .as_ref()
        .is_some_and(|p| p.ensure(vyasa_core::user::Capability::ManageThemes).is_ok());
    if !allowed {
        return missing(&state).await;
    }
    let Ok(row) = state.themes.get(theme_id).await else {
        return missing(&state).await;
    };
    let path = params.get("path").map_or("/", String::as_str);
    match render_candidate(
        &state,
        &row.tokens,
        &row.layout,
        row.templates.as_ref(),
        row.assets.as_ref(),
        path,
        Some(theme_id),
    )
    .await
    {
        Ok(html) => preview_response(html),
        Err(message) => (StatusCode::UNPROCESSABLE_ENTITY, message).into_response(),
    }
}

/// A preview page: marked as such and never cached by anything in between.
#[must_use]
pub fn preview_response(html: String) -> R {
    let mut resp = Html(html).into_response();
    resp.headers_mut().insert(
        header::HeaderName::from_static("x-vyasa-preview"),
        header::HeaderValue::from_static("1"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    resp
}

/// `GET /theme-assets/images/…` and `/theme-assets/fonts/…` — a file the
/// active theme bundles. The URL names a path, not a version, so a theme
/// switch must show through: a minute of cache, then a cheap ETag
/// revalidation. SVG is a document: it gets a policy that runs no script.
const THEME_FILE_CACHE: &str = "public, max-age=60, must-revalidate";

async fn theme_file(
    state: &AppState,
    path: &str,
    req_headers: &HeaderMap,
    version: Option<i64>,
) -> R {
    let row = if let Some(id) = version {
        if !vyasa_themes::package::valid_bundled_path(path) {
            return StatusCode::NOT_FOUND.into_response();
        }
        state.themes.file(id, path).await.ok().flatten()
    } else {
        crate::theme_assets::lookup_file(state, path).await
    };
    let Some(row) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let file_cache = if version.is_some() {
        "private, no-store"
    } else {
        THEME_FILE_CACHE
    };
    let etag = format!("\"{}\"", row.sha256);
    if matches!(
        cache::evaluate_if_none_match(
            req_headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok()),
            &etag,
        ),
        cache::ConditionalGet::NotModified
    ) {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::CACHE_CONTROL, file_cache),
                (header::ETAG, etag.as_str()),
            ],
        )
            .into_response();
    }
    let mut headers = axum::http::HeaderMap::new();
    let put = |headers: &mut axum::http::HeaderMap, name: &'static str, value: String| {
        if let Ok(v) = header::HeaderValue::from_str(&value) {
            headers.insert(axum::http::HeaderName::from_static(name), v);
        }
    };
    put(&mut headers, "content-type", row.content_type.clone());
    put(&mut headers, "etag", etag);
    put(&mut headers, "cache-control", file_cache.to_owned());
    put(&mut headers, "x-content-type-options", "nosniff".to_owned());
    if row.content_type == "image/svg+xml" {
        put(
            &mut headers,
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'; sandbox".to_owned(),
        );
    }
    (StatusCode::OK, headers, row.bytes).into_response()
}

/// `GET /theme-assets/{file}` — the active theme's own CSS or JS.
///
/// The file name carries a content hash, so the URL changes whenever the
/// bytes do and the response can be cached indefinitely.
pub async fn theme_asset(
    State(state): State<AppState>,
    Path(file): Path<String>,
    req_headers: HeaderMap,
    crate::middleware::MaybePrincipal(principal): crate::middleware::MaybePrincipal,
) -> R {
    if file == "components.js" {
        return (
            [
                (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                (header::CACHE_CONTROL, "public, max-age=0, must-revalidate"),
            ],
            vyasa_themes::sections::COMPONENTS_JS,
        )
            .into_response();
    }
    if let Some(name) = file.strip_prefix("preview/") {
        let allowed = principal
            .as_ref()
            .is_some_and(|p| p.ensure(vyasa_core::user::Capability::ManageThemes).is_ok());
        if !allowed {
            return StatusCode::NOT_FOUND.into_response();
        }
        let Some(asset) = state.preview_assets.get(name) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        return (
            [
                (header::CONTENT_TYPE, asset.content_type),
                (header::CACHE_CONTROL, "private, no-store"),
            ],
            asset.body,
        )
            .into_response();
    }
    if let Some(rest) = file.strip_prefix("version/") {
        let allowed = principal
            .as_ref()
            .is_some_and(|p| p.ensure(vyasa_core::user::Capability::ManageThemes).is_ok());
        if !allowed {
            return StatusCode::NOT_FOUND.into_response();
        }
        let Some((id, path)) = rest.split_once('/') else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Ok(id) = id.parse::<i64>() else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if let Some(name) = path.strip_prefix("preview-") {
            let Some(asset) = state.preview_assets.get(name) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            return (
                [
                    (header::CONTENT_TYPE, asset.content_type),
                    (header::CACHE_CONTROL, "private, no-store"),
                ],
                asset.body,
            )
                .into_response();
        }
        return theme_file(&state, path, &req_headers, Some(id)).await;
    }
    // A path with a directory is a bundled picture or font
    // (`images/hero.jpg`); the flat form is the content-addressed
    // stylesheet or script.
    if file.contains('/') {
        return theme_file(&state, &file, &req_headers, None).await;
    }
    let Some((hash, ext)) = file.rsplit_once('.') else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(asset) = crate::theme_assets::lookup(&state, hash, ext).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, asset.content_type),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            (
                axum::http::HeaderName::from_static("x-content-type-options"),
                "nosniff",
            ),
        ],
        asset.body,
    )
        .into_response()
}
