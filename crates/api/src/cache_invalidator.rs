//! Keeps the render cache honest.
//!
//! The cache was implemented and tested but left disabled, because nothing
//! purged it when content changed — serving a stale page is worse than
//! rendering every request. The purge logic already existed in
//! `CacheService::apply_domain_event`; what was missing was something to call
//! it.
//!
//! This subscribes to the same domain event bus the search indexer uses, so
//! every publish, edit, trash, restore, delete or new comment drops the pages
//! it could have changed. Theme activation and option writes purge directly
//! at their call sites, since those are not post events.

use std::sync::Arc;

use vyasa_core::events;

use crate::cdn::Purger;
use crate::render_cache::RenderCache;
use crate::state::AppState;

/// Subscribes to domain events and purges affected pages for the life of the
/// process.
pub fn spawn(state: &AppState) {
    let Some(cache) = state.render_cache.clone() else {
        return;
    };
    let themes = state.themes.clone();
    let purger = std::sync::Arc::clone(&state.cdn);
    let posts = state.posts.clone();
    let options = state.options_service.clone();
    let mut rx = events::subscribe();

    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                // Dropped events mean we cannot know what is stale. Purging
                // everything costs a re-render; guessing costs correctness.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("cache invalidator lagged {n} events; purging everything");
                    cache.invalidate_theme();
                    purger.purge_all().await;
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            purge(&cache, &themes, &event).await;
            // The edge holds its own copy, so the same event has to reach
            // it — otherwise turning on `edge_cache_seconds` would mean a
            // correction sits behind a stale page until the TTL expires.
            // Read per event, not once: the site address is an option an
            // operator can change while the process runs.
            let base_url = options.site_url().await.ok();
            let pattern = options.permalink_pattern().await.unwrap_or_else(|_| {
                vyasa_core::options::PermalinkPattern(
                    vyasa_core::options::PermalinkPattern::DEFAULT.into(),
                )
            });
            purge_edge(&purger, &posts, &event, base_url.as_deref(), &pattern).await;
        }
    });
}

async fn purge(
    cache: &Arc<RenderCache>,
    themes: &vyasa_db::repo::themes::ThemesRepo,
    event: &events::Event,
) {
    // Cache keys are namespaced by the active theme version, so a purge needs
    // to know which namespace to sweep. If the theme cannot be read there is
    // no safe partial answer — drop everything.
    match themes.get_active().await {
        Ok(Some(active)) => {
            let namespace = crate::render_cache::theme_namespace(&active.name, active.version);
            let purged = cache.apply_domain_event(event, &namespace);
            if !purged.is_empty() {
                tracing::debug!("render cache purged {} key groups", purged.len());
            }
        }
        _ => cache.invalidate_theme(),
    }
}

/// Tells the CDN which URLs the event changed.
///
/// URLs, not cache keys: the edge knows nothing about theme namespaces.
/// A post event purges the entry itself plus the listings that show it,
/// because an edit changes a card's title and excerpt as surely as it
/// changes the page.
async fn purge_edge(
    purger: &Arc<dyn Purger>,
    posts: &vyasa_core::post::PostService,
    event: &events::Event,
    base_url: Option<&str>,
    pattern: &vyasa_core::options::PermalinkPattern,
) {
    let post_id = match event {
        events::Event::Published(e) => e.post_id,
        events::Event::Updated(e) => e.post_id,
        events::Event::Trashed(e) => e.post_id,
        events::Event::Restored(e) => e.post_id,
        events::Event::Deleted(e) => e.post_id,
        events::Event::CommentAdded(e) | events::Event::CommentApproved(e) => e.post_id,
        // An option change can alter every page, and there is no list of
        // which; the edge gets the same blunt answer the render cache does.
        events::Event::OptionChanged(_) | events::Event::MenuChanged => {
            purger.purge_all().await;
            return;
        }
    };
    let Ok(post) = posts.get(post_id).await else {
        // The row is gone (a hard delete), so its URL cannot be derived.
        // Everything is the only safe answer.
        purger.purge_all().await;
        return;
    };
    // The listings an entry appears on. Deriving the exact set would mean
    // resolving every archive it belongs to; the front page and the feeds
    // cover what changes in practice, and a stale archive page corrects
    // itself at the next event.
    let paths = [
        pattern.path_for(&post),
        "/".to_owned(),
        "/feed.xml".to_owned(),
        "/atom.xml".to_owned(),
    ];
    // Cloudflare's purge takes absolute URLs and answers a path with a
    // 4xx. These were sent as paths, so every targeted purge was refused —
    // silently, since a refusal is only logged — and a correction sat
    // behind the stale edge copy until the TTL ran out. With no site URL
    // configured there is nothing to make them absolute with, and the
    // blunt purge is the only honest one.
    match edge_urls(base_url, &paths) {
        Some(urls) => purger.purge(urls).await,
        None => purger.purge_all().await,
    }
}

/// The absolute URLs to purge, or `None` when they cannot be formed.
fn edge_urls(base_url: Option<&str>, paths: &[String]) -> Option<Vec<String>> {
    let base = base_url.map(str::trim).filter(|b| !b.is_empty())?;
    let base = base.trim_end_matches('/');
    Some(paths.iter().map(|p| format!("{base}{p}")).collect())
}

#[cfg(test)]
mod edge_url_tests {
    use super::edge_urls;

    #[test]
    fn purge_urls_are_absolute_or_nothing() {
        let paths = vec!["/post/hello".to_owned(), "/".to_owned()];
        // The shape the CDN accepts. These were sent as bare paths and
        // refused, silently, for as long as edge caching had existed.
        assert_eq!(
            edge_urls(Some("https://example.test/"), &paths).unwrap(),
            ["https://example.test/post/hello", "https://example.test/"]
        );
        // No address configured: there is nothing to make them absolute
        // with, and the caller purges everything instead.
        assert!(edge_urls(None, &paths).is_none());
        assert!(edge_urls(Some("  "), &paths).is_none());
    }
}
