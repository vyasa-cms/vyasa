//! Resolver output snapshots driven by an in-memory ContentQueries fake.

#![allow(clippy::pedantic)]

use std::future::ready;

use vyasa_themes::{
    builtin_registry, CommentNodeData, ContentQueries, MonthArchiveData, PostCardData,
    ResolveContext, TermLinkData,
};

#[derive(Clone, Default)]
struct FakeQueries {
    posts: Vec<PostCardData>,
    categories: Vec<TermLinkData>,
    tags: Vec<TermLinkData>,
    months: Vec<MonthArchiveData>,
}

impl ContentQueries for FakeQueries {
    fn entries<'a>(
        &'a self,
        source: &'a str,
        _sort: vyasa_themes::EntrySort,
        limit: u32,
        _term: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<PostCardData>, String>> + Send + 'a>,
    > {
        // The fake serves any source from the same list; the api impl owns
        // the real per-type behaviour.
        let _ = source;
        let items: Vec<PostCardData> = self.posts.iter().take(limit as usize).cloned().collect();
        Box::pin(ready(Ok(items)))
    }

    fn categories<'a>(
        &'a self,
        show_counts: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<TermLinkData>, String>> + Send + 'a>,
    > {
        let items: Vec<TermLinkData> = self
            .categories
            .iter()
            .map(|t| TermLinkData {
                count: if show_counts { t.count } else { None },
                ..t.clone()
            })
            .collect();
        Box::pin(ready(Ok(items)))
    }

    fn popular_tags<'a>(
        &'a self,
        count: u32,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<TermLinkData>, String>> + Send + 'a>,
    > {
        let items: Vec<TermLinkData> = self.tags.iter().take(count as usize).cloned().collect();
        Box::pin(ready(Ok(items)))
    }

    fn monthly_archives<'a>(
        &'a self,
        months: u32,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<MonthArchiveData>, String>> + Send + 'a>,
    > {
        let items: Vec<MonthArchiveData> =
            self.months.iter().take(months as usize).cloned().collect();
        Box::pin(ready(Ok(items)))
    }

    fn navigation_menu<'a>(
        &'a self,
        slug: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        let html = if slug == "main" {
            "<ul class=\"vy-menu\"><li><a href=\"/\">Home</a></li>\
             <li><a href=\"/category/rust\">Rust</a></li></ul>"
                .to_owned()
        } else {
            String::new()
        };
        Box::pin(ready(Ok(html)))
    }

    fn page_tree<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::PageNodeData>, String>>
                + Send
                + 'a,
        >,
    > {
        {
            Box::pin(ready(Ok(Vec::new())))
        }
    }
    fn approved_comments<'a>(
        &'a self,
        post_id: i64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<CommentNodeData>, String>> + Send + 'a>,
    > {
        let tree = if post_id == 1 {
            vec![CommentNodeData {
                id: 10,
                author: "Ada".into(),
                date: "Aug 23, 2026".into(),
                html: "<p>Nice <strong>post</strong>!</p>".into(),
                children: vec![CommentNodeData {
                    id: 11,
                    author: "Linus".into(),
                    date: "Aug 23, 2026".into(),
                    html: "<p>Talk is cheap.</p>".into(),
                    children: Vec::new(),
                }],
            }]
        } else {
            Vec::new()
        };
        Box::pin(ready(Ok(tree)))
    }
}

fn settings(entries: &[(&str, serde_json::Value)]) -> vyasa_themes::MapSettings {
    let mut m = vyasa_themes::MapSettings::default();
    for (k, v) in entries {
        m.set((*k).to_owned(), v.clone());
    }
    m
}

fn resolve_now<'a>(
    kind: &str,
    ctx: &'a ResolveContext<'a>,
    s: &'a vyasa_themes::MapSettings,
) -> Option<Result<vyasa_themes::BlockPayload, String>> {
    let spec = builtin_registry().get(kind)?;
    Some(futures::executor::block_on(spec.resolve(ctx, s)))
}

fn payload_html(result: Result<vyasa_themes::BlockPayload, String>) -> String {
    match result {
        Ok(vyasa_themes::BlockPayload::Html(html)) => html,
        other => format!("{other:?}"),
    }
}

fn fake() -> FakeQueries {
    FakeQueries {
        posts: vec![
            PostCardData {
                id: 1,
                title: "Hello & welcome".into(),
                url: "/hello-welcome".into(),
                excerpt: "First \"post\"".into(),
                thumb_url: Some("https://cdn.example.com/t.jpg".into()),
                thumb_focal: None,
                blurhash: Some("LEHV6nWB2yk8pyo0adR*.7kCMdnj".into()),
                date: "Aug 23, 2026".into(),
                author: "Ada".into(),
            },
            PostCardData {
                id: 2,
                title: "Second".into(),
                url: "/second".into(),
                excerpt: String::new(),
                thumb_url: None,
                thumb_focal: None,
                blurhash: None,
                date: "Aug 22, 2026".into(),
                author: "Ada".into(),
            },
        ],
        categories: vec![
            TermLinkData {
                name: "Rust".into(),
                url: "/category/rust".into(),
                count: Some(4),
            },
            TermLinkData {
                name: "Ops".into(),
                url: "/category/ops".into(),
                count: Some(1),
            },
        ],
        tags: vec![
            TermLinkData {
                name: "tokio".into(),
                url: "/tag/tokio".into(),
                count: Some(9),
            },
            TermLinkData {
                name: "axum".into(),
                url: "/tag/axum".into(),
                count: Some(3),
            },
        ],
        months: vec![
            MonthArchiveData {
                label: "August 2026".into(),
                url: "/08-2026/".into(),
                count: 3,
            },
            MonthArchiveData {
                label: "July 2026".into(),
                url: "/07-2026/".into(),
                count: 5,
            },
        ],
    }
}

