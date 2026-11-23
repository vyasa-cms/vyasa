//! Generated social cards: which entries get one, and the `/_card/...` route.
//!
//! The card decision is two functions, [`card_scope`] and [`card_for`], and
//! both the page head and the route go through them, so the two cannot
//! disagree about which entry has a card. The drawing itself is
//! `vyasa_core::social_card`; this module adds the option, the entry rules,
//! a byte-bounded cache, an in-flight map so one key renders once, and a
//! semaphore so at most two renders run at a time whatever clients do.
//!
//! Design: `vyasa-internal specs/2026-11-22-social-card-renderer-design.md`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use vyasa_core::social_card::{clean, MAX_LABEL_CHARS, MAX_TITLE_CHARS};
use vyasa_db::content_models::{PostRow, PostStatus, PostType};

use crate::public::routes::CardSource;
use crate::state::AppState;

/// Renders allowed at once, process-wide.
const MAX_RENDERS: usize = 2;
/// Card URLs remembered for edge purges.
const MAX_RECORDED_URLS: u64 = 10_000;

/// Condition 1 held: the option is on and the active theme has a usable
/// layout. Carries that layout, so later checks read no option.
#[derive(Clone)]
pub struct CardsOn {
    source: Arc<CardSource>,
}

/// An entry's card: where it is served, its key and its alt text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// Site path of the card, `/_card/{id}-{key}.png`.
    pub url: String,
    /// The content key (16 hex characters).
    pub key: String,
    /// `og:image:alt`, from the theme's pattern.
    pub alt: String,
    /// The cleaned title drawn on the card.
    pub title: String,
    /// The cleaned label drawn above it; empty for none.
    pub label: String,
}

/// Condition 1: the `social_cards` option is on (one option read) and the
/// active theme's layout loaded. `None` otherwise, also on a failed read:
/// a card is never worth a failed page.
pub async fn cards_enabled(state: &AppState) -> Option<CardsOn> {
    if !state.options_service.social_cards().await.unwrap_or(false) {
        return None;
    }
    match crate::public::routes::active_card(state).await {
        Ok(Some(Ok(source))) => Some(CardsOn { source }),
        _ => None,
    }
}

/// Conditions 2 to 4, from the loaded row and the plugin surface only: the
/// type is in the layout's scope; the entry is published, has no password
/// (an unlocked one included) and its type is publicly openable; it has no
/// image of its own.
pub async fn card_scope(on: &CardsOn, state: &AppState, post: &PostRow) -> bool {
    on.source.theme.covers(post.post_type.as_str())
        && post.status == PostStatus::Published
        && post.password_hash.is_none()
        && crate::public::page_meta::own_image(post).is_none()
        && crate::policy::publicly_openable(&state.plugin_surface, &state.pool, post.post_type)
            .await
}

/// The card title: `og_title`, else the title, each cleaned; empty when
/// both clean to nothing. `seo_title` is not used: the card shows the
/// page's own name.
#[must_use]
pub fn card_title(post: &PostRow) -> String {
    let og = post
        .meta
        .get("og_title")
        .and_then(serde_json::Value::as_str)
        .map(|t| clean(t, MAX_TITLE_CHARS))
        .unwrap_or_default();
    if og.is_empty() {
        clean(&post.title, MAX_TITLE_CHARS)
    } else {
        og
    }
}

/// The card label: the top ancestor's title when the trail has one (it
/// ends with the entry itself), else the post type's plural label.
async fn card_label(state: &AppState, post: &PostRow, trail: &[(String, String)]) -> String {
    if trail.len() > 1 {
        return clean(&trail[0].0, MAX_LABEL_CHARS);
    }
    let plural = match post.post_type {
        PostType::Post => "Posts".to_owned(),
        PostType::Page => "Pages".to_owned(),
        PostType::Block => "Blocks".to_owned(),
        PostType::Custom(slug) => {
            crate::entry_fields::served_type(&state.plugin_surface, &state.pool, slug)
                .await
                .map_or_else(|| slug.to_owned(), |t| t.plural)
        }
    };
    clean(&plural, MAX_LABEL_CHARS)
}

