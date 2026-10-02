//! Renders a section tree to HTML.
//!
//! The original layout model resolved each block to a named slot and let the
//! Tera template decide placement. That works for chrome — a header is a
//! header wherever it goes — but it cannot express composition: a template
//! placing `regions.hero` has no way to say "and inside it, three columns".
//!
//! This module walks the tree instead. Containers emit a wrapper carrying
//! their layout settings and their scope class; leaves emit whatever their
//! resolver produced. Slot filling still happens for chrome, so existing
//! templates keep working — a theme opts into composition by nesting, not by
//! rewriting its templates.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::layout::{MapSettings, Section, STATIC_REGIONS};

/// Resolved HTML for each section id, produced by the dynamic-block registry.
pub type ResolvedSections = BTreeMap<String, String>;

fn uint(settings: &MapSettings, key: &str, default: u64) -> u64 {
    settings
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(default)
}

fn attr(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;")
}

/// Whether a kind wraps children rather than rendering its own content.
fn is_container(kind: &str) -> bool {
    matches!(kind, "band" | "columns" | "grid" | "group")
}

/// Whether a section spans the page rather than the reading column.
///
/// Sections render inside a container centred at the content width, so a
/// band or a coloured strip has to escape it or it is a rectangle in the
/// middle of the text. Both ways of asking to stay put — a `band`'s
/// `contained` setting and a scope's `contained` — are honoured here, so
/// there is one rule rather than two that can disagree.
fn bleeds(section: &Section) -> bool {
    // An explicit width is the strongest word: wide and full escape the
    // column no matter what else is set, narrow and content stay put.
    match section.width {
        Some(crate::layout::SectionWidth::Wide | crate::layout::SectionWidth::Full) => {
            return true;
        }
        Some(_) => return false,
        None => {}
    }
    let asked_to_stay = section
        .settings
        .get("contained")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        || section
            .scope
            .as_ref()
            .is_some_and(|s| s.contained == Some(true));
    if asked_to_stay {
        return false;
    }
    section.kind == "band"
        || section
            .scope
            .as_ref()
            .is_some_and(|s| !s.colors().is_empty() || s.padding_y.is_some())
}

/// Inline style carrying a container's layout settings.
///
/// These are the only inline styles the renderer emits, and every value is a
/// number the schema already validated — never author text.
fn container_style(kind: &str, settings: &MapSettings) -> String {
    match kind {
        "columns" => {
            let count = uint(settings, "count", 2).clamp(1, 6);
            // Half the desktop count, rounded up: three across becomes two,
            // which is what an author means by "narrower" without saying so.
            let tablet = uint(settings, "count_tablet", count.div_ceil(2)).clamp(1, 6);
            let mobile = uint(settings, "count_mobile", 1).clamp(1, 2);
            let gap = uint(settings, "gap", 3).clamp(0, 12);
            let align = match settings.get("align").and_then(serde_json::Value::as_str) {
                Some("center") => "center",
                Some("end") => "end",
                Some("stretch") => "stretch",
                _ => "start",
            };
            let tracks = match (
                count,
                settings.get("ratio").and_then(serde_json::Value::as_str),
            ) {
                (2, Some("wide-start")) => "minmax(0, 2fr) minmax(0, 1fr)".to_owned(),
                (2, Some("wide-end")) => "minmax(0, 1fr) minmax(0, 2fr)".to_owned(),
                _ => format!("repeat({count}, minmax(0, 1fr))"),
            };
            format!(
                " style=\"--vy-align:{align};--vy-tracks:{tracks};--vy-cols:{count};--vy-cols-tablet:{tablet};\
                 --vy-cols-mobile:{mobile};--vy-gap:calc(var(--vy-space-unit) * {gap})\""
            )
        }
        "grid" => {
            let min = uint(settings, "min_width", 220).clamp(80, 600);
            let gap = uint(settings, "gap", 3).clamp(0, 12);
            format!(" style=\"--vy-grid-min:{min}px;--vy-gap:calc(var(--vy-space-unit) * {gap})\"")
        }
        _ => String::new(),
    }
}

