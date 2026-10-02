//! Server-side syntax highlighting for `code` blocks.
//!
//! The site ships no JavaScript for visitors, so highlighting happens
//! here: syntect emits class-annotated spans at render time and the
//! palette rides the theme stylesheet — one CSS generated from a light
//! syntect theme, one from a dark, wrapped in the same dark-mode guards
//! every other color obeys. An unknown language falls back to the plain
//! escaped block it always was.

use std::sync::OnceLock;

use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Class prefix, so highlight classes can never collide with a theme's.
const STYLE: ClassStyle = ClassStyle::SpacedPrefixed { prefix: "vy-hl-" };

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Highlighted `<span>` soup for `code`, or `None` when the language is
/// unknown (caller keeps its plain path) or highlighting fails.
#[must_use]
pub fn highlight(code: &str, language: &str) -> Option<String> {
    let set = syntaxes();
    let syntax = set
        .find_syntax_by_token(language)
        .or_else(|| set.find_syntax_by_extension(language))?;
    let mut generator = ClassedHTMLGenerator::new_with_class_style(syntax, set, STYLE);
    for line in LinesWithEndings::from(code) {
        generator
            .parse_html_for_line_which_includes_newline(line)
            .ok()?;
    }
    Some(generator.finalize())
}

/// The palette for those classes: light theme at the root, dark theme
/// under the same guards the token CSS uses (OS preference unless the
/// visitor chose, explicit choice always wins).
#[must_use]
pub fn highlight_css() -> &'static str {
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        let themes = syntect::highlighting::ThemeSet::load_defaults();
        let light = themes
            .themes
            .get("InspiredGitHub")
            .and_then(|t| syntect::html::css_for_theme_with_class_style(t, STYLE).ok())
            .unwrap_or_default();
        let dark = themes
            .themes
            .get("base16-ocean.dark")
            .and_then(|t| syntect::html::css_for_theme_with_class_style(t, STYLE).ok())
            .unwrap_or_default();
        // The generated CSS styles `.code` and body-level selectors too;
        // scope everything under .vy-code so it cannot restyle the page.
        let scope = |css: &str| -> String {
            css.lines()
                .map(|line| {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with('.') {
                        format!(".vy-code {line}")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let light = scope(&light);
        let dark = scope(&dark);
        format!(
            "/* syntax highlighting (generated) */\n{light}\n\
             @media (prefers-color-scheme: dark) {{\n\
             :root:not([data-theme=\"light\"]) {{ }}\n{dark_media}\n}}\n\
             {dark_forced}\n",
            dark_media = prefix_selectors(&dark, ":root:not([data-theme=\"light\"])"),
            dark_forced = prefix_selectors(&dark, "[data-theme=\"dark\"]"),
        )
    })
}

/// Prefixes every top-level selector line of simple generated CSS.
fn prefix_selectors(css: &str, prefix: &str) -> String {
    css.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('.') {
                format!("{prefix} {line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{highlight, highlight_css};

    #[test]
    fn known_languages_gain_spans_unknown_stay_none() {
        let html = highlight("fn main() {}", "rust").expect("rust is known");
        assert!(html.contains("<span class=\"vy-hl-"), "{html}");
        assert!(html.contains("main"), "{html}");
        assert!(highlight("x", "no-such-language").is_none());
    }

    #[test]
    fn the_palette_ships_light_and_dark_scoped_to_code() {
        let css = highlight_css();
        assert!(css.contains(".vy-code .vy-hl-"), "{}", &css[..300]);
        assert!(css.contains("@media (prefers-color-scheme: dark)"));
        assert!(css.contains("[data-theme=\"dark\"] .vy-code"));
    }

    #[test]
    fn highlighted_output_never_carries_raw_input_html() {
        let html = highlight("<script>alert(1)</script>", "html").expect("html known");
        assert!(!html.contains("<script>"), "{html}");
    }
}
