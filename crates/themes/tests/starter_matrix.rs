//! Every starter theme, every registered section kind, every template:
//! the layout must validate and the page must render. This is the
//! contract the theme studio relies on when it offers a kind for insertion
//! into a starter — a kind that renders for one theme and errors for
//! another would be a trap the palette cannot see.

#![allow(clippy::pedantic)]

#[path = "support/fake_queries.rs"]
mod fake_queries;

use vyasa_themes::{
    builtin_registry, parse_token_set, render_page, validate_layout, Engine, Layout, MapSettings,
    PageContext, Section, SiteMeta, TemplateType, TokenSet,
};

const STARTERS: [(&str, &str, &str); 5] = [
    (
        "technology",
        include_str!("../../../themes-starter/technology/layout.json"),
        include_str!("../../../themes-starter/technology/tokens.json"),
    ),
    (
        "blog",
        include_str!("../../../themes-starter/blog/layout.json"),
        include_str!("../../../themes-starter/blog/tokens.json"),
    ),
    (
        "docs",
        include_str!("../../../themes-starter/docs/layout.json"),
        include_str!("../../../themes-starter/docs/tokens.json"),
    ),
    (
        "portfolio",
        include_str!("../../../themes-starter/portfolio/layout.json"),
        include_str!("../../../themes-starter/portfolio/tokens.json"),
    ),
    (
        "storefront-lite",
        include_str!("../../../themes-starter/storefront-lite/layout.json"),
        include_str!("../../../themes-starter/storefront-lite/tokens.json"),
    ),
];

const TEMPLATES: [TemplateType; 6] = [
    TemplateType::Index,
    TemplateType::Single,
    TemplateType::Archive,
    TemplateType::Page,
    TemplateType::Search,
    TemplateType::NotFound,
];

fn starters() -> Vec<(&'static str, Layout, TokenSet)> {
    STARTERS
        .iter()
        .map(|(name, layout, tokens)| {
            let layout: Layout =
                serde_json::from_str(layout).unwrap_or_else(|e| panic!("{name} layout: {e}"));
            let tokens = parse_token_set(tokens).unwrap_or_else(|e| panic!("{name} tokens: {e:?}"));
            (*name, layout, tokens)
        })
        .collect()
}

fn render(layout: &Layout, tokens: &TokenSet, template: TemplateType) -> String {
    let engine = Engine::builtin().unwrap_or_else(|e| panic!("builtin engine: {e}"));
    futures::executor::block_on(render_page(vyasa_themes::RenderRequest {
        engine: &engine,
        layout,
        registry: &builtin_registry(),
        tokens,
        template,
        site: &SiteMeta::default(),
        page: &PageContext {
            title: "Probe".into(),
            ..PageContext::default()
        },
        queries: fake_queries::FakeQueries::shared(),
        post_id: None,
        content_blocks: None,
        reply_to: None,
        editor: false,
    }))
    .unwrap_or_else(|e| panic!("renders: {e}"))
}

/// The shipped starters validate against the registry with no errors.
#[test]
fn starters_validate_clean() {
    let registry = builtin_registry();
    for (name, layout, _) in starters() {
        let errors: Vec<_> = validate_layout(&layout, &registry)
            .into_iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:?}");
    }
}

/// Inserting any registered kind with default settings into any starter
/// template keeps the layout valid and the page renderable, in both the
/// public and the studio-canvas modes.
#[test]
fn every_kind_renders_in_every_starter() {
    let registry = builtin_registry();
    let kinds: Vec<String> = registry.kinds().into_iter().map(str::to_owned).collect();
    assert!(kinds.len() > 20, "registry lists its kinds: {kinds:?}");
    let mut failures: Vec<String> = Vec::new();
    for (name, layout, tokens) in starters() {
        for kind in &kinds {
            for template in TEMPLATES {
                let mut probe = layout.clone();
                // The studio seeds an inserted kind with its sample
                // settings; the probe does exactly what the palette does.
                let settings: MapSettings = registry
                    .get(kind)
                    .and_then(|b| serde_json::from_value(b.sample()).ok())
                    .unwrap_or_default();
                probe.for_template_mut(template).push(Section {
                    id: "probe".into(),
                    kind: kind.clone(),
                    settings,
                    children: Vec::new(),
                    scope: None,
                    hide_on: None,
                    width: None,
                });
                let errors: Vec<_> = validate_layout(&probe, &registry)
                    .into_iter()
                    .filter(|d| d.is_error())
                    .collect();
                if !errors.is_empty() {
                    failures.push(format!("{name}/{template:?} + {kind}: {errors:?}"));
                    continue;
                }
                let html = render(&probe, &tokens, template);
                if !html.starts_with("<!doctype html>") {
                    failures.push(format!("{name}/{template:?} + {kind}: not a page"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The shell prints the menu once. A layout that declares a `menu`
/// section and composes its body used to get the menu in the masthead
/// and again at the top of the body.
#[test]
fn chrome_renders_once_per_page() {
    for (name, layout, tokens) in starters() {
        for template in TEMPLATES {
            let html = render(&layout, &tokens, template);
            for (needle, label) in [
                ("class=\"vy-site-name\"", "site name"),
                ("class=\"vy-nav\"", "navigation"),
                ("class=\"vy-footer-inner\"", "footer"),
            ] {
                let n = html.matches(needle).count();
                assert!(n <= 1, "{name}/{template:?}: {label} rendered {n} times");
            }
        }
    }
}