/// Renders one section and its subtree.
fn render_section(out: &mut String, section: &Section, resolved: &ResolvedSections, editor: bool) {
    let anchor = attr(&section.id);
    let scope_class = section
        .scope
        .as_ref()
        .filter(|s| !s.is_empty())
        .map(|_| format!(" vy-scope-{}", attr(&section.id)))
        .unwrap_or_default();
    let bleed_class = if bleeds(section) { " vy-bleed" } else { "" };
    let hide_class = section
        .hide_on
        .map(|s| format!(" {}", s.class()))
        .unwrap_or_default();
    let motion_class = match section.scope.as_ref().and_then(|s| s.motion.as_deref()) {
        Some("fade") => " vy-motion-fade",
        Some("rise") => " vy-motion-rise",
        _ => "",
    };
    let scope_class = scope_class + motion_class;
    let width_class = section
        .width
        .map(|w| format!(" {}", w.class()))
        .unwrap_or_default();

    if is_container(&section.kind) {
        let _ = write!(
            out,
            "<div class=\"vy-section vy-{}{}{}{}{}\" id=\"{anchor}\" data-section=\"{}\"{}>",
            attr(&section.kind),
            scope_class,
            bleed_class,
            hide_class,
            width_class,
            attr(&section.id),
            container_style(&section.kind, &section.settings)
        );
        for child in &section.children {
            render_section(out, child, resolved, editor);
        }
        out.push_str("</div>");
        return;
    }

    // A leaf contributes whatever its resolver produced. An empty result is
    // skipped entirely rather than leaving a hollow wrapper on the page —
    // except on the studio's canvas, where an invisible section can be
    // neither selected nor dragged, so the editor gets a labelled
    // placeholder box instead of a mystery.
    let Some(html) = resolved.get(&section.id).filter(|h| !h.trim().is_empty()) else {
        if editor && !STATIC_REGIONS.contains(&section.kind.as_str()) {
            let _ = write!(
                out,
                "<div class=\"vy-section vy-section--{}{}{}{}{}\" id=\"{anchor}\" data-section=\"{}\" \
                 data-vy-empty=\"{}\"></div>",
                attr(&section.kind),
                scope_class,
                bleed_class,
                hide_class,
                width_class,
                attr(&section.id),
                attr(&section.kind),
            );
        }
        return;
    };
    // A leaf's wrapper is named after the kind with a `vy-section--`
    // prefix, never `vy-<kind>` bare: the component's own root carries
    // that class, and a wrapper sharing it inherited the grid, the
    // padding, the list styling of the thing inside it — the collection's
    // heading fell into a grid cell, the hero's padding doubled.
    let _ = write!(
        out,
        "<div class=\"vy-section vy-section--{}{}{}{}{}\" id=\"{anchor}\" data-section=\"{}\">{html}</div>",
        attr(&section.kind),
        scope_class,
        bleed_class,
        width_class,
        hide_class,
        attr(&section.id)
    );
}

/// Renders a template's whole section tree.
#[must_use]
pub fn render_tree(sections: &[Section], resolved: &ResolvedSections) -> String {
    render_tree_with(sections, resolved, false)
}

/// [`render_tree`] with the studio-preview flag: empty leaves render as
/// labelled placeholder boxes so the canvas can select and drag them.
#[must_use]
pub fn render_tree_with(sections: &[Section], resolved: &ResolvedSections, editor: bool) -> String {
    let mut out = String::new();
    for section in sections {
        render_section(&mut out, section, resolved, editor);
    }
    out
}

/// Whether a template composes its own body.
///
/// A theme that nests anything, or uses a marketing section, is composing —
/// and the engine should hand the template one rendered tree instead of a
/// bag of slots. Themes that stay flat and chrome-only keep the old
/// behaviour, so nothing that works today stops working.
#[must_use]
pub fn composes(sections: &[Section]) -> bool {
    sections.iter().any(|s| {
        !s.children.is_empty()
            || is_container(&s.kind)
            || matches!(
                s.kind.as_str(),
                "hero"
                    | "feature-grid"
                    | "cta-band"
                    | "stats-band"
                    | "faq"
                    | "logo-wall"
                    | "code-tabs"
                    | "execution-flow"
                    | "status-cards"
            )
    })
}

/// Stylesheet for composed sections.
///
/// Kept beside the renderer because the two must agree on class names, and
/// appended to the theme's base CSS rather than injected per page.
#[must_use]
pub fn sections_css(tokens: &crate::tokens::TokenSet) -> String {
    (SECTIONS_CSS.to_owned() + include_str!("product.css"))
        .as_str()
        .replace("0sm", &crate::css::px(tokens.layout.breakpoint_sm_px))
        .replace("0md", &crate::css::px(tokens.layout.breakpoint_md_px))
}

