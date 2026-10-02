//! Deterministic CSS compiler: [`TokenSet`] → custom properties.
//!
//! Output is a single stylesheet with:
//! - a `:root` block holding every token as a `--vy-*` custom property,
//! - dark overrides under both `@media (prefers-color-scheme: dark)` and
//!   `[data-theme="dark"]` (manual toggle wins over the media query),
//! - one `@font-face` rule per declared web font.
//!
//! The same input always produces byte-identical output, so themes can be
//! content-addressed and cached aggressively.

use std::fmt::Write as _;

use crate::tokens::{ColorSlot, FontChoice, TokenSet};

const ROLES: [&str; 7] = [
    "bg",
    "surface",
    "text",
    "text-muted",
    "border",
    "primary",
    "on-primary",
];

fn slot_for<'a>(tokens: &'a TokenSet, role: &str) -> Option<&'a ColorSlot> {
    match role {
        "bg" => Some(&tokens.colors.bg),
        "surface" => Some(&tokens.colors.surface),
        "text" => Some(&tokens.colors.text),
        "text-muted" => Some(&tokens.colors.text_muted),
        "border" => Some(&tokens.colors.border),
        "primary" => Some(&tokens.colors.primary),
        "on-primary" => Some(&tokens.colors.on_primary),
        _ => None,
    }
}

/// Compiles validated tokens into a CSS custom-property stylesheet.
///
/// Callers must run [`crate::tokens::validate_token_set`] first; compilation
/// itself is total and never fails.
#[must_use]
pub fn tokens_to_css(tokens: &TokenSet) -> String {
    let t = &tokens.typography;
    let mut out = String::from("/* Vyasa design tokens — generated; do not edit. */\n");

    out.push_str(":root {\n  color-scheme: light;\n");
    push_colors(&mut out, tokens, false);
    for line in [
        px_line("--vy-radius", tokens.radius_px),
        format!("  --vy-shadow: {};\n", tokens.shadow.css_value()),
        px_line("--vy-space-unit", tokens.spacing.unit_px),
        format!(
            "  --vy-section-scale:{};\n",
            tokens
                .spacing
                .section_scale
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        px_line("--vy-content-width", tokens.layout.content_width_px),
        px_line("--vy-sidebar-width", tokens.layout.sidebar_width_px),
        format!("  --vy-density: {};\n", tokens.layout.density.multiplier()),
        format!(
            "  --vy-direction: {};\n  direction: {};\n",
            tokens.direction.css_value(),
            tokens.direction.css_value()
        ),
        px_line("--vy-base-size", t.base_size_px),
        format!("  --vy-scale-ratio: {};\n", t.scale_ratio.value()),
        format!("  --vy-font-heading: {};\n", font_stack(&t.heading)),
        format!("  --vy-font-body: {};\n", font_stack(&t.body)),
    ] {
        out.push_str(&line);
    }
    out.push_str("}\n");

    let has_dark_overrides = ROLES
        .iter()
        .any(|r| slot_for(tokens, r).is_some_and(|s| s.dark.is_some()));
    if has_dark_overrides {
        // The media block is guarded so an explicit light choice beats a
        // dark OS. Without the guard, "light" in the studio's preview (or
        // a visitor-facing toggle) was a no-op on any dark machine: the
        // media query kept applying and there was nothing to override it.
        out.push_str(
            "@media (prefers-color-scheme: dark) {\n:root:not([data-theme=\"light\"]) {\n",
        );
        push_colors(&mut out, tokens, true);
        out.push_str("  color-scheme: dark;\n}\n}\n");
        out.push_str("[data-theme=\"dark\"] {\n");
        push_colors(&mut out, tokens, true);
        out.push_str("  color-scheme: dark;\n}\n");
    }

    for face in &t.font_faces {
        if let Some(src) = &face.src {
            let _ = write!(
                out,
                "@font-face {{\n  font-family: \"{}\";\n  src: url(\"{}\") format(\"woff2\");\n}}\n",
                css_string(&face.family),
                css_string(src)
            );
        }
    }
    out
}

/// Makes a value safe inside a double-quoted CSS string.
///
/// The stylesheet is inlined into a `<style>` element, so a font source of
/// `https://x") } </style><script>…` — which passes the "must start with
/// https://" check — closed the URL, the rule and the element and ran a
/// script on every page. Backslash and the quote are the two characters
/// that end a CSS string; `<` and `>` are escaped as code points so the
/// sequence `</style>` cannot appear in the output at all.
fn css_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '<' => out.push_str("\\3c "),
            '>' => out.push_str("\\3e "),
            '\n' | '\r' => {}
            other => out.push(other),
        }
    }
    out
}

fn px_line(name: &str, v: f32) -> String {
    format!("  {name}: {};\n", px(v))
}

/// A length as CSS, for the places a custom property will not do — media
/// queries, which cannot read one.
pub(crate) fn px(v: f32) -> String {
    format!("{v}px")
}

fn push_colors(out: &mut String, tokens: &TokenSet, dark: bool) {
    for role in ROLES {
        let Some(slot) = slot_for(tokens, role) else {
            continue;
        };
        let value = if dark {
            match slot.dark.as_ref() {
                Some(v) => v,
                None => continue,
            }
        } else {
            &slot.light
        };
        let _ = writeln!(out, "  --vy-color-{}: {};", role, value.normalized());
    }
}

