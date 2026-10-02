//! A page composing itself, rather than being laid out by its theme.
//!
//! Until pages carried their own section tree, every page on a site had the
//! same shape: the theme's `page` template, with the author's words dropped
//! into one slot. A landing page for a single campaign — hero, features, a
//! dark call to action — was not expressible no matter how good the theme
//! was. These tests pin the seam that changed.

use vyasa_themes::{
    builtin_registry, page_sections, render_page, validate_sections, Layout, MapSettings,
    PageContext, RenderRequest, Section, SiteMeta, TemplateType, TokenSet,
};

#[path = "support/fake_queries.rs"]
mod fake_queries;

fn settings(pairs: &[(&str, serde_json::Value)]) -> MapSettings {
    let mut s = MapSettings::default();
    for (k, v) in pairs {
        s.set((*k).to_owned(), v.clone());
    }
    s
}

/// A theme that lays pages out the ordinary way: chrome, body, chrome.
fn plain_theme() -> Layout {
    let mut l = Layout::default();
    for t in TemplateType::ALL {
        *l.for_template_mut(t) = vec![
            Section::new("site-header", "header"),
            Section::new("body", "post-content"),
            Section::new("site-footer", "footer"),
        ];
    }
    l
}

/// What an author composes for one campaign page.
fn campaign() -> Vec<Section> {
    let mut hero = Section::new("lead", "hero");
    hero.settings = settings(&[
        ("headline", serde_json::json!("Ship your first site today")),
        ("subhead", serde_json::json!("No template wrangling.")),
    ]);

    let mut cta = Section::new("signup", "cta-band");
    cta.settings = settings(&[
        ("headline", serde_json::json!("Ready?")),
        ("label", serde_json::json!("Start free")),
        ("url", serde_json::json!("/signup")),
    ]);
    cta.scope = Some(vyasa_themes::StyleScope {
        bg: Some(vyasa_themes::ColorRef::Value("$text".into())),
        text: Some(vyasa_themes::ColorRef::Value("$bg".into())),
        ..Default::default()
    });

    let mut band = Section::new("closing", "band");
    band.children = vec![cta];

    // The author's own words sit between the hero and the call to action —
    // the placement the template could never offer.
    vec![hero, Section::new("words", "content"), band]
}

/// Renders one page through a plain theme. Returns the failure rather than
/// panicking so the `expect` stays in the test that cares about it.
fn render(page: &PageContext) -> Result<String, String> {
    render_with(page, false)
}

fn render_with(page: &PageContext, editor: bool) -> Result<String, String> {
    let engine = vyasa_themes::Engine::builtin().map_err(|e| e.to_string())?;
    futures::executor::block_on(render_page(RenderRequest {
        engine: &engine,
        layout: &plain_theme(),
        registry: &builtin_registry(),
        tokens: &TokenSet::default(),
        template: TemplateType::Page,
        site: &SiteMeta::default(),
        page,
        queries: fake_queries::FakeQueries::shared(),
        post_id: Some(1),
        content_blocks: None,
        reply_to: None,
        editor,
    }))
    .map_err(|e| e.to_string())
}

fn campaign_page() -> PageContext {
    PageContext {
        title: "Launch".into(),
        post_title: Some("Launch".into()),
        regions_extra: vec![
            ("body".to_owned(), "<p>Written by hand.</p>".to_owned()),
            ("content".to_owned(), "<p>Written by hand.</p>".to_owned()),
        ],
        sections: campaign(),
        ..PageContext::default()
    }
}

#[test]
fn the_pages_own_tree_replaces_the_template_body() {
    let html = render(&campaign_page()).expect("renders");

    assert!(
        html.contains("Ship your first site today"),
        "hero rendered:\n{html}"
    );
    assert!(
        !html.contains("<article class=\"vy-page\">"),
        "template body stood down for the composed one:\n{html}"
    );
}

#[test]
fn the_authors_words_land_where_the_tree_puts_them() {
    let html = render(&campaign_page()).expect("renders");

    let hero = html.find("Ship your first site").expect("hero");
    let words = html.find("Written by hand.").expect("body");
    let cta = html.find("Start free").expect("cta");
    assert!(
        hero < words && words < cta,
        "prose sits between hero and call to action:\n{html}"
    );
}

#[test]
fn theme_chrome_survives_composition() {
    // A page composing its middle must not have to re-declare the site's
    // header and footer; those still come from the theme.
    let html = render(&campaign_page()).expect("renders");
    assert!(
        html.contains("class=\"vy-header\""),
        "header still placed:\n{html}"
    );
    assert!(
        html.contains("class=\"vy-footer\""),
        "footer still placed:\n{html}"
    );
}

#[test]
fn a_page_scope_reaches_the_stylesheet() {
    let html = render(&campaign_page()).expect("renders");
    let tokens = TokenSet::default();
    assert!(
        html.contains(".vy-scope-signup"),
        "page-level scope compiled:\n{html}"
    );
    assert!(
        html.contains(&format!(
            "--vy-color-bg: {}",
            tokens.colors.text.light.normalized()
        )),
        "the dark band resolved against the palette:\n{html}"
    );
}

