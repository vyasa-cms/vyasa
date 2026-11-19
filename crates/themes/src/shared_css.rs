//! The base and highlight stylesheets as files, not inline text.
//!
//! Both depend only on the theme's tokens (its two breakpoints and its code
//! palette), never on the page, so one minified file per theme version serves
//! every page. The file name carries a hash of the bytes, so a visitor's
//! cache can keep it for a year and a changed theme gets a new address.
//! What does depend on the page, [`crate::css::scopes_css`], stays inline.

use crate::tokens::TokenSet;

const BASE_PREFIX: &str = "vy-base-";
const HIGHLIGHT_PREFIX: &str = "vy-hl-";

/// One stylesheet file: its name under `/theme-assets/` and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    /// File name, for example `vy-base-0123456789abcdef.css`.
    pub name: String,
    /// Minified stylesheet text.
    pub body: String,
}

impl Sheet {
    fn new(prefix: &str, css: &str) -> Self {
        let body = minify(css);
        Self {
            name: format!("{prefix}{:016x}.css", fnv1a(body.as_bytes())),
            body,
        }
    }

    /// The URL a page links.
    #[must_use]
    pub fn url(&self) -> String {
        format!("/theme-assets/{}", self.name)
    }
}

/// The grid, component and section rules every page uses.
#[must_use]
pub fn base_sheet(tokens: &TokenSet) -> Sheet {
    let css = format!(
        "{}{}",
        crate::base_css::base_css(tokens),
        crate::sections::sections_css(tokens)
    );
    Sheet::new(BASE_PREFIX, &css)
}

/// The syntax-highlighting rules, needed only by pages holding code.
#[must_use]
pub fn highlight_sheet(tokens: &TokenSet) -> Sheet {
    let css = format!(
        "{}{}",
        crate::renderer::highlight::highlight_css(),
        crate::css::code_palette_css(tokens)
    );
    Sheet::new(HIGHLIGHT_PREFIX, &css)
}

/// The sheet of this theme with the given file name, if it is one.
#[must_use]
pub fn find(tokens: &TokenSet, name: &str) -> Option<Sheet> {
    let sheet = if name.starts_with(BASE_PREFIX) {
        base_sheet(tokens)
    } else if name.starts_with(HIGHLIGHT_PREFIX) {
        highlight_sheet(tokens)
    } else {
        return None;
    };
    (sheet.name == name).then_some(sheet)
}

/// 64-bit FNV-1a: stable across builds, which a std hasher does not promise.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Drops comments and the whitespace that carries no meaning.
///
/// Conservative on purpose: strings and `url(...)` are copied untouched, a
/// run of whitespace becomes one space, and a space is removed only beside
/// `{ } ; ,` and after `:`. Spaces around `+ - * /` (in `calc`), `>`, `~`
/// and before `:` (a descendant selector) are kept.
#[must_use]
pub fn minify(css: &str) -> String {
    let chars: Vec<char> = css.chars().collect();
    let mut out = String::with_capacity(css.len() / 2);
    let mut i = 0;
    let mut pending_space = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            pending_space = true;
            continue;
        }
        if c.is_whitespace() {
            pending_space = true;
            i += 1;
            continue;
        }
        let tight_before = matches!(c, '{' | '}' | ';' | ',');
        let tight_after = out.ends_with(['{', '}', ';', ',', ':']) || out.is_empty();
        if pending_space && !tight_before && !tight_after {
            out.push(' ');
        }
        pending_space = false;
        if c == '}' && out.ends_with(';') {
            out.pop();
        }
        if c == '"' || c == '\'' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            out.extend(&chars[start..i.min(chars.len())]);
            continue;
        }
        if c == 'u' && chars[i..].starts_with(&['u', 'r', 'l', '(']) {
            let start = i;
            while i < chars.len() && chars[i] != ')' {
                i += 1;
            }
            i += 1;
            out.extend(&chars[start..i.min(chars.len())]);
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_indentation_go() {
        let css = "/* c */\n.a, .b {\n  color: red;\n  margin: 0 auto;\n}\n";
        assert_eq!(minify(css), ".a,.b{color:red;margin:0 auto}");
    }

    #[test]
    fn meaningful_spaces_stay() {
        assert_eq!(
            minify("a :hover > b { width: calc(1px + 2px); }"),
            "a :hover > b{width:calc(1px + 2px)}"
        );
    }

    #[test]
    fn strings_and_urls_are_untouched() {
        let css = "a::after { content: \"a  /* b */  c\"; background: url(data:image/svg+xml;utf8,<svg  a='1'/>); }";
        assert_eq!(
            minify(css),
            "a::after{content:\"a  /* b */  c\";background:url(data:image/svg+xml;utf8,<svg  a='1'/>)}"
        );
    }

    #[test]
    fn the_base_sheet_is_much_smaller_and_found_by_its_name() {
        let tokens = TokenSet::default();
        let sheet = base_sheet(&tokens);
        let inline = format!(
            "{}{}",
            crate::base_css::base_css(&tokens),
            crate::sections::sections_css(&tokens)
        );
        assert!(
            sheet.body.len() < inline.len() * 3 / 4,
            "{} vs {}",
            sheet.body.len(),
            inline.len()
        );
        assert!(!sheet.body.contains("/*"));
        assert_eq!(find(&tokens, &sheet.name), Some(sheet.clone()));
        assert_eq!(find(&tokens, "vy-base-0000000000000000.css"), None);
        assert_eq!(find(&tokens, "other.css"), None);
        assert_eq!(base_sheet(&tokens), sheet, "same tokens, same address");
    }

    #[test]
    fn breakpoints_change_the_address() {
        let mut tokens = TokenSet::default();
        let before = base_sheet(&tokens).name;
        tokens.layout.breakpoint_sm_px += 40.0;
        assert_ne!(base_sheet(&tokens).name, before);
    }

    #[test]
    fn the_highlight_sheet_is_found_by_its_name() {
        let tokens = TokenSet::default();
        let sheet = highlight_sheet(&tokens);
        assert!(sheet.body.contains(".vy-hl-") || sheet.body.contains("vy-hl"));
        assert_eq!(find(&tokens, &sheet.name), Some(sheet));
    }
}
