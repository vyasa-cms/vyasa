//! `SiteMeta::base_css_href`: the shared rules move to a linked file.
#![allow(clippy::expect_used)]

use vyasa_themes::{
    builtin_registry, render_page, Engine, Layout, PageContext, RenderRequest, SiteMeta,
    TemplateType, TokenSet,
};

#[path = "support/fake_queries.rs"]
mod fake_queries;

fn render(href: Option<&str>) -> String {
    let engine = Engine::builtin().expect("builtin engine");
    let layout = Layout::default();
    futures::executor::block_on(render_page(RenderRequest {
        engine: &engine,
        layout: &layout,
        registry: &builtin_registry(),
        tokens: &TokenSet::default(),
        template: TemplateType::Page,
        site: &SiteMeta {
            base_css_href: href.map(str::to_owned),
            ..SiteMeta::default()
        },
        page: &PageContext::default(),
        queries: fake_queries::FakeQueries::shared(),
        post_id: Some(1),
        content_blocks: None,
        reply_to: None,
        editor: false,
    }))
    .expect("renders")
}

#[test]
fn without_a_href_the_base_rules_stay_inline() {
    let html = render(None);
    assert!(html.contains("Vyasa base stylesheet"), "{html}");
    assert!(!html.contains("shared.css"), "{html}");
}

#[test]
fn with_a_href_the_page_links_the_file_and_drops_the_rules() {
    let html = render(Some("/theme-assets/shared.css?v=abc"));
    assert!(
        html.contains("<link rel=\"stylesheet\" href=\"/theme-assets/shared.css?v=abc\">"),
        "{html}"
    );
    assert!(!html.contains("Vyasa base stylesheet"), "{html}");
    assert!(html.len() < 20_000, "page is {} bytes", html.len());
    assert!(vyasa_themes::shared_css(&TokenSet::default()).contains("Vyasa base stylesheet"));
}
