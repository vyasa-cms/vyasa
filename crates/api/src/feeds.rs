//! RSS 2.0 + Atom feed generation for home and per-term archives.
//!
//! Generated with the `rss` and `atom_syndication` crates from the latest
//! [`FEED_ITEM_LIMIT`] published posts. (The phase doc named the `feed`
//! crate; that crate is an unmaintained RSS-only wrapper around `rss`, so
//! the underlying crates are used directly — deviation recorded in
//! PHASE-25.md.)

use atom_syndication as atom;
use chrono::{DateTime, Utc};
use rss::{Channel, Item};

use crate::state::AppState;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::Response;

use vyasa_common::AppError;
use vyasa_db::content_models::{PostRow, PostStatus};
use vyasa_db::repo::{PostFilter, PostsRepo};

/// Maximum posts included in any feed.
pub const FEED_ITEM_LIMIT: u32 = 50;

/// Site identity + post source for feeds.
pub struct FeedSource<'a> {
    /// Site title.
    pub site_name: &'a str,
    /// Site description/tagline.
    pub tagline: &'a str,
    /// Absolute base URL (no trailing slash), e.g. `https://blog.example`.
    pub base_url: &'a str,
    /// Configured addresses for posts.
    pub permalinks: vyasa_core::options::PermalinkPattern,
}

/// Which syndication format to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedKind {
    /// RSS 2.0 (`/feed.xml`).
    Rss,
    /// Atom (`/atom.xml`).
    Atom,
}

/// A rendered feed plus its content type.
pub struct RenderedFeed {
    /// XML document.
    pub body: String,
    /// MIME content type including charset.
    pub content_type: &'static str,
}

/// The newest published *posts*, for the home feed or one term's. Pages and custom types are not feed items: a
/// docs import writes hundreds of pages, and without this filter they filled the newest-50 window and pushed every
/// post out of both feeds. `readable_types` is passed on, but a post type filter already excludes custom types.
fn feed_filter(term_id: Option<i64>, readable_types: Option<Vec<String>>) -> PostFilter {
    PostFilter {
        status: Some(PostStatus::Published),
        post_type: Some(vyasa_db::content_models::PostType::Post),
        limit: FEED_ITEM_LIMIT,
        term_id,
        readable_types,
        ..PostFilter::default()
    }
}

/// Builds the feed for a listing.
///
/// `term` selects an optional `(taxonomy, slug, name)` for a category or tag
/// archive, with its id in `term_id`; `None` produces the home feed.
///
/// # Errors
/// Returns [`AppError::Db`] on repository failure.
pub async fn build_feed(
    posts: &vyasa_db::repo::PostsRepo,
    src: &FeedSource<'_>,
    term: Option<(&str, &str, &str)>,
    term_id: Option<i64>,
    kind: FeedKind,
    readable_types: Option<Vec<String>>,
) -> Result<RenderedFeed, AppError> {
    let rows = posts.list(&feed_filter(term_id, readable_types)).await?;
    let rendered = render_feed(src, &rows, term, kind);
    Ok(rendered)
}

