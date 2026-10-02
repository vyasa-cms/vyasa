//! Product-site rendering and responsive layout contracts.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "support/fake_queries.rs"]
mod fake_queries;
use vyasa_themes::{
    builtin_registry, parse_token_set, render_page, Engine, Layout, PageContext, SiteMeta,
    TemplateType,
};

#[test]
fn technology_starter_is_editable_and_progressively_enhanced() {
    let layout: Layout = serde_json::from_str(include_str!(
        "../../../themes-starter/technology/layout.json"
    ))
    .expect("layout");
    let tokens = parse_token_set(include_str!(
        "../../../themes-starter/technology/tokens.json"
    ))
    .expect("tokens");
    let engine = Engine::builtin().expect("engine");
    let page = PageContext {
        title: "Technology".into(),
        ..PageContext::default()
    };
    let html = futures::executor::block_on(render_page(vyasa_themes::RenderRequest {
        engine: &engine,
        layout: &layout,
        registry: &builtin_registry(),
        tokens: &tokens,
        template: TemplateType::Index,
        site: &SiteMeta::default(),
        page: &page,
        queries: fake_queries::FakeQueries::shared(),
        post_id: None,
        content_blocks: None,
        reply_to: None,
        editor: false,
    }))
    .expect("render");
    for marker in [
        "vy-code-examples",
        "vy-execution-flow",
        "vy-status-cards",
        "components.js",
        "prefers-reduced-motion",
        "vy-motion-rise",
        "println!",
    ] {
        assert!(html.contains(marker), "missing {marker}");
    }
    assert!(!html.contains("data-vy-edit="));
    if let Ok(path) = std::env::var("VYASA_PRODUCT_PREVIEW") {
        std::fs::write(path, html).expect("write preview");
    }
}

#[test]
fn column_proportions_and_alignment_are_rendered_and_validated() {
    let mut section = vyasa_themes::Section::new("columns", "columns");
    section.settings = serde_json::from_value(
        serde_json::json!({"count":2,"ratio":"wide-start","align":"center"}),
    )
    .expect("settings");
    let registry = builtin_registry();
    assert!(registry
        .get("columns")
        .expect("kind")
        .validate_settings(&section.settings)
        .is_empty());
    let html = vyasa_themes::sections::render_tree(
        &[section.clone()],
        &std::collections::BTreeMap::default(),
    );
    assert!(html.contains("--vy-tracks:minmax(0, 2fr) minmax(0, 1fr)"));
    assert!(html.contains("--vy-align:center"));
    section
        .settings
        .set("ratio", serde_json::json!("arbitrary"));
    assert!(!registry
        .get("columns")
        .expect("kind")
        .validate_settings(&section.settings)
        .is_empty());
}

#[test]
fn motion_does_not_change_section_width_and_rejects_unknown_effects() {
    let mut section = vyasa_themes::Section::new("motion", "group");
    section.scope = Some(vyasa_themes::StyleScope {
        motion: Some("fade".into()),
        ..vyasa_themes::StyleScope::default()
    });
    let html = vyasa_themes::sections::render_tree(
        &[section.clone()],
        &std::collections::BTreeMap::default(),
    );
    assert!(html.contains("vy-motion-fade"));
    assert!(!html.contains("vy-bleed"));
    let scope = vyasa_themes::StyleScope {
        motion: Some("custom-script".into()),
        ..vyasa_themes::StyleScope::default()
    };
    assert!(!vyasa_themes::scope::validate_scope(
        &scope,
        &vyasa_themes::TokenSet::default(),
        "motion"
    )
    .is_empty());
}
