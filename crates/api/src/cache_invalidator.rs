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
/// process. Runs with or without the render cache: the edge and the social
/// card URLs need purging either way, and starting it always means turning
/// cards on needs no restart.
pub fn spawn(state: &AppState) {
    let cache = state.render_cache.clone();
    let cards = std::sync::Arc::clone(&state.social_cards);
    let themes = state.themes.clone();
    let purger = std::sync::Arc::clone(&state.cdn);
    let posts = state.posts.clone();
    let queries = std::sync::Arc::clone(&state.content_queries);
    let options = state.options_service.clone();
    let repo = vyasa_db::repo::PostsRepo::new(state.pool.clone());
    let mut rx = events::subscribe();

    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                // Dropped events mean we cannot know what is stale. Purging
                // everything costs a re-render; guessing costs correctness.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("cache invalidator lagged {n} events; purging everything");
                    if let Some(cache) = &cache {
                        cache.invalidate_theme();
                    }
                    purger.purge_all().await;
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            let linked = match post_id_of(&event) {
                Some(id) => linked_entries(&repo, id).await,
                None => Some(Vec::new()),
            };
            if let Some(cache) = &cache {
                purge(
                    cache,
                    &themes,
                    &posts,
                    &event,
                    queries.as_ref(),
                    linked.as_deref(),
                )
                .await;
            }
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
            purge_edge(
                &purger,
                &posts,
                &cards,
                &event,
                base_url.as_deref(),
                &pattern,
                linked.as_deref().unwrap_or_default(),
            )
            .await;
        }
    });
}

/// The entry an event is about, when it is about a post.
fn post_id_of(event: &events::Event) -> Option<i64> {
    match event {
        events::Event::Published(e) => Some(e.post_id),
        events::Event::Updated(e) => Some(e.post_id),
        events::Event::Trashed(e) => Some(e.post_id),
        events::Event::Restored(e) => Some(e.post_id),
        events::Event::Deleted(e) => Some(e.post_id),
        _ => None,
    }
}

/// Whether an event drops every cached single page: any post event on an
/// entry of a hierarchical type, because every page of such a type shows
/// the tree (publishing one doc must refresh the sidebar of all others).
/// Comment events keep the handling `apply_domain_event` gives them.
fn purges_singles(hierarchical: bool, event: &events::Event) -> bool {
    hierarchical && post_id_of(event).is_some()
}

/// The entries whose pages show entry `id` as a neighbour or a related
/// entry: its newer and older neighbours and its related entries (the
/// relation is symmetric enough that these are the pages naming it). Empty
/// for a type whose pages show no such links; `None` when the entry cannot
/// be read (a hard delete), so the caller cannot know which pages name it.
async fn linked_entries(
    repo: &vyasa_db::repo::PostsRepo,
    id: i64,
) -> Option<Vec<vyasa_db::content_models::PostRow>> {
    let post = repo.get(id).await.ok()?;
    if !crate::public::routes::shows_reading_links(post.post_type) {
        return Some(Vec::new());
    }
    let (newer, older) = repo.neighbours(&post).await.ok()?;
    let related = repo
        .related_by_terms(&post, crate::public::routes::RELATED_ON_ENTRY)
        .await
        .ok()?;
    Some(newer.into_iter().chain(older).chain(related).collect())
}