#[test]
fn a_scoped_band_spans_the_page_rather_than_the_reading_column() {
    // Sections render inside `.vy-main`, which is centred at the reading
    // width. A "dark band" that stops at the text column is a rectangle in
    // the middle of the page, not a band — so a scope escapes the column
    // unless its author asked for the reading width.
    let html = render(&campaign_page()).expect("renders");
    assert!(
        html.contains("vy-scope-signup vy-bleed") || html.contains("vy-bleed vy-scope-signup"),
        "the scoped section bleeds:\n{html}"
    );
    assert!(html.contains(".vy-bleed {"), "the rule exists:\n{html}");
    // The container it sits in bleeds too, so the strip runs edge to edge.
    assert!(
        html.contains("vy-band vy-bleed") || html.contains("vy-bleed vy-band"),
        "the band bleeds:\n{html}"
    );
}

#[test]
fn a_scope_can_ask_to_stay_in_the_reading_column() {
    let mut card = Section::new("aside", "cta-band");
    card.scope = Some(vyasa_themes::StyleScope {
        bg: Some(vyasa_themes::ColorRef::Value("$surface".into())),
        contained: Some(true),
        ..Default::default()
    });
    card.settings = settings(&[("headline", serde_json::json!("Still in the column"))]);
    let page = PageContext {
        title: "Launch".into(),
        sections: vec![card],
        ..PageContext::default()
    };
    let html = render(&page).expect("renders");
    // `vy-bleed` also names a stylesheet rule, so assert on the element.
    assert!(
        html.contains("class=\"vy-section vy-section--cta-band vy-scope-aside\""),
        "scoped and left in the column:\n{html}"
    );
}

#[test]
fn a_band_can_ask_to_stay_in_the_reading_column() {
    // `band` has its own `contained` setting, separate from a scope's. It
    // was validated and then ignored by the renderer; one rule now honours
    // both so they cannot disagree.
    let mut band = Section::new("quiet", "band");
    band.settings = settings(&[("contained", serde_json::json!(true))]);
    band.children = vec![Section::new("words", "content")];
    let page = PageContext {
        title: "Launch".into(),
        regions_extra: vec![("content".to_owned(), "<p>Hello.</p>".to_owned())],
        sections: vec![band],
        ..PageContext::default()
    };
    let html = render(&page).expect("renders");
    assert!(
        html.contains("class=\"vy-section vy-band\""),
        "the band kept the reading width:\n{html}"
    );
}

#[test]
fn a_page_section_cannot_take_over_a_theme_region() {
    // Both are keyed by id. If the two maps were merged, a page calling its
    // section `site-header` would replace the site's header for that page.
    let mut hijack = Section::new("site-header", "hero");
    hijack.settings = settings(&[("headline", serde_json::json!("Not the header"))]);
    let page = PageContext {
        title: "Launch".into(),
        sections: vec![hijack],
        ..PageContext::default()
    };

    let html = render(&page).expect("renders");
    assert!(
        html.contains("class=\"vy-header\""),
        "real header still there:\n{html}"
    );
    assert!(
        html.contains("Not the header"),
        "and the page's own section rendered too:\n{html}"
    );
}

#[test]
fn chrome_inside_a_composed_body_is_not_duplicated() {
    // An assistant that adds `header` to a page tree should not produce two
    // headers; the theme already places one.
    let page = PageContext {
        title: "Launch".into(),
        sections: vec![Section::new("dupe", "header")],
        ..PageContext::default()
    };
    let html = render(&page).expect("renders");
    // `vy-header` also names stylesheet rules, so count the element.
    assert_eq!(
        html.matches("class=\"vy-header\"").count(),
        1,
        "one header:\n{html}"
    );
}

#[test]
fn a_page_without_a_tree_renders_exactly_as_before() {
    let plain = PageContext {
        title: "About".into(),
        post_title: Some("About".into()),
        regions_extra: vec![("content".to_owned(), "<p>Hello.</p>".to_owned())],
        ..PageContext::default()
    };
    let html = render(&plain).expect("renders");
    assert!(
        html.contains("<article class=\"vy-page\">"),
        "template still lays the page out:\n{html}"
    );
    assert!(html.contains("Hello."));
}

#[test]
fn stored_trees_that_are_not_usable_fall_back_to_the_template() {
    // Render time is the wrong place to fail: the write path validates, and
    // a live page must not break because one row is odd.
    assert!(page_sections(None).is_empty());
    assert!(page_sections(Some(&serde_json::Value::Null)).is_empty());
    assert!(page_sections(Some(&serde_json::json!([]))).is_empty());
    assert!(page_sections(Some(&serde_json::json!({"nope": 1}))).is_empty());
    assert!(page_sections(Some(&serde_json::json!([{"id": "x"}]))).is_empty());

    let good = serde_json::json!([{"id": "lead", "kind": "hero"}]);
    assert_eq!(page_sections(Some(&good)).len(), 1);
}