/// Pure XML rendering over post rows (unit-testable without a database).
pub fn render_feed(
    src: &FeedSource<'_>,
    rows: &[PostRow],
    term: Option<(&str, &str, &str)>,
    kind: FeedKind,
) -> RenderedFeed {
    // The newest edit, not the newest publication: a post published last
    // year and corrected this morning changed the feed this morning, and
    // `rows` is ordered by publication.
    //
    // With no posts at all this used to be `Utc::now()`, which made an
    // empty feed a different document on every fetch — every reader was
    // told it had changed, every poll re-transferred it, and no validator
    // could ever match. The epoch is not informative but it is true and it
    // is stable: nothing has been published yet.
    let last_build = rows
        .iter()
        .map(|p| p.updated_at)
        .max()
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);

    let (title, self_url) = match term {
        Some((taxonomy, slug, name)) => (
            format!("{} — {name}", src.site_name),
            format!("{}/{taxonomy}/{slug}", src.base_url),
        ),
        None => (src.site_name.to_owned(), format!("{}/", src.base_url)),
    };

    // Both feeds link items through the same rule the public router uses, so
    // a link in a feed reader resolves. These used to be built as
    // "{base}/{slug}", which is the *page* route: every post link in every
    // feed 404'd, while the guid alongside it pointed at a working URL.
    let item_link = |p: &PostRow| format!("{}{}", src.base_url, src.permalinks.path_for(p));

    match kind {
        FeedKind::Rss => {
            let items: Vec<Item> = rows
                .iter()
                .map(|p| Item {
                    title: Some(p.title.clone()),
                    link: Some(item_link(p)),
                    description: Some(p.excerpt.clone().unwrap_or_default()),
                    // A permalink guid must actually resolve: the route
                    // takes a slug, not an id.
                    guid: Some(rss::Guid {
                        value: item_link(p),
                        permalink: true,
                    }),
                    pub_date: Some(p.published_at.unwrap_or(p.created_at).to_rfc2822()),
                    ..Item::default()
                })
                .collect();
            let channel = Channel {
                title,
                link: src.base_url.to_owned(),
                description: src.tagline.to_owned(),
                language: None,
                last_build_date: Some(last_build.to_rfc2822()),
                items,
                ..Channel::default()
            };
            RenderedFeed {
                body: channel.to_string(),
                content_type: "application/rss+xml; charset=utf-8",
            }
        }
        FeedKind::Atom => {
            let entries: Vec<atom::Entry> = rows
                .iter()
                .map(|p| {
                    atom::EntryBuilder::default()
                        .id(format!("{}/post/{}", src.base_url, p.id))
                        .title(atom::Text::plain(p.title.clone()))
                        .link(atom::LinkBuilder::default().href(item_link(p)).build())
                        .summary(p.excerpt.clone().map(atom::Text::plain))
                        .updated(p.published_at.unwrap_or(p.created_at))
                        .build()
                })
                .collect();
            let feed_doc = atom::FeedBuilder::default()
                .id(format!("{}/feed", src.base_url))
                .title(title)
                .links(vec![
                    atom::LinkBuilder::default().href(self_url).build(),
                    atom::LinkBuilder::default()
                        .href(format!("{}/atom.xml", src.base_url))
                        .rel("self")
                        .build(),
                ])
                .subtitle(Some(atom::Text::plain(src.tagline)))
                .updated(last_build)
                .entries(entries)
                .build();
            RenderedFeed {
                body: feed_doc.to_string(),
                content_type: "application/atom+xml; charset=utf-8",
            }
        }
    }
}

// --- Public route handlers (kept beside generation for cohesion) -------------

/// Builds an absolute origin from the request when `site_url` is unset.
///
/// A feed must carry absolute URLs, so falling back to the request's own
/// host is the only honest guess available. `X-Forwarded-Proto` is honoured
/// because the server runs behind a TLS-terminating tunnel in production,
/// where the local connection is plain http.
#[must_use]
pub fn origin_from_headers(headers: &header::HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map_or_else(
            || {
                if host.starts_with("localhost") || host.starts_with("127.0.0.1") {
                    "http"
                } else {
                    "https"
                }
            },
            |proto| {
                // The header can carry a list; the first entry is the client's.
                proto.split(',').next().unwrap_or("https").trim()
            },
        );
    format!("{scheme}://{host}")
}

/// `GET /feed.xml`.
pub async fn rss_route(
    State(state): State<AppState>,
    headers: header::HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    serve(state, &headers, FeedKind::Rss, None).await
}

/// `GET /tag/{slug}/feed.xml`: the posts filed under one tag.
pub async fn tag_rss_route(
    State(state): State<AppState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    headers: header::HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    serve(state, &headers, FeedKind::Rss, Some(("tag", slug))).await
}

/// `GET /category/{slug}/feed.xml`: the posts filed under one category.
pub async fn category_rss_route(
    State(state): State<AppState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    headers: header::HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    serve(state, &headers, FeedKind::Rss, Some(("category", slug))).await
}

/// `GET /atom.xml`.
pub async fn atom_route(
    State(state): State<AppState>,
    headers: header::HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    serve(state, &headers, FeedKind::Atom, None).await
}

