//! What a theme can say about narrow screens.
//!
//! Until breakpoints were named, the stylesheet carried two magic numbers
//! (640 and 900) that no theme could change, `columns` went from four
//! across straight to one, and there was no way to withhold a decorative
//! section from a phone. All three were the same gap: the design system had
//! nothing to say about screen size.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_themes::{
    builtin_registry, render_tree, sections_css, validate_sections, MapSettings, ResolvedSections,
    Screen, Section, TokenSet,
};

fn settings(pairs: &[(&str, serde_json::Value)]) -> MapSettings {
    let mut s = MapSettings::default();
    for (k, v) in pairs {
        s.set((*k).to_owned(), v.clone());
    }
    s
}

fn columns(pairs: &[(&str, serde_json::Value)]) -> String {
    let mut cols = Section::new("cols", "columns");
    cols.settings = settings(pairs);
    cols.children = vec![Section::new("a", "hero")];
    let mut resolved = ResolvedSections::new();
    resolved.insert("a".into(), "<p>a</p>".into());
    render_tree(&[cols], &resolved)
}

#[test]
fn a_theme_sets_its_own_breakpoints() {
    let mut tokens = TokenSet::default();
    tokens.layout.breakpoint_sm_px = 520.0;
    tokens.layout.breakpoint_md_px = 1100.0;
    let css = sections_css(&tokens);

    assert!(css.contains("max-width: 520px"), "sm honoured:\n{css}");
    assert!(css.contains("max-width: 1100px"), "md honoured:\n{css}");
    // And no marker survived into the stylesheet.
    assert!(
        !css.contains("0sm") && !css.contains("0md"),
        "stale marker:\n{css}"
    );
}

#[test]
fn the_defaults_are_the_numbers_that_used_to_be_hard_coded() {
    // A theme written before breakpoints existed has to render identically.
    let css = sections_css(&TokenSet::default());
    assert!(css.contains("max-width: 640px"));
    assert!(css.contains("max-width: 900px"));
}

#[test]
fn base_rules_take_the_breakpoints_too() {
    let mut tokens = TokenSet::default();
    tokens.layout.breakpoint_md_px = 1100.0;
    let css = vyasa_themes::base_css(&tokens);
    assert!(
        css.contains("min-width: 1100px"),
        "docs layout query:\n{css}"
    );
    assert!(!css.contains("0md"), "stale marker:\n{css}");
}

#[test]
fn breakpoints_have_to_be_the_right_way_round() {
    let mut tokens = TokenSet::default();
    tokens.layout.breakpoint_sm_px = 1200.0;
    tokens.layout.breakpoint_md_px = 700.0;
    let diags = vyasa_themes::validate_token_set(&tokens);
    assert!(
        diags
            .iter()
            .any(|d| d.path() == "layout.breakpoint_md_px" && d.is_error()),
        "{diags:?}"
    );
}

#[test]
fn columns_have_a_size_between_desktop_and_phone() {
    // Four across going straight to one skipped the width most visitors
    // are actually on.
    let html = columns(&[("count", serde_json::json!(4))]);
    assert!(html.contains("--vy-cols:4"), "{html}");
    assert!(
        html.contains("--vy-cols-tablet:2"),
        "half, rounded up: {html}"
    );
    assert!(html.contains("--vy-cols-mobile:1"), "{html}");
}

#[test]
fn an_author_can_say_what_the_middle_size_does() {
    let html = columns(&[
        ("count", serde_json::json!(6)),
        ("count_tablet", serde_json::json!(3)),
    ]);
    assert!(html.contains("--vy-cols-tablet:3"), "{html}");
}

#[test]
fn a_nonsense_column_count_is_refused_by_name() {
    let mut cols = Section::new("cols", "columns");
    cols.settings = settings(&[("count_tablet", serde_json::json!(99))]);
    let diags = validate_sections(&[cols], &builtin_registry(), None, "page");
    assert!(
        diags.iter().any(|d| d.path() == "page[0].settings"),
        "{diags:?}"
    );
}

#[test]
fn a_section_can_be_withheld_from_one_screen_size() {
    let mut band = Section::new("decor", "logo-wall");
    band.settings = settings(&[("items", serde_json::json!([{"name": "Acme"}]))]);
    band.hide_on = Some(Screen::Mobile);
    let mut resolved = ResolvedSections::new();
    resolved.insert("decor".into(), "<p>logos</p>".into());

    let html = render_tree(&[band], &resolved);
    assert!(html.contains("vy-hide-mobile"), "{html}");

    let css = sections_css(&TokenSet::default());
    assert!(
        css.contains("@media (max-width: 640px) { .vy-hide-mobile { display: none !important; } }"),
        "{css}"
    );
    // The complement, so there is no width where both or neither apply.
    assert!(
        css.contains("@media not all and (max-width: 640px) { .vy-hide-desktop"),
        "{css}"
    );
}

#[test]
fn hiding_survives_a_round_trip_through_json() {
    let mut s = Section::new("decor", "hero");
    s.hide_on = Some(Screen::Desktop);
    let json = serde_json::to_value(&s).expect("serialises");
    assert_eq!(json["hide_on"], serde_json::json!("desktop"));
    let back: Section = serde_json::from_value(json).expect("round trips");
    assert_eq!(back.hide_on, Some(Screen::Desktop));
}

#[test]
fn a_section_that_hides_nothing_says_nothing() {
    // The field is skipped when unset, so existing layouts do not grow a
    // null and every stored document stays byte-identical.
    let json = serde_json::to_value(Section::new("plain", "hero")).expect("serialises");
    assert!(json.get("hide_on").is_none(), "{json}");
}
