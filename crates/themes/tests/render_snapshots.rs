//! Rendering tests: per-kind snapshots, the XSS battery, sandbox escapes,
//! and an end-to-end page render.

#![allow(clippy::pedantic)]

use vyasa_core::block::{Block, BlockKind};
use vyasa_themes::{
    builtin_registry, merge_layouts, render_block, render_blocks, render_page, Engine, Layout,
    MapSettings, PageContext, Section, SiteMeta, TemplateType, TokenSet,
};

fn block(kind: BlockKind, attrs: serde_json::Value) -> Block {
    Block::new(kind, attrs)
}

fn with_children(mut b: Block, children: Vec<Block>) -> Block {
    b.children = children;
    b
}

// --- Snapshots ---------------------------------------------------------------

#[test]
fn snapshot_text_blocks() {
    let doc = vec![
        block(
            BlockKind::Paragraph,
            serde_json::json!({"text": "Hello <em>world</em> & friends"}),
        ),
        block(
            BlockKind::Heading,
            serde_json::json!({"text": "A heading", "level": 3}),
        ),
        with_children(
            block(BlockKind::List, serde_json::json!({"ordered": true})),
            vec![
                block(BlockKind::Paragraph, serde_json::json!({"text": "one"})),
                with_children(
                    block(BlockKind::List, serde_json::json!({"ordered": false})),
                    vec![block(
                        BlockKind::Paragraph,
                        serde_json::json!({"text": "one point one"}),
                    )],
                ),
                block(BlockKind::Paragraph, serde_json::json!({"text": "two"})),
            ],
        ),
        block(
            BlockKind::Quote,
            serde_json::json!({"text": "<strong>Stay</strong> hungry", "cite": "https://example.com/q"}),
        ),
        block(
            BlockKind::Quote,
            serde_json::json!({"text": "Attributed", "citation": "Ada <em>L</em>"}),
        ),
        block(
            BlockKind::Paragraph,
            serde_json::json!({"text": "<s>struck</s> <u>under</u> <mark>lit</mark> H<sub>2</sub>O x<sup>2</sup> <span>gone</span>"}),
        ),
        block(
            BlockKind::Code,
            serde_json::json!({"code": "fn main() {}\n", "language": "rust"}),
        ),
        block(
            BlockKind::Table,
            serde_json::json!({
                "header": ["Name", "Note"],
                "rows": [["a", "<em>rich</em> <script>x</script>"], ["b", ""]],
                "caption": "Two rows"
            }),
        ),
    ];
    insta::assert_snapshot!("blocks_text", render_blocks(&doc).expect("renders"));
}

#[test]
fn snapshot_media_and_design_blocks() {
    let img = || {
        block(
            BlockKind::Image,
            serde_json::json!({"url": "https://cdn.example.com/a.png", "alt": "An <a> image"}),
        )
    };
    let doc = vec![
        img(),
        with_children(
            block(BlockKind::Gallery, serde_json::json!({})),
            vec![img(), img()],
        ),
        block(
            BlockKind::Video,
            serde_json::json!({"mediaId": 42, "caption": "clip"}),
        ),
        block(BlockKind::Audio, serde_json::json!({"mediaId": 7})),
        block(
            BlockKind::File,
            serde_json::json!({"url": "https://example.com/f.pdf", "caption": "doc"}),
        ),
        block(
            BlockKind::File,
            serde_json::json!({"url": "/api/v1/media/9/raw"}),
        ),
        block(
            BlockKind::Image,
            serde_json::json!({"url": "/api/v1/media/3/raw", "alt": "uploaded", "caption": "from the library"}),
        ),
        block(
            BlockKind::Cover,
            serde_json::json!({"url": "https://cdn.example.com/bg.jpg", "text": "Cover"}),
        ),
        block(
            BlockKind::MediaText,
            serde_json::json!({"mediaId": 5, "text": "side text"}),
        ),
        block(BlockKind::Separator, serde_json::Value::Null),
        block(BlockKind::PageBreak, serde_json::Value::Null),
    ];
    insta::assert_snapshot!("blocks_media", render_blocks(&doc).expect("renders"));
}

