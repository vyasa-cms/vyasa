//! Rendering an entry through a type's section tree.
#![allow(clippy::expect_used)]

use vyasa_themes::{Engine, Layout, TokenSet};

#[path = "support/fake_queries.rs"]
mod fake_queries;
#[path = "support/marker.rs"]
mod marker;

fn render(engine: &Engine, page: &vyasa_themes::PageContext) -> String {
    marker::render(engine, &Layout::default(), &TokenSet::default(), page)
}

#[test]
fn the_bundled_single_template_prints_a_typed_tree() {
    let engine = Engine::builtin().expect("engine");
    let html = render(&engine, &marker::page(marker::tree()));
    assert!(html.contains("MARK:test/marker"), "{html}");
    assert!(html.contains("BODY-HTML"), "{html}");
    assert!(
        html.find("MARK:test/marker") < html.find("BODY-HTML"),
        "sidebar column precedes the body"
    );
}

#[test]
fn a_theme_that_ignores_sections_still_shows_the_tree() {
    let mut engine = Engine::builtin().expect("engine");
    // A theme's own single template that never prints `sections`.
    engine
        .install_theme_templates(&[(
            "single.html",
            r#"{% extends "base.html" %}{% block body %}<article class="mine">{{ page.post_title }}</article>{% endblock %}"#,
        )])
        .expect("installs");
    let html = render(&engine, &marker::page(marker::tree()));
    assert!(
        html.contains("MARK:test/marker"),
        "composed.html stands in:\n{html}"
    );
    assert!(html.contains("BODY-HTML"));
    assert!(
        !html.contains("class=\"mine\""),
        "the theme template did not run for a composed entry"
    );
    // Without a tree the theme's own template is used as before.
    let plain = render(&engine, &marker::page(Vec::new()));
    assert!(plain.contains("class=\"mine\""), "{plain}");
}

#[test]
fn a_type_template_still_wins_over_the_tree_fallback() {
    let mut engine = Engine::builtin().expect("engine");
    engine
        .install_theme_templates(&[(
            "single-doc.html",
            r#"{% extends "base.html" %}{% block body %}<article class="doc-tpl">{{ sections | safe }}</article>{% endblock %}"#,
        )])
        .expect("installs");
    let html = render(&engine, &marker::page(marker::tree()));
    assert!(html.contains("class=\"doc-tpl\""), "{html}");
    assert!(html.contains("MARK:test/marker"), "{html}");
}
