//! An entry template sees the entry's headings as `toc`, so a theme can draw
//! an "on this page" list in its own markup (a post rail, a docs sidebar)
//! instead of only through the `toc` block inside the content.
#![allow(clippy::expect_used)]

use vyasa_core::BlockDocument;
use vyasa_themes::{
    builtin_registry, render_page, Layout, PageContext, RenderRequest, SiteMeta, TemplateType,
    TokenSet,
};

#[path = "support/fake_queries.rs"]
mod fake_queries;

const SINGLE: &str = r#"{% extends "base.html" %}{% block body %}<main>{% if toc %}<ol class="otp">{% for h in toc %}<li data-l="{{ h.level }}"><a href="{{ h.anchor }}">{{ h.text }}</a></li>{% endfor %}</ol>{% else %}<p class="no-toc"></p>{% endif %}</main>{% endblock %}"#;

fn doc() -> BlockDocument {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "blocks": [
            {"kind": "paragraph", "attrs": {"text": "Intro"}},
            {"kind": "heading", "attrs": {"level": 2, "text": "Install"}},
            {"kind": "group", "attrs": {}, "children": [
                {"kind": "heading", "attrs": {"level": 3, "text": "From PyPI"}}
            ]},
            {"kind": "heading", "attrs": {"level": 2, "text": "Run & check"}}
        ]
    }))
    .expect("document")
}

fn render(blocks: Option<&[vyasa_core::block::Block]>) -> String {
    let mut engine = vyasa_themes::Engine::builtin().expect("engine");
    engine
        .install_theme_templates(&[("single.html", SINGLE)])
        .expect("installs");
    let page = PageContext {
        title: "Post".into(),
        post_title: Some("Post".into()),
        ..PageContext::default()
    };
    futures::executor::block_on(render_page(RenderRequest {
        engine: &engine,
        layout: &Layout::default(),
        registry: &builtin_registry(),
        tokens: &TokenSet::default(),
        template: TemplateType::Single,
        site: &SiteMeta::default(),
        page: &page,
        queries: fake_queries::FakeQueries::shared(),
        post_id: Some(1),
        content_blocks: blocks,
        reply_to: None,
        editor: false,
    }))
    .expect("renders")
}

#[test]
fn an_entry_template_lists_the_headings_with_their_anchors() {
    let document = doc();
    let html = render(Some(&document.blocks));
    assert!(
        html.contains(
            r##"<ol class="otp"><li data-l="2"><a href="#vy-h-1">Install</a></li><li data-l="3"><a href="#vy-h-2">From PyPI</a></li><li data-l="2"><a href="#vy-h-3">Run &amp; check</a></li></ol>"##
        ),
        "{html}"
    );
}

#[test]
fn a_page_without_entry_content_has_an_empty_toc() {
    let html = render(None);
    assert!(html.contains(r#"<p class="no-toc"></p>"#), "{html}");
}