/// The entry's card, or `None` unless all five conditions hold. `trail` is
/// `breadcrumbs_for`'s answer; `site` is the site title for the alt text.
pub async fn card_for(
    on: &CardsOn,
    state: &AppState,
    post: &PostRow,
    trail: &[(String, String)],
    site: &str,
) -> Option<Card> {
    if !card_scope(on, state, post).await {
        return None;
    }
    let title = card_title(post);
    let theme = &on.source.theme;
    // A layout without a label box draws none, so the label takes no part
    // in the text check or the key.
    let label = if theme.layout().label.is_some() {
        card_label(state, post, trail).await
    } else {
        String::new()
    };
    if title.is_empty() || !theme.supports(&title, &label) {
        return None;
    }
    let key = theme.key(
        &on.source.theme_name,
        on.source.theme_version,
        &title,
        &label,
    );
    Some(Card {
        url: card_path(post.id, &key),
        alt: theme.layout().alt_text(&title, &label, site),
        key,
        title,
        label,
    })
}

/// `/_card/{id}-{key}.png`.
#[must_use]
pub fn card_path(id: i64, key: &str) -> String {
    format!("/_card/{id}-{key}.png")
}

/// `{id}-{key}.png` back into its parts; `None` for anything else.
fn parse_file(file: &str) -> Option<(i64, &str)> {
    let stem = file.strip_suffix(".png")?;
    let (digits, key) = stem.split_once('-')?;
    let id: i64 = digits.parse().ok().filter(|n| *n > 0)?;
    // Canonical digits only, so `+5` or `05` are not a second address for
    // entry 5.
    if id.to_string() != digits {
        return None;
    }
    let valid_key = key.len() == 16
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    valid_key.then_some((id, key))
}

type RenderResult = Option<Result<Arc<Vec<u8>>, ()>>;

/// The process-wide card cache, the in-flight renders and the render
/// permits.
pub struct SocialCards {
    cache: moka::sync::Cache<String, Arc<Vec<u8>>>,
    /// Card path → (entry id, post type), for edge purges.
    served: moka::sync::Cache<String, (i64, &'static str)>,
    in_flight: Mutex<HashMap<String, tokio::sync::watch::Receiver<RenderResult>>>,
    permits: Arc<tokio::sync::Semaphore>,
}

impl std::fmt::Debug for SocialCards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocialCards")
            .field("cached", &self.cache.entry_count())
            .finish_non_exhaustive()
    }
}

impl SocialCards {
    /// A cache bounded by `max_bytes` of PNG.
    #[must_use]
    pub fn new(max_bytes: u64) -> Self {
        Self {
            cache: moka::sync::Cache::builder()
                .weigher(|_k: &String, v: &Arc<Vec<u8>>| u32::try_from(v.len()).unwrap_or(u32::MAX))
                .max_capacity(max_bytes)
                .build(),
            served: moka::sync::Cache::new(MAX_RECORDED_URLS),
            in_flight: Mutex::new(HashMap::new()),
            permits: Arc::new(tokio::sync::Semaphore::new(MAX_RENDERS)),
        }
    }

    /// Takes the served card paths of entry `id`, or of every entry of
    /// `post_type` when the type is hierarchical (a child's label is its
    /// top ancestor's title), and drops their cached PNGs.
    pub fn take_urls(&self, id: i64, post_type: PostType) -> Vec<String> {
        let all_of_type = post_type.is_hierarchical();
        let name = post_type.as_str();
        let paths: Vec<String> = self
            .served
            .iter()
            .filter(|(_, (entry, ty))| *entry == id || (all_of_type && *ty == name))
            .map(|(path, _)| path.as_ref().clone())
            .collect();
        for path in &paths {
            self.served.invalidate(path);
            if let Some(key) = path
                .strip_prefix("/_card/")
                .and_then(|f| f.strip_suffix(".png"))
                .and_then(|s| s.split_once('-'))
                .map(|(_, k)| k)
            {
                self.cache.invalidate(key);
            }
        }
        paths
    }

    /// The PNG for `key`: cached, joined to a render in flight, or a new
    /// render by `render`. The render runs in a task of its own, so a
    /// dropped request neither cancels it nor frees its permit early.
    async fn png<F>(self: &Arc<Self>, key: &str, render: F) -> Result<Arc<Vec<u8>>, ()>
    where
        F: FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
    {
        if let Some(png) = self.cache.get(key) {
            return Ok(png);
        }
        let mut rx = {
            let mut flight = self.in_flight.lock().map_err(|_| ())?;
            // Checked again under the lock: a render may have finished
            // between the first look and here.
            if let Some(png) = self.cache.get(key) {
                return Ok(png);
            }
            if let Some(rx) = flight.get(key) {
                rx.clone()
            } else {
                let (tx, rx) = tokio::sync::watch::channel(None);
                flight.insert(key.to_owned(), rx.clone());
                self.spawn_render(tx, key.to_owned(), render);
                rx
            }
        };
        let done = rx.wait_for(Option::is_some).await.map_err(|_| ())?;
        done.clone().unwrap_or(Err(()))
    }