const SECTIONS_CSS: &str = r"
.vy-section { display: block; }
.vy-columns {
  display: grid;
  grid-template-columns: var(--vy-tracks, repeat(var(--vy-cols, 2), minmax(0, 1fr)));
  gap: var(--vy-gap, 1.5rem);
  align-items: var(--vy-align, start);
}
.vy-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(var(--vy-grid-min, 220px), 1fr));
  gap: var(--vy-gap, 1.5rem);
}
/* A band is a stripe across the page, but it renders inside .vy-main,
   which is centred at the reading width. Without this escape a full-width
   band is a rectangle in the middle of the column, which is what it was.
   Nested inside an already-bled parent the margin resolves to zero, so
   this is safe at any depth. */
.vy-bleed {
  width: 100vw;
  max-width: 100vw;
  margin-inline: calc(50% - 50vw);
}
.vy-band { padding-block: calc(var(--vy-space-unit) * 6); }
.vy-bleed > * {
  max-width: var(--vy-content-width);
  margin-inline: auto;
  padding-inline: calc(var(--vy-space-unit) * 5);
}
.vy-hero { padding-block: calc(var(--vy-space-unit) * 8); }
/* Section headings are headings: they take the theme's heading face,
   or a serif brand sits over a sans headline. */
.vy-hero-title, .vy-section-title, .vy-cta-title { font-family: var(--vy-font-heading); }
.vy-hero-title {
  font-size: clamp(calc(var(--vy-base-size) * 1.7),
                   calc(var(--vy-base-size) * 1.15 + 3.2vw),
                   calc(var(--vy-base-size) * 2.6));
  line-height: 1.1; margin: 0 0 .4em;
}
.vy-hero-sub { font-size: calc(var(--vy-base-size) * 1.15); color: var(--vy-color-text-muted); max-width: 46ch; }
.vy-eyebrow {
  text-transform: uppercase; letter-spacing: .1em;
  font-size: calc(var(--vy-base-size) * .75); color: var(--vy-color-primary);
  margin: 0 0 .6em; font-weight: 600;
}
.vy-actions { display: flex; flex-wrap: wrap; gap: .6rem; margin-top: 1.4rem; }
.vy-btn {
  display: inline-block; padding: .6em 1.2em; border-radius: var(--vy-radius);
  text-decoration: none; font-weight: 600;
}
.vy-btn-primary { background: var(--vy-color-primary); color: var(--vy-color-on-primary); }
.vy-btn-ghost { border: 1px solid var(--vy-color-border); color: var(--vy-color-text); }
.vy-cards { list-style: none; padding: 0; margin: 1.5rem 0 0; display: grid; gap: 1.2rem;
  grid-template-columns: repeat(var(--vy-cards, 3), minmax(0, 1fr)); }
.vy-cards[data-columns='1'] { --vy-cards: 1; }
.vy-cards[data-columns='2'] { --vy-cards: 2; }
.vy-cards[data-columns='4'] { --vy-cards: 4; }
.vy-card { background: var(--vy-color-surface); border: 1px solid var(--vy-color-border);
  border-radius: var(--vy-radius); padding: 1.1rem 1.2rem; }
.vy-card-title { margin: 0 0 .35em; font-size: calc(var(--vy-base-size) * 1.05); }
.vy-card-body { margin: 0; color: var(--vy-color-text-muted); }
.vy-section-title { margin: 0;
  font-size: clamp(calc(var(--vy-base-size) * 1.25),
                   calc(var(--vy-base-size) * 0.95 + 1.6vw),
                   calc(var(--vy-base-size) * 1.7)); }
.vy-section-intro { color: var(--vy-color-text-muted); max-width: 56ch; }
.vy-stats { list-style: none; display: flex; flex-wrap: wrap; gap: 2.4rem; padding: 0; margin: 0; }
.vy-stat { display: flex; flex-direction: column; }
.vy-stat-value { font-size: calc(var(--vy-base-size) * 2.2); font-weight: 700; line-height: 1; }
.vy-stat-label { color: var(--vy-color-text-muted); font-size: calc(var(--vy-base-size) * .85); }
.vy-cta { padding-block: calc(var(--vy-space-unit) * 4); }
.vy-cta-title { margin: 0 0 .3em;
  font-size: clamp(calc(var(--vy-base-size) * 1.35),
                   calc(var(--vy-base-size) * 1.0 + 1.9vw),
                   calc(var(--vy-base-size) * 1.9)); }