#[test]
fn snapshot_resolvers() {
    let q = fake();
    let ctx = ResolveContext::new(&q, None);

    let reg = builtin_registry();
    let out =
        |kind: &str, s: &vyasa_themes::MapSettings| -> Result<vyasa_themes::BlockPayload, String> {
            let spec = reg.get(kind).expect("registered");
            futures::executor::block_on(spec.resolve(&ctx, s))
        };

    let mut text = String::new();
    for (kind, s) in [
        ("latest-posts", settings(&[("count", serde_json::json!(2))])),
        (
            "categories-list",
            settings(&[("show_counts", serde_json::json!(true))]),
        ),
        ("tag-cloud", settings(&[("count", serde_json::json!(2))])),
        ("archives", settings(&[])),
        (
            "search-box",
            settings(&[("placeholder", serde_json::json!("Search…"))]),
        ),
        ("menu", settings(&[("slug", serde_json::json!("main"))])),
        ("breadcrumbs", settings(&[])),
    ] {
        text.push_str(&format!(
            "<!-- {kind} -->\n{}\n",
            payload_html(out(kind, &s))
        ));
    }
    insta::assert_snapshot!("resolver_outputs", text);
}

#[test]
fn collection_renders_bound_cards_and_survives_a_dead_source() {
    // The generic bound section: same cards as latest-posts, any source.
    let q = fake();
    let ctx = ResolveContext::new(&q, None);
    let spec = builtin_registry().get("collection").expect("registered");

    let s = settings(&[
        (
            "bind",
            serde_json::json!({"source": "book", "sort": "title", "limit": 1}),
        ),
        ("columns", serde_json::json!(2)),
        ("heading", serde_json::json!("From the shelf")),
    ]);
    let html = match futures::executor::block_on(spec.resolve(&ctx, &s)) {
        Ok(vyasa_themes::BlockPayload::Html(html)) => html,
        other => panic!("expected html, got {other:?}"),
    };
    assert!(html.contains("vy-collection"), "{html}");
    assert!(html.contains("--vy-cols:2"), "{html}");
    assert!(html.contains("From the shelf"), "{html}");
    assert!(html.contains("vy-post-card"), "{html}");

    // A binding whose source vanished (its plugin was disabled) renders as
    // nothing — the page survives, like stored entries do.
    let empty = FakeQueries::default();
    let ctx = ResolveContext::new(&empty, None);
    let result = futures::executor::block_on(spec.resolve(&ctx, &s)).expect("resolves");
    assert!(matches!(result, vyasa_themes::BlockPayload::Empty));

    // And a broken bind object never takes the page down either.
    let broken = settings(&[("bind", serde_json::json!("not an object"))]);
    let q = fake();
    let ctx = ResolveContext::new(&q, None);
    let result = futures::executor::block_on(spec.resolve(&ctx, &broken)).expect("resolves");
    assert!(matches!(result, vyasa_themes::BlockPayload::Empty));
}

#[test]
fn comments_resolver_renders_thread_and_escapes_meta() {
    let q = fake();
    let ctx = ResolveContext::new(&q, Some(1));
    let comments_settings = settings(&[]);
    let spec = builtin_registry().get("comments").expect("registered");
    let html = match futures::executor::block_on(spec.resolve(&ctx, &comments_settings)) {
        Ok(vyasa_themes::BlockPayload::Html(html)) => html,
        other => panic!("expected html, got {other:?}"),
    };
    assert!(html.contains("id=\"comment-10\""));
    assert!(
        html.contains("<strong>post</strong>"),
        "sanitized body kept"
    );
    assert!(
        html.contains("vy-comment-children"),
        "nested replies render"
    );
    insta::assert_snapshot!("resolver_comments", html);
}

#[test]
fn resolvers_return_empty_without_data() {
    let empty = FakeQueries::default();
    let ctx = ResolveContext::new(&empty, None);
    for kind in ["latest-posts", "categories-list", "archives"] {
        let result = resolve_now(kind, &ctx, &settings(&[])).expect("known");
        assert!(
            matches!(result, Ok(vyasa_themes::BlockPayload::Empty)),
            "{kind} must be Empty without data"
        );
    }
    // comments without a post context are empty too
    let result = resolve_now("comments", &ctx, &settings(&[])).expect("known");
    assert!(matches!(result, Ok(vyasa_themes::BlockPayload::Empty)));
}

#[test]
fn a_card_thumbnail_carries_its_focal_point_and_nothing_else() {
    use vyasa_themes::PostCardData;
    let card = |focal: Option<&str>| PostCardData {
        id: 1,
        title: "T".into(),
        url: "/post/t".into(),
        excerpt: String::new(),
        thumb_url: Some("/api/v1/media/1/raw?variant=medium".into()),
        thumb_focal: focal.map(str::to_owned),
        blurhash: Some("LEHV6nWB2yk8".into()),
        date: "today".into(),
        author: "A".into(),
    };
    let html = vyasa_themes::card_html_for_test(&card(Some("30% 70%")));
    assert!(html.contains("style=\"object-position:30% 70%\""), "{html}");
    let unsafe_html = vyasa_themes::card_html_for_test(&card(Some("30% 70%\" onload=\"x")));
    assert!(!unsafe_html.contains("onload"), "{unsafe_html}");
    assert!(!vyasa_themes::card_html_for_test(&card(None)).contains("object-position"));
}