    fn spawn_render<F>(
        self: &Arc<Self>,
        tx: tokio::sync::watch::Sender<RenderResult>,
        key: String,
        render: F,
    ) where
        F: FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
    {
        let cards = Arc::clone(self);
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let result = match Arc::clone(&cards.permits).acquire_owned().await {
                Ok(permit) => {
                    let queued = started.elapsed();
                    let rendered = tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let t = std::time::Instant::now();
                        render().map(|png| (png, t.elapsed()))
                    })
                    .await;
                    match rendered {
                        Ok(Ok((png, took))) => {
                            tracing::debug!(
                                "social card {key}: queued {queued:?}, rendered {took:?}"
                            );
                            Ok(Arc::new(png))
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("social card {key} failed: {e}");
                            Err(())
                        }
                        Err(e) => {
                            tracing::error!("social card {key} panicked: {e}");
                            Err(())
                        }
                    }
                }
                Err(_) => Err(()),
            };
            // Cache first, then leave the map, then wake the waiters: a
            // request arriving in between finds one or the other.
            if let Ok(png) = &result {
                cards.cache.insert(key.clone(), Arc::clone(png));
            }
            if let Ok(mut flight) = cards.in_flight.lock() {
                flight.remove(&key);
            }
            let _ = tx.send(Some(result));
        });
    }
}

fn status(code: StatusCode) -> Response {
    code.into_response()
}