.vy-cta-sub { color: var(--vy-color-text-muted); margin: 0; }
.vy-faq-item { border-bottom: 1px solid var(--vy-color-border); padding: .8rem 0; }
.vy-faq-item summary { cursor: pointer; font-weight: 600;
  display: flex; align-items: center; min-height: 44px; }
.vy-signup-form { display: grid; gap: .8rem; max-width: 28rem; }
.vy-signup-field { display: grid; gap: .25rem; font-size: .9rem; }
.vy-signup-field input, .vy-signup-field textarea {
  padding: .55rem .7rem; border: 1px solid var(--vy-color-border);
  border-radius: var(--vy-radius); background: var(--vy-color-surface);
  color: var(--vy-color-text); font: inherit;
}
.vy-signup-subhead { color: var(--vy-color-text-muted); }
/* The honeypot: parked off-screen so people never see it and bots do. */
.vy-hp { position: absolute !important; left: -9999px !important; width: 1px; height: 1px; }
/* Explicit widths. Narrow keeps prose readable; wide and full carry the
   vy-bleed escape, so their children caps are relaxed here. */
.vy-w-narrow { max-width: 42rem; margin-inline: auto; }
.vy-w-wide.vy-bleed > * { max-width: min(92rem, 100%); }
.vy-w-full.vy-bleed > * { max-width: none; }
.vy-image { margin: 0; }
.vy-image img { width: 100%; height: auto; border-radius: var(--vy-radius); display: block; }
.vy-image figcaption, .vy-video figcaption {
  color: var(--vy-color-text-muted); font-size: calc(var(--vy-base-size) * .85);
  margin-top: .5rem; }
.vy-video { margin: 0; }
.vy-video video { width: 100%; border-radius: var(--vy-radius); display: block; }
.vy-hero--split { display: grid; grid-template-columns: 1.05fr .95fr; gap: 3rem; align-items: center; }
.vy-hero-media { width: 100%; height: auto; border-radius: var(--vy-radius); }
.vy-hero--center { text-align: center; }
.vy-hero--center .vy-hero-sub { margin-inline: auto; }
.vy-hero--center .vy-actions { justify-content: center; }
.vy-hero--media-start .vy-hero-media { order: -1; }
.vy-card-title a { color: inherit; text-decoration: none; }
.vy-card-title a:hover { color: var(--vy-color-primary); text-decoration: underline; }
.vy-section[id] { scroll-margin-top: 2rem; }
.vy-nav-toggle { display: none; }
.vy-nav-check { position: absolute; opacity: 0; width: 1px; height: 1px; pointer-events: none; }
.vy-nav .vy-menu { display: flex; flex-wrap: wrap; gap: 1.4rem; list-style: none; padding: 0; margin: 0; }
.vy-nav .vy-menu a { text-decoration: none; color: var(--vy-color-text); font-weight: 500; }
.vy-nav .vy-menu .vy-menu { display: none; }
.vy-collection { display: grid; gap: 1.2rem; margin-top: 1.2rem;
  grid-template-columns: repeat(var(--vy-cols, 3), minmax(0, 1fr)); }
.vy-collection .vy-post-card { background: var(--vy-color-surface);
  border: 1px solid var(--vy-color-border); border-radius: var(--vy-radius);
  padding: 1.1rem 1.2rem; display: flex; flex-direction: column; gap: .35rem; }
.vy-collection .vy-post-card__link { font-weight: 600; text-decoration: none;
  color: var(--vy-color-text); font-size: calc(var(--vy-base-size) * 1.05); }
.vy-collection .vy-post-card__meta { color: var(--vy-color-text-muted);
  font-size: calc(var(--vy-base-size) * .8); }
.vy-collection .vy-post-card__thumb { width: 100%; border-radius: var(--vy-radius);
  order: -1; aspect-ratio: 16 / 10; object-fit: cover; }
.vy-collection .vy-post-card__excerpt { margin: 0; color: var(--vy-color-text-muted);
  font-size: calc(var(--vy-base-size) * .9); }