async fn serve(
    state: AppState,
    headers: &header::HeaderMap,
    kind: FeedKind,
    term: Option<(&'static str, String)>,
) -> Result<Response, (StatusCode, String)> {
    // Previously hardcoded to "Vyasa" at http://localhost:8080, which meant
    // every published feed advertised the wrong name and linked every entry
    // to a host the reader cannot reach.
    let identity = state
        .options_service
        .site_identity()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let configured = state
        .options_service
        .site_url()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let base_url = if configured.is_empty() {
        origin_from_headers(headers)
    } else {
        configured
    };
    let title = if identity.title.is_empty() {
        String::from("Vyasa")
    } else {
        identity.title
    };
    let source = FeedSource {
        permalinks: crate::permalinks::pattern(&state).await,
        site_name: &title,
        tagline: &identity.tagline,
        base_url: &base_url,
    };
    let posts = PostsRepo::new(state.pool.clone());
    let readable = crate::policy::readable_custom_types(&state, None).await;
    // A term feed names the term; an unknown term is a 404, not an empty feed.
    let found = match &term {
        Some((taxonomy, slug)) => {
            let tax = vyasa_db::content_models::Taxonomy::parse(taxonomy)
                .map_err(|_| (StatusCode::NOT_FOUND, "not found".to_owned()))?;
            let row = vyasa_db::repo::TermsRepo::new(state.pool.clone())
                .get_by_slug(tax, slug)
                .await
                .map_err(|_| (StatusCode::NOT_FOUND, "not found".to_owned()))?;
            Some(row)
        }
        None => None,
    };
    let term_arg = match (&term, &found) {
        (Some((taxonomy, slug)), Some(row)) => Some((*taxonomy, slug.as_str(), row.name.as_str())),
        _ => None,
    };
    let rendered = build_feed(
        &posts,
        &source,
        term_arg,
        found.as_ref().map(|r| r.id),
        kind,
        readable,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    // `feed-item` runs on the rendered feed before delivery, which is
    // what the contract says it does; it previously ran nowhere.
    let body = crate::public::routes::filter_through_plugins(
        &state,
        vyasa_plugins::hooks::HookPoint::FeedItem,
        rendered.body,
    )
    .await;
    Ok(crate::public::cacheable::cacheable(
        headers,
        rendered.content_type,
        crate::public::cacheable::FEED_MAX_AGE,
        body,
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::pedantic, clippy::unwrap_used)]

    use super::*;

    fn src() -> FeedSource<'static> {
        FeedSource {
            permalinks: vyasa_core::options::PermalinkPattern(
                vyasa_core::options::PermalinkPattern::DEFAULT.into(),
            ),
            site_name: "Test",
            tagline: "t",
            base_url: "https://example.test",
        }
    }

    #[test]
    fn the_feed_lists_posts_only_never_pages_or_custom_types() {
        // A page or a custom-type entry newer than every post must not take a place in the newest-N window.
        for term_id in [None, Some(7)] {
            let f = feed_filter(term_id, Some(vec!["doc".to_owned()]));
            assert_eq!(f.post_type, Some(vyasa_db::content_models::PostType::Post));
            assert_eq!(f.status, Some(PostStatus::Published));
            assert_eq!(f.term_id, term_id);
        }
    }

    #[test]
    fn an_empty_feed_is_the_same_document_every_time() {
        // It used to stamp `Utc::now()`, so a site with nothing published
        // told every reader its feed had changed on every single poll, and
        // no validator could ever match.
        let a = render_feed(&src(), &[], None, FeedKind::Atom).body;
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = render_feed(&src(), &[], None, FeedKind::Atom).body;
        assert_eq!(a, b, "an empty feed changed between two renders");
    }

    #[test]
    fn the_feed_timestamp_is_the_newest_edit_not_the_newest_publication() {
        // `rows` is ordered by publication. A post published long ago and
        // corrected this morning changed the feed this morning, and taking
        // the first row's timestamp reported the correction as older than
        // it is — so readers holding a cached copy never re-fetched.
        let mut old_but_edited = row(1, "old", "old", "e", true);
        let mut recent = row(2, "recent", "recent", "e", true);
        old_but_edited.updated_at = Utc::now();
        recent.updated_at = Utc::now() - chrono::Duration::days(30);

        let newest = render_feed(
            &src(),
            &[recent, old_but_edited.clone()],
            None,
            FeedKind::Atom,
        );
        assert!(
            newest
                .body
                .contains(&old_but_edited.updated_at.to_rfc3339()),
            "feed did not report the most recent edit"
        );
    }

    fn row(id: i64, title: &str, slug: &str, excerpt: &str, published: bool) -> PostRow {
        PostRow {
            id,
            sticky: false,
            lang: String::new(),
            translation_group: None,
            post_type: vyasa_db::content_models::PostType::Post,
            status: PostStatus::Published,
            slug: slug.to_owned(),
            title: title.to_owned(),
            content: serde_json::json!([]),
            layout: None,
            excerpt: Some(excerpt.to_owned()),
            author_id: 1,
            parent_id: None,
            meta: serde_json::json!({}),
            published_at: published.then_some(Utc::now()),
            scheduled_for: None,
            password_hash: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            path: None,
            position: 0,
        }
    }

    fn source() -> FeedSource<'static> {
        FeedSource {
            permalinks: vyasa_core::options::PermalinkPattern(
                vyasa_core::options::PermalinkPattern::DEFAULT.into(),
            ),
            site_name: "Vyasa",
            tagline: "a blog",
            base_url: "http://localhost:8080",
        }
    }

    #[test]
    fn rss_feed_is_valid_xml_with_escaping_and_order() {
        let rows = vec![
            row(1, "First & foremost", "first", "excerpt <em>one</em>", true),
            row(2, "Second \"post\"", "second", "excerpt two", true),
        ];
        let rendered = render_feed(&source(), &rows, None, FeedKind::Rss);
        assert_eq!(rendered.content_type, "application/rss+xml; charset=utf-8");

        let parsed = feed_rs::parser::parse(rendered.body.as_bytes()).expect("valid RSS XML");
        assert_eq!(
            parsed
                .title
                .as_ref()
                .map(|t| t.content.clone())
                .unwrap_or_default(),
            "Vyasa"
        );
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(
            parsed.entries[0]
                .title
                .as_ref()
                .map(|t| t.content.clone())
                .unwrap_or_default(),
            "First & foremost"
        );
        // Ampersands must be escaped in the serialized document.
        assert!(rendered.body.contains("First &amp; foremost"));
        assert!(!rendered.body.contains("& "), "unescaped ampersand leaked");
    }

    #[test]
    fn atom_feed_is_valid_with_ids_and_self_link() {
        let rows = vec![
            row(2, "Second", "second", "e2", true),
            row(1, "First", "first", "e1", true),
        ];
        let rendered = render_feed(&source(), &rows, None, FeedKind::Atom);
        let parsed = feed_rs::parser::parse(rendered.body.as_bytes()).expect("valid Atom XML");
        assert_eq!(parsed.entries.len(), 2);
        assert!(rendered.body.contains(r#"rel="self""#));
        for e in &parsed.entries {
            assert!(
                e.id.starts_with("http://localhost:8080/post/"),
                "entry id {}",
                e.id
            );
        }
    }

    #[test]
    fn term_feeds_title_the_category() {
        let rendered = render_feed(
            &source(),
            &[row(5, "Rusty", "rusty", "", true)],
            Some(("category", "rust", "Rust")),
            FeedKind::Rss,
        );
        assert!(rendered.body.contains("<title>Vyasa — Rust</title>"));
    }

    #[test]
    fn empty_site_renders_a_valid_empty_feed() {
        for kind in [FeedKind::Rss, FeedKind::Atom] {
            let rendered = render_feed(&source(), &[], None, kind);
            assert!(feed_rs::parser::parse(rendered.body.as_bytes()).is_ok());
        }
    }

    #[test]
    fn forwarded_proto_wins_over_the_host_heuristic() {
        // Behind the tunnel the local hop is plain http; the client's is not.
        let mut h = header::HeaderMap::new();
        h.insert(header::HOST, "wp.example.com".parse().unwrap());
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert_eq!(origin_from_headers(&h), "https://wp.example.com");
    }

    #[test]
    fn localhost_falls_back_to_http() {
        let mut h = header::HeaderMap::new();
        h.insert(header::HOST, "localhost:8080".parse().unwrap());
        assert_eq!(origin_from_headers(&h), "http://localhost:8080");
    }

    #[test]
    fn a_proto_list_uses_the_client_hop() {
        let mut h = header::HeaderMap::new();
        h.insert(header::HOST, "wp.example.com".parse().unwrap());
        h.insert("x-forwarded-proto", "https, http".parse().unwrap());
        assert_eq!(origin_from_headers(&h), "https://wp.example.com");
    }

    #[test]
    fn a_missing_host_still_produces_a_parseable_origin() {
        let h = header::HeaderMap::new();
        assert_eq!(origin_from_headers(&h), "http://localhost");
    }

    #[test]
    fn item_links_use_the_post_route_not_the_page_route() {
        // A feed whose item links 404 is worse than no feed: a reader shows
        // the entry, the click fails, and nothing in the feed says why.
        let rows = vec![row(1, "Hello", "hello-world", "", true)];
        let src = FeedSource {
            permalinks: vyasa_core::options::PermalinkPattern(
                vyasa_core::options::PermalinkPattern::DEFAULT.into(),
            ),
            site_name: "Test",
            tagline: "",
            base_url: "https://example.test",
        };
        let rendered = render_feed(&src, &rows, None, FeedKind::Rss);
        assert!(
            rendered
                .body
                .contains("<link>https://example.test/post/hello-world</link>"),
            "got {}",
            rendered.body
        );
        assert!(
            !rendered
                .body
                .contains("<link>https://example.test/hello-world</link>"),
            "the page route must not be used for a post"
        );
    }
}