/// `GET /_card/{id}-{key}.png`: 200 with the PNG for the current key, 304
/// on a matching `If-None-Match`, 302 to the current URL for an old key,
/// 404 when the entry has no card. The order of work keeps the cost of a
/// miss low: the path is parsed before any query, the option before the
/// entry lookup, the scope before the breadcrumb lookups.
pub async fn card_route(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some((id, asked)) = parse_file(&file) else {
        return status(StatusCode::NOT_FOUND);
    };
    let Some(on) = cards_enabled(&state).await else {
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(post) = state.posts.get(id).await else {
        return status(StatusCode::NOT_FOUND);
    };
    if !card_scope(&on, &state, &post).await {
        return status(StatusCode::NOT_FOUND);
    }
    let site = crate::public::page_meta::SiteContext::load(
        &state,
        &crate::feeds::origin_from_headers(&headers),
    )
    .await;
    let path = site.permalinks.path_for(&post);
    let trail = crate::public::routes::breadcrumbs_for(&state, &site, &post, &path).await;
    let Some(card) = card_for(&on, &state, &post, &trail, &site.title).await else {
        return status(StatusCode::NOT_FOUND);
    };
    if card.key != asked {
        return (
            StatusCode::FOUND,
            [
                (header::LOCATION, card.url.clone()),
                (header::CACHE_CONTROL, "public, max-age=300".to_owned()),
            ],
        )
            .into_response();
    }
    let etag = format!("\"{}\"", card.key);
    let immutable = "public, max-age=31536000, s-maxage=86400, immutable";
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"))
    {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, immutable.to_owned()),
            ],
        )
            .into_response();
    }
    let source = Arc::clone(&on.source);
    let (title, label) = (card.title, card.label);
    let Ok(png) = state
        .social_cards
        .png(&card.key, move || source.theme.render(&title, &label))
        .await
    else {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    };
    state
        .social_cards
        .served
        .insert(card.url.clone(), (post.id, post.post_type.as_str()));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png".to_owned()),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, immutable.to_owned()),
        ],
        png.as_ref().clone(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{parse_file, SocialCards, MAX_RENDERS};

    const KEY: &str = "0123456789abcdef";

    /// A render that counts its calls and blocks until `gate` opens.
    fn gated(
        calls: &Arc<AtomicUsize>,
        gate: &Arc<std::sync::Barrier>,
    ) -> impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static {
        let (calls, gate) = (Arc::clone(calls), Arc::clone(gate));
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            gate.wait();
            Ok(vec![1, 2, 3])
        }
    }

    async fn until(mut check: impl FnMut() -> bool) {
        for _ in 0..500 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition not reached");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_concurrent_misses_render_once() {
        let cards = Arc::new(SocialCards::new(1 << 20));
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(std::sync::Barrier::new(2));
        let a = tokio::spawn({
            let (cards, r) = (Arc::clone(&cards), gated(&calls, &gate));
            async move { cards.png(KEY, r).await }
        });
        until(|| calls.load(Ordering::SeqCst) == 1).await;
        let b = tokio::spawn({
            let (cards, r) = (Arc::clone(&cards), gated(&calls, &gate));
            async move { cards.png(KEY, r).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        gate.wait();
        assert_eq!(*a.await.unwrap().unwrap(), vec![1, 2, 3]);
        assert_eq!(*b.await.unwrap().unwrap(), vec![1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one render");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_request_still_caches_the_card() {
        let cards = Arc::new(SocialCards::new(1 << 20));
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(std::sync::Barrier::new(2));
        let first = tokio::spawn({
            let (cards, r) = (Arc::clone(&cards), gated(&calls, &gate));
            async move { cards.png(KEY, r).await }
        });
        until(|| calls.load(Ordering::SeqCst) == 1).await;
        first.abort();
        let _ = first.await;
        let second = tokio::spawn({
            let (cards, r) = (Arc::clone(&cards), gated(&calls, &gate));
            async move { cards.png(KEY, r).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        gate.wait();
        assert_eq!(*second.await.unwrap().unwrap(), vec![1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "joined the running render");
        assert!(cards.cache.get(KEY).is_some(), "cached");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_third_render_waits_for_a_permit_even_when_requests_drop() {
        assert_eq!(MAX_RENDERS, 2);
        let cards = Arc::new(SocialCards::new(1 << 20));
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(std::sync::Barrier::new(3));
        let keys: [&'static str; 3] = ["000000000000000a", "000000000000000b", "000000000000000c"];
        let mut tasks = Vec::new();
        for key in keys.into_iter().take(2) {
            let (cards, r) = (Arc::clone(&cards), gated(&calls, &gate));
            tasks.push(tokio::spawn(async move { cards.png(key, r).await }));
        }
        until(|| calls.load(Ordering::SeqCst) == 2).await;
        for t in tasks.drain(..) {
            t.abort();
        }
        let third = tokio::spawn({
            let (cards, r) = (Arc::clone(&cards), gated(&calls, &gate));
            async move { cards.png(keys[2], r).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "no third render yet");
        gate.wait();
        // The two finished; the third now runs and waits on its own gate.
        until(|| calls.load(Ordering::SeqCst) == 3).await;
        // Open the third render's gate with two more parties.
        let g1 = Arc::clone(&gate);
        let g2 = Arc::clone(&gate);
        let h1 = std::thread::spawn(move || g1.wait());
        let h2 = std::thread::spawn(move || g2.wait());
        assert_eq!(*third.await.unwrap().unwrap(), vec![1, 2, 3]);
        h1.join().unwrap();
        h2.join().unwrap();
        for key in keys {
            assert!(cards.cache.get(key).is_some(), "{key} cached");
        }
    }

    #[tokio::test]
    async fn a_failed_render_is_not_cached() {
        let cards = Arc::new(SocialCards::new(1 << 20));
        assert!(cards.png(KEY, || Err("boom".to_owned())).await.is_err());
        assert!(cards.cache.get(KEY).is_none());
        assert_eq!(*cards.png(KEY, || Ok(vec![9])).await.unwrap(), vec![9]);
    }

    #[test]
    fn card_paths_parse_strictly() {
        assert_eq!(
            parse_file("42-0123456789abcdef.png"),
            Some((42, "0123456789abcdef"))
        );
        for bad in [
            "42-0123456789abcdef",
            "42-0123456789ABCDEF.png",
            "42-0123456789abcde.png",
            "042-0123456789abcdef.png",
            "+42-0123456789abcdef.png",
            "0-0123456789abcdef.png",
            "-1-0123456789abcdef.png",
            "x-0123456789abcdef.png",
            "42_0123456789abcdef.png",
            "",
        ] {
            assert_eq!(parse_file(bad), None, "{bad}");
        }
    }
}
