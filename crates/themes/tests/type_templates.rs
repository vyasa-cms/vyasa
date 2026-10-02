//! Per-type template resolution: `single-book.html` over `single.html`.
//!
//! The WordPress-style hierarchy, one level deep: a theme that ships a
//! template named for a post type gets it used for that type's entries and
//! archives, and a theme that does not renders exactly as before. This is
//! what "design one product page, get one per product" is made of — the
//! studio edits a template file, and every entry of the type flows through
//! it.
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
        template: TemplateType::Single,
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

fn book_page() -> PageContext {
    PageContext {
        title: "Dune".into(),
        post_title: Some("Dune".into()),
        post_type_slug: Some("book".into()),
        ..PageContext::default()
    }
}

#[test]
fn a_type_named_template_wins_for_that_type() {
    let mut engine = vyasa_themes::Engine::builtin().expect("engine");
    engine
        .install_theme_templates(&[(
            "single-book.html",
            r#"{% extends "base.html" %}{% block body %}<article class="shelf">{{ page.post_title }}</article>{% endblock %}"#,
        )])
        .expect("installs");

    let html = render(&engine, &book_page());
    assert!(
        html.contains("<article class=\"shelf\">Dune</article>"),
        "single-book.html renders the book:\n{html}"
    );

    // An ordinary post on the same engine keeps the ordinary template.
    let post = PageContext {
        title: "Hello".into(),
        post_title: Some("Hello".into()),
        ..PageContext::default()
    };
    let html = render(&engine, &post);
    assert!(
        !html.contains("class=\"shelf\""),
        "posts do not fall into the book template:\n{html}"
    );
}

#[test]
fn without_an_override_the_type_renders_as_before() {
    let engine = vyasa_themes::Engine::builtin().expect("engine");
    let html = render(&engine, &book_page());
    assert!(
        html.contains("Dune"),
        "the ordinary single template still shows the entry:\n{html}"
    );
}

#[test]
fn per_type_names_are_legal_template_names() {
    use vyasa_themes::studio::is_template_name;
    assert!(is_template_name("single-book.html"));
    assert!(is_template_name("archive-product.html"));
    assert!(is_template_name("single.html"));
    // The stem set stays closed: a typo'd stem would silently shadow
    // nothing, so it is refused loudly instead.
    assert!(!is_template_name("entry-book.html"));
    assert!(!is_template_name("single-.html"));
    assert!(!is_template_name("single-Book.html"));
    assert!(!is_template_name("single-book"));
}