#[test]
fn an_empty_tree_is_not_an_error() {
    // It is how a page says "render me through the theme's template".
    let diags = validate_sections(&[], &builtin_registry(), None, "layout");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn a_bad_tree_is_rejected_with_the_field_that_is_wrong() {
    let mut leaf = Section::new("lead", "hero");
    leaf.children = vec![Section::new("inner", "cta-band")];
    let sections = vec![
        Section::new("dupe", "cta-band"),
        Section::new("dupe", "cta-band"),
        Section::new("mystery", "no-such-kind"),
        leaf,
    ];

    let diags = validate_sections(
        &sections,
        &builtin_registry(),
        Some(&TokenSet::default()),
        "layout",
    );
    let paths: Vec<&str> = diags
        .iter()
        .filter(|d| d.is_error())
        .map(vyasa_themes::TokenDiagnostic::path)
        .collect();

    assert!(paths.contains(&"layout[1].id"), "duplicate id: {paths:?}");
    assert!(paths.contains(&"layout[2].kind"), "unknown kind: {paths:?}");
    assert!(
        paths.contains(&"layout[3].children"),
        "children on a leaf: {paths:?}"
    );
}

/// The studio's canvas needs to know which element is which setting; a
/// visitor's page must carry none of that. One flag, two guarantees.
#[test]
fn editing_markers_exist_only_in_the_studio_preview() {
    let public = render(&campaign_page()).expect("public render");
    assert!(
        !public.contains("data-vy-"),
        "a public page carries no editor markers:\n{public}"
    );
    // Selection needs no markers at all: the section wrapper has always
    // carried its id.
    assert!(
        public.contains("data-section=\"lead\""),
        "section ids are part of the ordinary output:\n{public}"
    );

    let studio = render_with(&campaign_page(), true).expect("editor render");
    assert!(
        studio.contains("data-vy-edit=\"headline\""),
        "the canvas can find the hero headline:\n{studio}"
    );
    assert!(
        studio.contains("data-vy-edit=\"subhead\""),
        "and the supporting line:\n{studio}"
    );
}

/// A section the canvas cannot see is one the author cannot select or
/// drag — which is how "some components are not drag and drop" happened:
/// chrome fills template slots with no wrapper, and an empty leaf was
/// skipped outright. On the canvas, both now exist.
#[test]
fn the_canvas_can_reach_chrome_and_empty_sections() {
    let mut page = campaign_page();
    // A bound section with nothing behind it (the fake queries return no
    // entries) — invisible to a visitor, a labelled box to the author.
    page.sections.push(Section::new("news", "latest-posts"));

    let public = render(&page).expect("public render");
    assert!(
        !public.contains("data-vy-"),
        "a visitor sees no editor scaffolding:\n{public}"
    );
    assert!(
        !public.contains("data-section=\"news\""),
        "an empty leaf stays skipped for visitors:\n{public}"
    );

    let studio = render_with(&page, true).expect("editor render");
    assert!(
        studio.contains("data-vy-empty=\"latest-posts\""),
        "an empty section is a labelled placeholder on the canvas:\n{studio}"
    );
    assert!(
        studio.contains("data-section=\"site-header\"") && studio.contains("data-vy-slot"),
        "slot-placed chrome is selectable on the canvas:\n{studio}"
    );
}

/// A template that defers to `{{ sections }}` has placed every block; the
/// loose-block rescue must stand down. Before this, a composed layout
/// rendered through such a template carried the whole page twice — once
/// from the tree, once appended as "unplaced" extras.
#[test]
fn a_sections_printing_template_renders_the_tree_exactly_once() {
    let mut layout = Layout::default();
    *layout.for_template_mut(TemplateType::Index) = campaign();

    let mut engine = vyasa_themes::Engine::builtin().expect("engine");
    engine
        .install_theme_templates(&[(
            "index.html",
            r#"{% extends "base.html" %}{% block body %}{% if sections %}{{ sections | safe }}{% else %}<p>flat</p>{% endif %}{% endblock %}"#,
        )])
        .expect("installs");

    let page = PageContext {
        title: "Home".into(),
        ..PageContext::default()
    };
    let html = futures::executor::block_on(render_page(RenderRequest {
        engine: &engine,
        layout: &layout,
        registry: &builtin_registry(),
        tokens: &TokenSet::default(),
        template: TemplateType::Index,
        site: &SiteMeta::default(),
        page: &page,
        queries: fake_queries::FakeQueries::shared(),
        post_id: None,
        content_blocks: None,
        reply_to: None,
        editor: false,
    }))
    .expect("renders");

    assert_eq!(
        html.matches("Ship your first site today").count(),
        1,
        "the hero renders exactly once:\n{html}"
    );
    assert_eq!(
        html.matches("<h2 class=\"vy-cta-title\"").count(),
        1,
        "the call to action renders exactly once:\n{html}"
    );
}