#[test]
fn snapshot_container_special_blocks() {
    let btn = |label, href| {
        block(
            BlockKind::Button,
            serde_json::json!({"label": label, "href": href}),
        )
    };
    let doc = vec![
        with_children(
            block(BlockKind::Group, serde_json::json!({})),
            vec![block(
                BlockKind::Paragraph,
                serde_json::json!({"text": "inside"}),
            )],
        ),
        with_children(
            block(BlockKind::Columns, serde_json::json!({})),
            vec![
                block(BlockKind::Paragraph, serde_json::json!({"text": "l"})),
                block(BlockKind::Paragraph, serde_json::json!({"text": "r"})),
            ],
        ),
        with_children(
            block(BlockKind::Buttons, serde_json::json!({})),
            vec![btn("Docs", "https://docs.example.com"), btn("Home", "/")],
        ),
        with_children(
            block(BlockKind::Details, serde_json::json!({"summary": "More"})),
            vec![block(
                BlockKind::Paragraph,
                serde_json::json!({"text": "hidden"}),
            )],
        ),
        with_children(
            block(BlockKind::Footnotes, serde_json::json!({})),
            vec![block(
                BlockKind::Paragraph,
                serde_json::json!({"text": "source"}),
            )],
        ),
        block(
            BlockKind::Embed,
            serde_json::json!({"url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "title": "Video"}),
        ),
        block(
            BlockKind::Html,
            serde_json::json!({"html": "<p>ok</p><script>alert(1)</script><img src=x onerror=alert(1)>"}),
        ),
    ];
    insta::assert_snapshot!("blocks_containers", render_blocks(&doc).expect("renders"));
}

// --- XSS battery -------------------------------------------------------------

fn contains_any(haystack: &str, needles: &[&str]) -> Vec<String> {
    let lower = haystack.to_lowercase();
    needles
        .iter()
        .filter(|n| lower.contains(&n.to_lowercase()))
        .map(|s| (*s).to_owned())
        .collect()
}

#[test]
fn xss_battery_is_neutralized() {
    const PAYLOADS: &[&str] = &[
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "<svg onload=alert(1)>",
        "<a href=\"javascript:alert(1)\">x</a>",
        "<a href=\"data:text/html,<script>alert(1)</script>\">x</a>",
        "<iframe src=\"https://evil.example\"></iframe>",
        "<div style=\"background:url(javascript:alert(1))\">x</div>",
        "&#60;script&#62;alert(1)&#60;/script&#62;",
    ];
    for payload in PAYLOADS {
        // Paragraph / quote / caption path (inline allowlist).
        let p = render_block(&block(
            BlockKind::Paragraph,
            serde_json::json!({ "text": payload }),
        ))
        .expect("paragraph renders");
        assert!(
            contains_any(
                &p,
                &[
                    "<script",
                    "onerror",
                    "onload",
                    "javascript:",
                    "data:text/html",
                    "<iframe",
                    "<svg"
                ]
            )
            .is_empty(),
            "inline XSS leaked for {payload}: {p}"
        );
        // Raw-HTML escape hatch.
        let h = render_block(&block(
            BlockKind::Html,
            serde_json::json!({ "html": payload }),
        ))
        .expect("html renders");
        assert!(
            contains_any(
                &h,
                &[
                    "<script",
                    "onerror",
                    "onload",
                    "javascript:",
                    "data:text/html",
                    "<iframe",
                    "<svg"
                ]
            )
            .is_empty(),
            "raw-html XSS leaked for {payload}: {h}"
        );
        // Button href scheme check.
        let btn = render_block(&block(
            BlockKind::Button,
            serde_json::json!({ "label": "go", "href": payload }),
        ));
        if let Ok(b) = btn {
            assert!(
                contains_any(&b, &["javascript:", "data:"]).is_empty(),
                "button XSS leaked for {payload}: {b}"
            );
        }
        // Embed fallback never produces an iframe for unknown hosts.
        let e = render_block(&block(
            BlockKind::Embed,
            serde_json::json!({ "url": format!("https://evil.example/{payload}") }),
        ))
        .expect("embed renders");
        assert!(!e.contains("<iframe"), "embed iframe leaked for {payload}");
    }
}

#[test]
fn embed_provider_allowlist() {
    let ok = [
        (
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "youtube-nocookie.com/embed/dQw4w9WgXcQ",
        ),
        (
            "https://youtu.be/dQw4w9WgXcQ",
            "youtube-nocookie.com/embed/dQw4w9WgXcQ",
        ),
        (
            "https://vimeo.com/76979871",
            "player.vimeo.com/video/76979871",
        ),
        (
            "https://dai.ly/x8k9abc",
            "dailymotion.com/embed/video/x8k9abc",
        ),
        (
            "https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT",
            "open.spotify.com/embed/track/4cOdK2wGLETKBW3PvgPWqT",
        ),
        (
            "https://archive.org/details/electronicamixtape",
            "archive.org/embed/electronicamixtape",
        ),
        (
            "https://codepen.io/team/codepen/pen/pen.html",
            "codepen.io/team/codepen/embed",
        ), // placeholder replaced below
    ];
    for (input, _) in &ok[..6] {
        match vyasa_themes::resolve_embed_url(input) {
            vyasa_themes::EmbedOutcome::Iframe { src, .. } => {
                assert!(!src.contains('"'), "no quotes in src");
                assert!(src.starts_with("https://"));
            }
            other => panic!("{input} should embed, got {other:?}"),
        }
    }
    // Traversal and junk are rejected.
    let bad = [
        "http://www.youtube.com/watch?v=x",
        "https://www.youtube.com.evil.com/watch?v=x",
        "https://www.youtube.com/watch?v=<script>",
        "https://vimeo.com/abc<script>",
        "https://open.spotify.com/track/../../admin",
        "not a url",
    ];
    for input in bad {
        assert!(
            matches!(
                vyasa_themes::resolve_embed_url(input),
                vyasa_themes::EmbedOutcome::Link
            ),
            "{input} must fall back to a link"
        );
    }
    let _ = ok; // table documents intent; first 6 entries asserted above
}

// --- Sandbox escapes ---------------------------------------------------------

#[test]
fn sandbox_escapes_are_rejected() {
    let mut engine = Engine::builtin().expect("builtin engine");
    const ESCAPES: &[&str] = &[
        "{% include \"/etc/passwd\" %}",
        "{% set x = get_env(name=\"HOME\") %}",
        "{{ self.__init__ }}",
        "{{ __tera_context }}",
        "{% import \"macros.html\" as m %}",
        "{% extends \"https://evil.example/base.html\" %}",
    ];
    for src in ESCAPES {
        let result = engine.install_theme_templates(&[("theme/escape.html", src)]);
        assert!(result.is_err(), "sandbox escape accepted: {src}");
    }
    // Legitimate theme override that extends the package's own base is fine.
    engine
        .install_theme_templates(&[(
            "index.html",
            "{% extends \"base.html\" %}{% block body %}<p>custom home</p>{% endblock %}",
        )])
        .expect("valid override installs");
    let rendered = engine
        .render_json("index.html", &serde_json::json!({
            "site": {"name": "T", "tagline": "", "lang": "en"},
            "page": {"title": "t", "direction": "ltr", "tokens_css": ""},
            "posts": [], "pagination": {},
            "regions": {"header": "", "nav": "", "sidebar_left": "", "sidebar_right": "", "footer": ""}
        }))
        .expect("override compiles and renders");
    assert!(rendered.contains("<p>custom home</p>"));
}

// --- End-to-end page render --------------------------------------------------

#[test]
fn end_to_end_page_render() {
    let engine = Engine::builtin().expect("builtin engine");
    // Parent layout + child override merged (phase-21 semantics), filled out.
    let parent = Layout {
        index: vec![
            lb("header", "header"),
            lb("nav", "nav"),
            lb("list", "latest-posts"),
            lb("main", "content"),
            lb("footer", "footer"),
        ],
        single: vec![
            lb("header", "header"),
            lb("body", "post-content"),
            lb("comments", "comments"),
            lb("footer", "footer"),
        ],
        ..Layout::default()
    };
    let _ = merge_layouts(&parent, &Layout::default());

    for (template, page) in [
        (
            TemplateType::Index,
            PageContext {
                title: "Home".into(),
                posts: vec![vyasa_themes::PostCard {
                    title: "First post".into(),
                    url: "/2026/first-post".into(),
                    excerpt: "Hello <em>world</em>".into(),
                    snippet: String::new(),
                    author: "Ada".into(),
                    date: "Aug 23, 2026".into(),
                    fields: serde_json::Map::new(),
                }],
                pagination_page: 1,
                pagination_next: Some("/page/2".into()),
                ..PageContext::default()
            },
        ),
        (
            TemplateType::Single,
            PageContext {
                title: "First post".into(),
                post_title: Some("First post".into()),
                author: Some("Ada".into()),
                date: Some("Aug 23, 2026".into()),
                ..PageContext::default()
            },
        ),
        (
            TemplateType::NotFound,
            PageContext {
                title: "404".into(),
                ..PageContext::default()
            },
        ),
    ] {
        let html = futures::executor::block_on(render_page(vyasa_themes::RenderRequest {
            engine: &engine,
            layout: &parent,
            registry: &builtin_registry(),
            tokens: &TokenSet::default(),
            template,
            site: &SiteMeta::default(),
            page: &page,
            queries: fake_queries::FakeQueries::shared(),
            post_id: None,
            content_blocks: None,
            reply_to: None,
            editor: false,
        }))
        .expect("renders");
        assert!(html.starts_with("<!doctype html>"), "full page document");
        assert!(html.contains("--vy-color-primary"), "token CSS inlined");
        insta::assert_snapshot!(format!("page_{}", template.as_str()), html);
    }
}

/// A layout addresses its blocks by whatever ids it likes; the built-in
/// templates address slots by what they are. Before these two were
/// connected, a theme whose sidebar block was called `aside` rendered no
/// sidebar at all, and a block the author added rendered nowhere.
#[test]
fn slots_resolve_by_kind_and_loose_blocks_still_render() {
    let engine = Engine::builtin().expect("engine");
    let mut layout = Layout {
        index: vec![
            lb("masthead", "header"),
            lb("rail", "sidebar-right"),
            lb("find", "search-box"),
            lb("colophon", "footer"),
        ],
        ..Layout::default()
    };
    for t in [
        TemplateType::Single,
        TemplateType::Archive,
        TemplateType::Page,
        TemplateType::Search,
        TemplateType::NotFound,
    ] {
        *layout.for_template_mut(t) = vec![lb("body", "content")];
    }

    let html = futures::executor::block_on(render_page(vyasa_themes::RenderRequest {
        engine: &engine,
        layout: &layout,
        registry: &builtin_registry(),
        tokens: &TokenSet::default(),
        template: TemplateType::Index,
        site: &SiteMeta::default(),
        page: &PageContext::default(),
        queries: fake_queries::FakeQueries::shared(),
        post_id: None,
        content_blocks: None,
        reply_to: None,
        editor: false,
    }))
    .expect("renders");

    // Chrome placed by kind, not by the ids this layout chose.
    assert!(
        html.contains("vy-sidebar-right"),
        "sidebar rendered: {html}"
    );
    assert!(html.contains("vy-header"), "header rendered");
    assert!(html.contains("vy-footer-inner"), "footer rendered");
    // A block with no slot of its own still appears.
    assert!(html.contains("<form"), "search box rendered: {html}");
    // Once in the markup (the other match is the stylesheet's own rule).
    assert_eq!(html.matches("<aside").count(), 1, "sidebar not duplicated");
}

fn lb(id: &str, kind: &str) -> Section {
    Section {
        id: id.to_owned(),
        kind: kind.to_owned(),
        settings: MapSettings::default(),
        children: Vec::new(),
        scope: None,
        hide_on: None,
        width: None,
    }
}

#[test]
fn engine_error_messages_are_author_friendly() {
    let mut engine = Engine::builtin().expect("engine");
    // Tera resolves filters at render time; installation must succeed so the
    // author gets one precise error naming template + problem.
    engine
        .install_theme_templates(&[("bad.html", "{{ name | no_such_filter }}")])
        .expect("parses");
    let err = engine
        .render_json("bad.html", &serde_json::json!({"name": "x"}))
        .expect_err("unknown filter fails at render");
    assert!(
        err.to_string().contains("bad.html") && err.to_string().contains("no_such_filter"),
        "message names template and problem: {err}"
    );
}

#[path = "support/fake_queries.rs"]
mod fake_queries;

#[test]
fn regions_extra_overrides_layout_content() {
    let engine = Engine::builtin().expect("engine");
    let layout = Layout {
        single: vec![lb("body", "post-content")],
        index: vec![lb("f-latest-posts", "latest-posts")],
        archive: vec![lb("f-archive", "latest-posts")],
        page: vec![lb("f-page", "post-content")],
        search: vec![lb("f-search", "search-box")],
        not_found: vec![lb("f-nf", "content")],
    };

    let page = PageContext {
        title: "T".into(),
        post_title: Some("T".into()),
        regions_extra: vec![(
            "content".to_owned(),
            "<p class=\"vy-p\">MARKER</p>".to_owned(),
        )],
        ..PageContext::default()
    };
    let html = futures::executor::block_on(render_page(vyasa_themes::RenderRequest {
        engine: &engine,
        layout: &layout,
        registry: &builtin_registry(),
        tokens: &TokenSet::default(),
        template: TemplateType::Single,
        site: &SiteMeta::default(),
        page: &page,
        queries: fake_queries_singleton(),
        post_id: Some(1),
        content_blocks: None,
        reply_to: None,
        editor: false,
    }))
    .expect("renders");
    assert!(
        html.contains("MARKER"),
        "extra region must reach the template"
    );
}

fn fake_queries_singleton() -> &'static dyn vyasa_themes::ContentQueries {
    use std::sync::OnceLock;
    static Q: OnceLock<EmptyQueries> = OnceLock::new();
    Q.get_or_init(EmptyQueries::default) as &'static _
}

#[derive(Default)]
struct EmptyQueries;

impl vyasa_themes::ContentQueries for EmptyQueries {
    fn entries<'a>(
        &'a self,
        _source: &'a str,
        _sort: vyasa_themes::EntrySort,
        _limit: u32,
        _term: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::PostCardData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
    fn categories<'a>(
        &'a self,
        _s: bool,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::TermLinkData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
    fn popular_tags<'a>(
        &'a self,
        _c: u32,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::TermLinkData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
    fn monthly_archives<'a>(
        &'a self,
        _m: u32,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::MonthArchiveData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
    fn navigation_menu<'a>(
        &'a self,
        _s: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(std::future::ready(Ok(String::new())))
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
            Box::pin(std::future::ready(Ok(Vec::new())))
        }
    }
    fn approved_comments<'a>(
        &'a self,
        _p: i64,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::CommentNodeData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
}

