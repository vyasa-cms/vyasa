//! `"credit": "theme"` in layout.json: the theme prints the credit itself
//! with `{{ vyasa.credit_html }}`; the engine then adds nothing, but still
//! makes sure the credit is on the page.
#![allow(clippy::expect_used)]

use vyasa_themes::{
    builtin_registry, render_page, Credit, Engine, Layout, PageContext, RenderRequest, SiteMeta,
    TemplateType, TokenSet,
};

#[path = "support/fake_queries.rs"]
mod fake_queries;

const WITH_CREDIT: &str = r#"{% extends "base.html" %}{% block body %}<main>x</main><footer class="mine">{{ vyasa.credit_html | safe }}</footer>{% endblock %}"#;
const WITHOUT_CREDIT: &str =
    r#"{% extends "base.html" %}{% block body %}<main>x</main><footer class="mine">c</footer>{% endblock %}"#;

fn render(template: &str, credit: Credit, footer_credit: bool) -> String {
    let mut engine = Engine::builtin().expect("engine");
    engine
        .install_theme_templates(&[("page.html", template)])
        .expect("installs");
    let layout = Layout {
        credit,
        ..Layout::default()
    };
    futures::executor::block_on(render_page(RenderRequest {
        engine: &engine,
        layout: &layout,
        registry: &builtin_registry(),
        tokens: &TokenSet::default(),
        template: TemplateType::Page,
        site: &SiteMeta {
            footer_credit,
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

fn credits(html: &str) -> usize {
    html.matches("class=\"vy-credit\"").count()
}

#[test]
fn a_theme_that_places_the_credit_gets_exactly_one() {
    let html = render(WITH_CREDIT, Credit::Theme, true);
    assert_eq!(credits(&html), 1, "{html}");
    assert!(html.contains("<footer class=\"mine\"><p class=\"vy-credit\">"), "{html}");
}

#[test]
fn a_theme_that_forgets_it_still_shows_the_credit() {
    let html = render(WITHOUT_CREDIT, Credit::Theme, true);
    assert_eq!(credits(&html), 1, "{html}");
}

#[test]
fn the_site_option_turns_it_off_either_way() {
    assert_eq!(credits(&render(WITH_CREDIT, Credit::Theme, false)), 0);
    assert_eq!(credits(&render(WITHOUT_CREDIT, Credit::Engine, false)), 0);
}

#[test]
fn the_default_is_still_the_engine() {
    assert_eq!(credits(&render(WITHOUT_CREDIT, Credit::Engine, true)), 1);
    assert_eq!(Layout::default().credit, Credit::Engine);
}

#[test]
fn layout_json_reads_the_option() {
    let l: Layout = serde_json::from_str(r#"{"credit":"theme"}"#).expect("parses");
    assert_eq!(l.credit, Credit::Theme);
}