/// The CSS `font-family` stack for a [`FontChoice`].
#[must_use]
pub fn font_stack(choice: &FontChoice) -> String {
    match choice {
        FontChoice::SystemUi => {
            "system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif".to_owned()
        }
        FontChoice::Serif => "Georgia, 'Times New Roman', Times, serif".to_owned(),
        FontChoice::Mono => "'SFMono-Regular', Consolas, 'Liberation Mono', monospace".to_owned(),
        FontChoice::Custom(name) => {
            // Family names are charset-validated upstream; quotes here are
            // purely defensive.
            let safe = name.replace(['"', '\\'], "");
            format!("\"{safe}\", system-ui, sans-serif")
        }
    }
}

/// Compiles every style scope in a layout into scoped custom properties.
///
/// Each scoped section gets a `.vy-scope-{id}` rule that redefines only the
/// roles it overrides. Because sections render inside one another, and custom
/// properties inherit, a nested section automatically picks up its ancestor's
/// scope unless it states its own — which is what makes "dark band containing
/// a card" behave the way an author expects without any cascade reasoning.
///
/// Output is deterministic: sections in document order, roles in
/// [`crate::scope::SCOPED_ROLES`] order.
#[must_use]
pub fn scopes_to_css(layout: &crate::layout::Layout, tokens: &TokenSet) -> String {
    scopes_css(layout, &[], tokens)
}

/// Scope rules for a theme layout together with one entry's own sections.
///
/// A composed page brings scopes the theme has never seen, so its rules have
/// to reach the same stylesheet — emitted after the theme's, and skipped on
/// an id the theme already claimed, so a page can never redefine a theme
/// section's appearance by reusing its id.
#[must_use]
pub fn scopes_css(
    layout: &crate::layout::Layout,
    page: &[crate::layout::Section],
    tokens: &TokenSet,
) -> String {
    use crate::layout::TemplateType;

    let mut out = String::new();
    let mut emitted: Vec<String> = Vec::new();

    for t in TemplateType::ALL {
        for top in layout.for_template(t) {
            push_tree(&mut out, top, tokens, &mut emitted);
        }
    }
    for top in page {
        push_tree(&mut out, top, tokens, &mut emitted);
    }

    if out.is_empty() {
        out
    } else {
        format!("/* Vyasa section scopes — generated; do not edit. */\n{out}")
    }
}

/// Emits every scope in one section's subtree, once per id.
fn push_tree(
    out: &mut String,
    top: &crate::layout::Section,
    tokens: &TokenSet,
    emitted: &mut Vec<String>,
) {
    for section in top.walk() {
        let Some(scope) = section.scope.as_ref() else {
            continue;
        };
        if scope.is_empty() || emitted.contains(&section.id) {
            continue;
        }
        emitted.push(section.id.clone());
        push_scope(out, &section.id, scope, tokens);
    }
}

fn push_scope(out: &mut String, id: &str, scope: &crate::scope::StyleScope, tokens: &TokenSet) {
    let mut light = String::new();
    let mut dark = String::new();

    for (role, _) in scope.colors() {
        let Some(slot) = scope.resolved(tokens, role) else {
            continue;
        };
        let _ = writeln!(light, "  --vy-color-{}: {};", role, slot.light.normalized());
        if let Some(d) = slot.dark.as_ref() {
            let _ = writeln!(dark, "  --vy-color-{}: {};", role, d.normalized());
        }
    }

    if let Some(padding) = scope.padding_y {
        let _ = writeln!(
            light,
            "  padding-block: calc(var(--vy-space-unit) * {padding});"
        );
    }
    if scope.contained == Some(true) {
        let _ = writeln!(
            light,
            "  max-width: var(--vy-content-width);\n  margin-inline: auto;"
        );
    }

    if !light.is_empty() {
        let _ = write!(
            out,
            ".vy-scope-{id} {{\n  background: var(--vy-color-bg);\n  color: var(--vy-color-text);\n{light}}}\n"
        );
    }
    if !dark.is_empty() {
        // Same guard as the root palette: explicit light wins on a dark OS.
        let _ = write!(
            out,
            "@media (prefers-color-scheme: dark) {{\n:root:not([data-theme=\"light\"]) .vy-scope-{id} {{\n{dark}}}\n}}\n[data-theme=\"dark\"] .vy-scope-{id} {{\n{dark}}}\n"
        );
    }
}

#[cfg(test)]
mod font_src_tests {
    use super::css_string;

    #[test]
    fn a_font_source_cannot_break_out_of_the_stylesheet() {
        // Passed the "starts with https://" check and closed the url(), the
        // rule, and the <style> element on every page of the site.
        let hostile = r#"https://x") } </style><script>alert(1)</script>"#;
        let safe = css_string(hostile);
        assert!(!safe.contains("</style>"), "{safe}");
        assert!(!safe.contains('<'), "{safe}");
        // Every quote left must be escaped: strip the escaped ones and none
        // may remain, or one of them closes url().
        assert!(
            !safe.replace("\\\"", "").contains('"'),
            "an unescaped quote: {safe}"
        );
        assert!(safe.starts_with("https://x"), "{safe}");
    }

    #[test]
    fn an_ordinary_source_is_untouched() {
        let src = "https://fonts.example/inter-var.woff2?v=3&w=400";
        assert_eq!(css_string(src), src);
    }
}