/* Withholding a section from one screen size. `!important` because the
   thing being hidden may be a grid or a flex container with a display of
   its own, and `not all and (...)` is the exact complement of the mobile
   query — no off-by-one pixel where both or neither apply. */
@media (max-width: 0sm) { .vy-hide-mobile { display: none !important; } }
@media not all and (max-width: 0sm) { .vy-hide-desktop { display: none !important; } }
@media (max-width: 0md) {
  .vy-columns { grid-template-columns: repeat(var(--vy-cols-tablet, 2), minmax(0, 1fr)); }
}
@media (max-width: 0md) {
  .vy-collection { grid-template-columns: repeat(min(var(--vy-cols, 3), 2), minmax(0, 1fr)); }
  .vy-cards { grid-template-columns: repeat(min(var(--vy-cards, 3), 2), minmax(0, 1fr)); }
}
@media (max-width: 0sm) {
  .vy-columns { grid-template-columns: repeat(var(--vy-cols-mobile, 1), minmax(0, 1fr)); }
  .vy-cards { grid-template-columns: 1fr; }
  .vy-collection { grid-template-columns: 1fr; }
  .vy-hero--split { grid-template-columns: 1fr; gap: 1.5rem; }
  /* The menu folds into a no-JS drawer: the label becomes the button and
     the checkbox it toggles decides whether the list shows. */
  .vy-nav-toggle {
    /* The primary navigation control on a phone, so a full tap target. */
    min-height: 44px;
    display: inline-flex; align-items: center; gap: .5rem; cursor: pointer;
    font-weight: 600; padding: .4rem .2rem; }
  .vy-nav-toggle:has(.vy-nav-check:not(:checked)) ~ .vy-menu { display: none; }
  .vy-nav-toggle:has(.vy-nav-check:focus-visible) { outline: 2px solid var(--vy-color-primary); outline-offset: 2px; }
  .vy-nav-toggle::before {
    content: ''; width: 1.1rem; height: .8rem;
    background: linear-gradient(var(--vy-color-text) 0 2px, transparent 2px 5px,
      var(--vy-color-text) 5px 7px, transparent 7px 10px, var(--vy-color-text) 10px 12px);
  }
  .vy-nav-toggle::after { content: 'Menu'; }
  .vy-nav .vy-menu { flex-direction: column; gap: .3rem; padding: .5rem 0; flex-basis: 100%; }
  .vy-nav .vy-menu a { display: block; padding: .45rem .2rem; }
}
";

/// Built-in interaction runtime shared by all themes.
pub const COMPONENTS_JS: &str = include_str!("components.js");

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: &str, kind: &str) -> Section {
        Section::new(id, kind)
    }

    #[test]
    fn containers_wrap_their_children() {
        let mut columns = leaf("cols", "columns");
        columns.children = vec![leaf("a", "hero"), leaf("b", "hero")];

        let mut resolved = ResolvedSections::new();
        resolved.insert("a".into(), "<p>A</p>".into());
        resolved.insert("b".into(), "<p>B</p>".into());

        let html = render_tree(&[columns], &resolved);
        assert!(html.contains("vy-columns"));
        assert!(html.contains("--vy-cols:2"));
        // Children land inside the wrapper, in order.
        let a = html.find("<p>A</p>").expect("a rendered");
        let b = html.find("<p>B</p>").expect("b rendered");
        assert!(a < b);
        assert!(html.ends_with("</div>"));
    }

    #[test]
    fn a_scoped_section_carries_its_class() {
        let mut band = leaf("cta", "band");
        band.scope = Some(crate::scope::StyleScope {
            bg: Some(crate::scope::ColorRef::Value("$text".into())),
            ..Default::default()
        });
        let html = render_tree(&[band], &ResolvedSections::new());
        assert!(html.contains("vy-scope-cta"));
    }

    #[test]
    fn an_unresolved_leaf_leaves_no_empty_wrapper() {
        let html = render_tree(&[leaf("ghost", "hero")], &ResolvedSections::new());
        assert_eq!(html, "");
    }

    #[test]
    fn flat_chrome_only_layouts_do_not_compose() {
        assert!(!composes(&[
            leaf("header", "header"),
            leaf("footer", "footer")
        ]));
        assert!(composes(&[leaf("hero", "hero")]));

        let mut group = leaf("g", "group");
        group.children = vec![leaf("x", "header")];
        assert!(composes(&[group]));
    }
}
