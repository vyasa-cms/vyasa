//! The thing the old model could not express.
//!
//! Before sections nested, a template was a flat list of blog primitives with
//! one global palette — a hero over a three-column feature grid over a dark
//! call-to-action band was simply not representable. This test builds exactly
//! that and asserts it validates, scopes, and renders.

use vyasa_themes::{
    builtin_registry, composes, render_tree, scopes_to_css, validate_layout, ColorRef, Layout,
    MapSettings, ResolvedSections, Section, StyleScope, TemplateType, TokenSet,
};

fn settings(pairs: &[(&str, serde_json::Value)]) -> MapSettings {
    let mut s = MapSettings::default();
    for (k, v) in pairs {
        s.set((*k).to_owned(), v.clone());
    }
    s
}

fn landing() -> Vec<Section> {
    let mut hero = Section::new("hero", "hero");
    hero.settings = settings(&[
        ("eyebrow", serde_json::json!("Release 2.4")),
        ("headline", serde_json::json!("Publishing that keeps up")),
        (
            "subhead",
            serde_json::json!("Faster builds and a rewritten editor."),
        ),
        ("primary_label", serde_json::json!("Read the notes")),
        ("primary_url", serde_json::json!("/release-2-4")),
    ]);

    let mut features = Section::new("features", "feature-grid");
    features.settings = settings(&[
        ("heading", serde_json::json!("What changed")),
        ("columns", serde_json::json!(3)),
        (
            "items",
            serde_json::json!([
                {"title": "Instant preview", "body": "Rendered as the reader sees it."},
                {"title": "Section library", "body": "Compose from typed parts."},
                {"title": "Safe by default", "body": "Sandboxed and validated."}
            ]),
        ),
    ]);

    let mut cta = Section::new("cta", "cta-band");
    cta.settings = settings(&[
        ("headline", serde_json::json!("Ready to try it?")),
        ("label", serde_json::json!("Start free")),
        ("url", serde_json::json!("/signup")),
    ]);
    // The dark band: derived from the palette, not hard-coded, so it stays
    // correct if the theme's colours change.
    cta.scope = Some(StyleScope {
        bg: Some(ColorRef::Value("$text".into())),
        text: Some(ColorRef::Value("$bg".into())),
        ..Default::default()
    });

    let mut band = Section::new("cta-band", "band");
    band.children = vec![cta];

    vec![
        Section::new("header", "header"),
        hero,
        features,
        band,
        Section::new("footer", "footer"),
    ]
}

fn layout_with(sections: Vec<Section>) -> Layout {
    let mut l = Layout {
        page: sections,
        ..Default::default()
    };
    // Every template needs at least one section to validate.
    for t in TemplateType::ALL {
        if l.for_template(t).is_empty() {
            *l.for_template_mut(t) = vec![Section::new("body", "post-content")];
        }
    }
    l
}

#[test]
fn a_landing_page_layout_validates() {
    let layout = layout_with(landing());
    let diags = validate_layout(&layout, &builtin_registry());
    let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn it_takes_the_composing_render_path() {
    let layout = layout_with(landing());
    assert!(composes(layout.for_template(TemplateType::Page)));
}

#[test]
fn the_tree_renders_nested_markup() {
    let layout = layout_with(landing());
    let mut resolved = ResolvedSections::new();
    resolved.insert("hero".into(), "<h1>Publishing that keeps up</h1>".into());
    resolved.insert("features".into(), "<ul class=\"vy-cards\"></ul>".into());
    resolved.insert("cta".into(), "<h2>Ready to try it?</h2>".into());

    let html = render_tree(layout.for_template(TemplateType::Page), &resolved);

    // The call to action sits *inside* the band — the nesting the flat model
    // could not express.
    let band_at = html.find("vy-band").expect("band rendered");
    let cta_at = html.find("Ready to try it?").expect("cta rendered");
    assert!(band_at < cta_at);
    assert!(html.contains("vy-scope-cta"), "scope class applied");
    // Order is document order.
    let hero_at = html.find("Publishing that keeps up").expect("hero");
    assert!(hero_at < cta_at);
}

#[test]
fn the_dark_band_compiles_to_scoped_properties() {
    let layout = layout_with(landing());
    let tokens = TokenSet::default();
    let css = scopes_to_css(&layout, &tokens);

    assert!(css.contains(".vy-scope-cta"), "scope rule emitted:\n{css}");
    // `$text` resolved against the theme, so the band's background is the
    // theme's text colour rather than a literal someone typed.
    let expected_bg = tokens.colors.text.light.normalized();
    assert!(
        css.contains(&format!("--vy-color-bg: {expected_bg}")),
        "expected bg {expected_bg} in:\n{css}"
    );
}

#[test]
fn a_scope_that_would_be_unreadable_is_flagged() {
    // Text and background set to the same colour: valid syntax, unreadable
    // output. The contrast check should warn rather than let it ship.
    let mut cta = Section::new("bad", "cta-band");
    cta.settings = settings(&[("headline", serde_json::json!("Hi"))]);
    cta.scope = Some(StyleScope {
        bg: Some(ColorRef::Value("#808080".into())),
        text: Some(ColorRef::Value("#828282".into())),
        ..Default::default()
    });

    let diags = vyasa_themes::validate_scope(
        cta.scope.as_ref().expect("scope"),
        &TokenSet::default(),
        "page[0]",
    );
    assert!(
        diags.iter().any(|d| d.to_string().contains("4.5:1")),
        "expected a contrast warning, got: {diags:?}"
    );
}

#[test]
fn a_dark_band_is_warned_about_the_text_it_did_not_override() {
    // The commonest scope there is: invert bg and text, say nothing about
    // secondary text. It keeps its light-mode grey and lands on near-black,
    // which is how a sub-heading nobody can read reaches a live page.
    let scope = StyleScope {
        bg: Some(ColorRef::Value("$text".into())),
        text: Some(ColorRef::Value("$bg".into())),
        ..Default::default()
    };
    let diags = vyasa_themes::validate_scope(&scope, &TokenSet::default(), "page[0]");
    assert!(
        diags.iter().any(|d| d.path() == "page[0].scope.text-muted"),
        "expected a text-muted warning, got: {diags:?}"
    );
    // And no errors: it is a warning, not a refusal — the author may mean it.
    assert!(!diags.iter().any(vyasa_themes::TokenDiagnostic::is_error));
}

#[test]
fn overriding_the_muted_role_too_clears_the_warning() {
    let scope = StyleScope {
        bg: Some(ColorRef::Value("$text".into())),
        text: Some(ColorRef::Value("$bg".into())),
        text_muted: Some(ColorRef::Value("$surface".into())),
        ..Default::default()
    };
    let diags = vyasa_themes::validate_scope(&scope, &TokenSet::default(), "page[0]");
    assert!(
        !diags.iter().any(|d| d.path() == "page[0].scope.text-muted"),
        "still warned: {diags:?}"
    );
}

#[test]
fn children_on_a_leaf_kind_are_rejected() {
    let mut hero = Section::new("hero", "hero");
    hero.settings = settings(&[("headline", serde_json::json!("x"))]);
    hero.children = vec![Section::new("nope", "header")];

    let layout = layout_with(vec![hero]);
    let diags = validate_layout(&layout, &builtin_registry());
    assert!(
        diags
            .iter()
            .any(|d| d.to_string().contains("holds no children")),
        "expected a children error, got: {diags:?}"
    );
}