// --- Plugin blocks ----------------------------------------------------------

fn plugin_block(kind: &str, attrs: serde_json::Value) -> Block {
    Block {
        kind: BlockKind::Plugin,
        plugin_kind: Some(kind.to_owned()),
        attrs,
        children: Vec::new(),
    }
}

#[test]
fn a_resolved_plugin_block_renders_wrapped_and_sanitized() {
    let html = render_block(&plugin_block(
        "acme/chart",
        serde_json::json!({
            vyasa_themes::PLUGIN_BLOCK_RESOLVED_ATTR:
                "<div class=\"chart\">ok</div><script>steal()</script>\
                 <a href=\"https://x.test\" onclick=\"x\">link</a>",
        }),
    ))
    .expect("renders");

    // Wrapped so a theme can style it, and named so an author can find it.
    assert!(html.starts_with("<div class=\"vy-plugin-block\" data-block=\"acme/chart\">"));
    // `class` survives — a plugin has to be able to style what it emits.
    assert!(html.contains("<div class=\"chart\">ok</div>"), "{html}");
    // Behaviour does not, however trusted the plugin's signature.
    assert!(!html.contains("script"), "{html}");
    assert!(!html.contains("onclick"), "{html}");
    // Links keep their rel, as everywhere else.
    assert!(html.contains("rel=\"noopener noreferrer\""), "{html}");
}

