//! Per-entry template resolution: `page-engine.html` over `page.html`, for the
//! entry whose slug is `engine` and no other. It also beats a type-named
//! template, so one product page can differ from its siblings.
#![allow(clippy::expect_used)]

use vyasa_themes::{
    builtin_registry, render_page, Layout, PageContext, RenderRequest, SiteMeta, TemplateType,
    TokenSet,
};

#[path = "support/fake_queries.rs"]
mod fake_queries;

fn render(engine: &vyasa_themes::Engine, page: &PageContext) -> String {
    futures::executor::block_on(render_page(RenderRequest {
        engine,
        layout: &Layout::default(),
        registry: &builtin_registry(),
        tokens: &TokenSet::default(),
        template: TemplateType::Page,
        site: &SiteMeta::default(),
        page,
        queries: fake_queries::FakeQueries::shared(),
        post_id: Some(1),
        content_blocks: None,
        reply_to: None,
        editor: false,
    }))
    .expect("renders")
}

fn entry(slug: &str, ty: Option<&str>) -> PageContext {
    PageContext {
        title: slug.into(),
        post_title: Some(slug.into()),
        entry_slug: Some(slug.into()),
        post_type_slug: ty.map(Into::into),
        ..PageContext::default()
    }
}

fn engine_with(files: &[(&str, &str)]) -> vyasa_themes::Engine {
    let mut engine = vyasa_themes::Engine::builtin().expect("engine");
    engine.install_theme_templates(files).expect("installs");
    engine
}

const ENGINE: &str =
    r#"{% extends "base.html" %}{% block body %}<main class="engine-page">x</main>{% endblock %}"#;
const PRODUCT: &str =
    r#"{% extends "base.html" %}{% block body %}<main class="product-page">x</main>{% endblock %}"#;

#[test]
fn a_slug_named_template_wins_for_that_entry_only() {
    let engine = engine_with(&[("page-engine.html", ENGINE)]);
    assert!(render(&engine, &entry("engine", None)).contains("engine-page"));
    assert!(!render(&engine, &entry("platform", None)).contains("engine-page"));
}

#[test]
fn the_slug_template_beats_the_type_template() {
    let engine = engine_with(&[("page-engine.html", ENGINE), ("page-product.html", PRODUCT)]);
    let html = render(&engine, &entry("engine", Some("product")));
    assert!(html.contains("engine-page"), "{html}");
    let html = render(&engine, &entry("platform", Some("product")));
    assert!(html.contains("product-page"), "{html}");
}

#[test]
fn without_an_override_the_entry_renders_as_before() {
    let engine = vyasa_themes::Engine::builtin().expect("engine");
    assert!(render(&engine, &entry("engine", None)).contains("engine"));
}