async fn purge(
    cache: &Arc<RenderCache>,
    themes: &vyasa_db::repo::themes::ThemesRepo,
    posts: &vyasa_core::post::PostService,
    event: &events::Event,
    queries: &dyn vyasa_themes::ContentQueries,
    linked: Option<&[vyasa_db::content_models::PostRow]>,
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
            // The pages linking to the entry as a neighbour or a related
            // entry show its title and address; when they cannot be named,
            // every entry page goes.
            match linked {
                Some(rows) => {
                    for row in rows {
                        cache
                            .invalidate_entry(&namespace, row.path.as_deref().unwrap_or(&row.slug));
                    }
                }
                None => {
                    cache.invalidate_entries(&namespace);
                }
            }
            // A tree changes every page that shows it, not only the
            // entry's own.
            if let Some(id) = post_id_of(event) {
                if posts
                    .get(id)
                    .await
                    .is_ok_and(|p| purges_singles(p.post_type.is_hierarchical(), event))
                {
                    cache.invalidate_entries(&namespace);
                    queries.forget_trees();
                }
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
    cards: &crate::social_card::SocialCards,
    event: &events::Event,
    base_url: Option<&str>,
    pattern: &vyasa_core::options::PermalinkPattern,
    linked: &[vyasa_db::content_models::PostRow],
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
    purge_entry_edge(purger, cards, event, &post, base_url, pattern, linked).await;
}

/// [`purge_edge`] for an event about `post`, once its row is loaded.
async fn purge_entry_edge(
    purger: &Arc<dyn Purger>,
    cards: &crate::social_card::SocialCards,
    event: &events::Event,
    post: &vyasa_db::content_models::PostRow,
    base_url: Option<&str>,
    pattern: &vyasa_core::options::PermalinkPattern,
    linked: &[vyasa_db::content_models::PostRow],
) {
    // The listings an entry appears on. Deriving the exact set would mean
    // resolving every archive it belongs to; the front page and the feeds
    // cover what changes in practice, and a stale archive page corrects
    // itself at the next event.
    // Plus the pages that link to it as a neighbour or a related entry.
    let mut paths: Vec<String> = [
        pattern.path_for(post),
        "/".to_owned(),
        "/feed.xml".to_owned(),
        "/atom.xml".to_owned(),
    ]
    .into_iter()
    .chain(linked.iter().map(|row| pattern.path_for(row)))
    .collect();
    // The entry's served social cards (all of its type's, for a tree: a
    // child's label is its top ancestor's title). A comment changes no card.
    if post_id_of(event).is_some() {
        paths.extend(cards.take_urls(post.id, post.post_type));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Purger` that records what it was asked to drop.
    #[derive(Default)]
    struct Recording {
        urls: std::sync::Mutex<Vec<String>>,
        all: std::sync::atomic::AtomicBool,
    }

    impl Purger for Recording {
        fn purge<'a>(
            &'a self,
            urls: Vec<String>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            self.urls.lock().unwrap().extend(urls);
            Box::pin(async {})
        }

        fn purge_all<'a>(
            &'a self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            self.all.store(true, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    fn row(id: i64) -> vyasa_db::content_models::PostRow {
        let now = chrono::Utc::now();
        vyasa_db::content_models::PostRow {
            id,
            sticky: false,
            lang: String::new(),
            translation_group: None,
            post_type: vyasa_db::content_models::PostType::Post,
            status: vyasa_db::content_models::PostStatus::Published,
            slug: "hello".to_owned(),
            title: "Hello".to_owned(),
            content: serde_json::json!([]),
            layout: None,
            excerpt: None,
            author_id: 1,
            parent_id: None,
            meta: serde_json::json!({}),
            published_at: Some(now),
            scheduled_for: None,
            password_hash: None,
            created_at: now,
            updated_at: now,
            path: None,
            position: 0,
        }
    }

    /// Runs `purge_entry_edge` for `event` on entry 7, which has a served
    /// card; returns the purged URLs and the cards it leaves.
    async fn edge_purge_of(
        event: &events::Event,
    ) -> (Vec<String>, crate::social_card::SocialCards) {
        let recording = Arc::new(Recording::default());
        let purger: Arc<dyn Purger> = recording.clone();
        let cards = crate::social_card::SocialCards::new(1 << 20);
        cards.record_served_for_test(7, "post", "0123456789abcdef");
        let pattern = vyasa_core::options::PermalinkPattern(
            vyasa_core::options::PermalinkPattern::DEFAULT.into(),
        );
        purge_entry_edge(
            &purger,
            &cards,
            event,
            &row(7),
            Some("https://site/"),
            &pattern,
            &[],
        )
        .await;
        assert!(!recording.all.load(std::sync::atomic::Ordering::SeqCst));
        let urls = recording.urls.lock().unwrap().clone();
        (urls, cards)
    }

    #[tokio::test]
    async fn an_entry_change_purges_its_card_url_at_the_edge() {
        let (urls, cards) =
            edge_purge_of(&events::Event::Updated(events::PostUpdated { post_id: 7 })).await;
        assert!(
            urls.contains(&"https://site/_card/7-0123456789abcdef.png".to_owned()),
            "{urls:?}"
        );
        assert!(cards
            .take_urls(7, vyasa_db::content_models::PostType::Post)
            .is_empty());
    }

    #[tokio::test]
    async fn a_comment_leaves_the_card_alone() {
        let comment = events::Event::CommentAdded(events::CommentAdded {
            comment_id: 1,
            post_id: 7,
        });
        let (urls, cards) = edge_purge_of(&comment).await;
        assert!(!urls.iter().any(|u| u.contains("/_card/")), "{urls:?}");
        assert_eq!(
            cards
                .take_urls(7, vyasa_db::content_models::PostType::Post)
                .len(),
            1
        );
    }

    fn published(id: i64) -> events::Event {
        events::Event::Published(events::PostPublished {
            post_id: id,
            author_id: 1,
        })
    }

    #[test]
    fn publishing_a_doc_refreshes_other_docs_pages() {
        assert!(purges_singles(true, &published(1)));
        assert!(purges_singles(
            true,
            &events::Event::Updated(events::PostUpdated { post_id: 1 })
        ));
        assert!(purges_singles(
            true,
            &events::Event::Trashed(events::PostTrashed { post_id: 1 })
        ));
    }

    #[test]
    fn publishing_a_post_does_not_purge_singles() {
        assert!(
            !purges_singles(false, &published(1)),
            "a blog must not start re-rendering every page on publish"
        );
    }
}