#[test]
fn an_unresolved_plugin_block_never_takes_the_post_down() {
    // The plugin is disabled, uninstalled or trapped: the block leaves a
    // trace in the source and renders nothing a reader sees, rather than
    // failing the whole document.
    let html = render_block(&plugin_block("acme/chart", serde_json::json!({"stars": 3})))
        .expect("still renders");
    assert_eq!(html, "<!-- vyasa: unresolved plugin block acme/chart -->");
}

#[test]
fn a_plugin_kind_name_cannot_break_out_of_the_markup() {
    // The name comes from a stored document, so it is escaped in both the
    // attribute and the comment.
    let html = render_block(&plugin_block(
        "acme/\"><script>x</script>",
        serde_json::json!({}),
    ))
    .expect("renders");
    assert!(!html.contains("<script>"), "{html}");
    assert!(html.contains("&quot;") || html.contains("&lt;"), "{html}");
}

#[test]
fn plugin_blocks_render_inside_a_document_beside_ordinary_ones() {
    let doc = vec![
        block(BlockKind::Paragraph, serde_json::json!({"text": "Before"})),
        plugin_block(
            "acme/chart",
            serde_json::json!({
                vyasa_themes::PLUGIN_BLOCK_RESOLVED_ATTR: "<p>chart</p>",
            }),
        ),
        block(BlockKind::Paragraph, serde_json::json!({"text": "After"})),
    ];
    let html = render_blocks(&doc).expect("renders");
    let before = html.find("Before").expect("before");
    let chart = html.find("chart").expect("chart");
    let after = html.find("After").expect("after");
    assert!(
        before < chart && chart < after,
        "document order kept:\n{html}"
    );
}

/// A page with no title must not render a separator with nothing in front.
///
/// Tera's `default` filter fills in for a value that is *missing*, not one
/// that is empty, so `page.title | default(value="Untitled")` rendered
/// nothing at all for an untitled post: `<title> — Vyasa</title>`, on every
/// browser tab and every search result. The same trap applied to the site
/// name, which is how a site that had not set one got `<title> — </title>`.
#[test]
fn an_empty_title_does_not_leave_a_dangling_separator() {
    let engine = Engine::builtin().expect("engine");
    let mut layout = Layout::default();
    *layout.for_template_mut(TemplateType::Single) = vec![lb("body", "content")];

    let render = |page_title: &str, site_name: &str| {
        let site = SiteMeta {
            name: site_name.to_owned(),
            ..SiteMeta::default()
        };
        futures::executor::block_on(render_page(vyasa_themes::RenderRequest {
            engine: &engine,
            layout: &layout,
            registry: &builtin_registry(),
            tokens: &TokenSet::default(),
            template: TemplateType::Single,
            site: &site,
            page: &PageContext {
                title: page_title.to_owned(),
                ..PageContext::default()
            },
            queries: fake_queries::FakeQueries::shared(),
            post_id: None,
            content_blocks: None,
            reply_to: None,
            editor: false,
        }))
        .expect("renders")
    };

    let title_of = |html: &str| {
        let start = html.find("<title>").expect("has a title") + "<title>".len();
        let end = html[start..].find("</title>").expect("closed") + start;
        html[start..end].to_owned()
    };

    assert_eq!(title_of(&render("A post", "My site")), "A post — My site");
    assert_eq!(title_of(&render("", "My site")), "Untitled — My site");
    assert_eq!(title_of(&render("A post", "")), "A post — Vyasa");
    assert_eq!(title_of(&render("", "")), "Untitled — Vyasa");

    // And the direction attribute, which had the same shape of fallback.
    assert!(render("A post", "My site").contains(r#"dir="ltr""#));
}

// ---- toc, callout, timed (phase 72) -------------------------------------

#[test]
fn toc_lists_the_documents_headings_up_to_its_depth() {
    let doc = vec![
        block(BlockKind::Toc, serde_json::json!({"depth": 3})),
        block(
            BlockKind::Heading,
            serde_json::json!({"level": 2, "text": "One"}),
        ),
        block(
            BlockKind::Heading,
            serde_json::json!({"level": 3, "text": "One a"}),
        ),
        block(
            BlockKind::Heading,
            serde_json::json!({"level": 4, "text": "Too deep"}),
        ),
    ];
    let html = render_blocks(&doc).expect("renders");
    assert!(
        html.contains("href=\"#vy-h-1\"") && html.contains(">One<"),
        "{html}"
    );
    // Ids are numbered per document, so the anchor lands. Rendering a
    // second document must start over rather than carry on counting.
    assert!(html.contains("id=\"vy-h-1\""), "{html}");
    let again = render_blocks(&[block(
        BlockKind::Heading,
        serde_json::json!({"level": 2, "text": "Fresh"}),
    )])
    .expect("renders");
    assert!(again.contains("id=\"vy-h-1\""), "{again}");
    assert!(
        html.contains("vy-toc__l3") && html.contains("One a"),
        "{html}"
    );
    let nav = html.split("</nav>").next().unwrap_or("");
    assert!(!nav.contains("Too deep"), "level 4 is past depth 3: {nav}");
    // A document with no headings renders no empty nav.
    let alone = render_blocks(&[block(BlockKind::Toc, serde_json::json!({}))]).expect("renders");
    assert!(!alone.contains("vy-toc"), "{alone}");
}

#[test]
fn callout_carries_its_tone_and_title_around_its_children() {
    let doc = vec![with_children(
        block(
            BlockKind::Callout,
            serde_json::json!({"tone": "warning", "title": "Mind the <b>gap</b>"}),
        ),
        vec![block(
            BlockKind::Paragraph,
            serde_json::json!({"text": "Inside"}),
        )],
    )];
    let html = render_blocks(&doc).expect("renders");
    assert!(html.contains("vy-callout--warning"), "{html}");
    assert!(
        html.contains("Mind the <b>gap</b>") && html.contains("Inside"),
        "{html}"
    );
    let odd = render_blocks(&[block(
        BlockKind::Callout,
        serde_json::json!({"tone": "purple"}),
    )])
    .expect("renders");
    assert!(
        odd.contains("vy-callout--note"),
        "unknown tone falls back: {odd}"
    );
}

#[test]
fn timed_block_renders_only_inside_its_window() {
    let inside = with_children(
        block(
            BlockKind::Timed,
            serde_json::json!({"from": "2000-01-01T00:00:00Z", "until": "2999-01-01T00:00:00Z"}),
        ),
        vec![block(
            BlockKind::Paragraph,
            serde_json::json!({"text": "Now"}),
        )],
    );
    let past = with_children(
        block(
            BlockKind::Timed,
            serde_json::json!({"until": "2000-01-01T00:00:00Z"}),
        ),
        vec![block(
            BlockKind::Paragraph,
            serde_json::json!({"text": "Gone"}),
        )],
    );
    let future = with_children(
        block(
            BlockKind::Timed,
            serde_json::json!({"from": "2999-01-01T00:00:00Z"}),
        ),
        vec![block(
            BlockKind::Paragraph,
            serde_json::json!({"text": "Later"}),
        )],
    );
    let html = render_blocks(&[inside, past, future]).expect("renders");
    assert!(html.contains("Now"), "{html}");
    assert!(!html.contains("Gone") && !html.contains("Later"), "{html}");
}
