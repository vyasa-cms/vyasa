//! A deliberately small YAML subset: `key: value` lines between `---`
//! fences. No anchors, no nesting, no multi-line scalars — a page that
//! needs them imports with a warning instead of pulling a YAML parser
//! (and its attack surface) into a 64 MiB sandbox.

use std::collections::BTreeMap;

/// What a page's frontmatter says.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    /// `title`.
    pub title: Option<String>,
    /// `description`.
    pub description: Option<String>,
    /// `position`, `order`, `sidebar_position` or `weight`.
    pub position: Option<i64>,
    /// `version`.
    pub version: Option<String>,
    /// `hide`, `hidden` or `hide_from_nav`.
    pub hide: bool,
    /// `slug`.
    pub slug: Option<String>,
    /// `section` or `group`: the sidebar group label.
    pub section: Option<String>,
    /// `updated`, `last_updated` or `lastmod`, as written.
    pub updated: Option<String>,
    /// Every other key, as written.
    pub extra: BTreeMap<String, String>,
}

/// The result of splitting a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split<'a> {
    /// Parsed frontmatter (all defaults when the page has none).
    pub front: Frontmatter,
    /// The Markdown after the closing fence.
    pub body: &'a str,
    /// Lines that were ignored, for the sync report.
    pub warnings: Vec<String>,
}

/// Splits `src` into frontmatter and body.
#[must_use]
pub fn split(src: &str) -> Split<'_> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut lines = src.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        return Split {
            front: Frontmatter::default(),
            body: src,
            warnings: Vec::new(),
        };
    }
    let mut offset = first.len();
    let mut block = Vec::new();
    let mut closed = false;
    for line in lines {
        offset += line.len();
        let t = line.trim_end();
        if t == "---" || t == "..." {
            closed = true;
            break;
        }
        block.push(t.to_owned());
    }
    if !closed {
        // An opening fence with no closing one is a horizontal rule, not
        // frontmatter.
        return Split {
            front: Frontmatter::default(),
            body: src,
            warnings: Vec::new(),
        };
    }
    let mut warnings = Vec::new();
    let front = parse_block(&block, &mut warnings);
    Split {
        front,
        body: &src[offset..],
        warnings,
    }
}

fn parse_block(lines: &[String], warnings: &mut Vec<String>) -> Frontmatter {
    let mut f = Frontmatter::default();
    for raw in lines {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') || line.starts_with('-') {
            warnings.push(format!(
                "frontmatter: nested or list value ignored: {}",
                line.trim()
            ));
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            warnings.push(format!("frontmatter: not a key: value line: {line}"));
            continue;
        };
        let key = key.trim().to_ascii_lowercase().replace('-', "_");
        let value = unquote(value.trim());
        match key.as_str() {
            "title" => f.title = non_empty(value),
            "description" => f.description = non_empty(value),
            "position" | "order" | "sidebar_position" | "weight" => match value.parse::<i64>() {
                Ok(n) => f.position = Some(n),
                Err(_) => {
                    warnings.push(format!("frontmatter: {key} is not a whole number: {value}"))
                }
            },
            "version" => f.version = non_empty(value),
            "hide" | "hidden" | "hide_from_nav" => {
                f.hide = matches!(value.as_str(), "true" | "yes" | "on")
            }
            "slug" => f.slug = non_empty(value),
            "section" | "group" => f.section = non_empty(value),
            "updated" | "last_updated" | "lastmod" => f.updated = non_empty(value),
            _ => {
                f.extra.insert(key, value);
            }
        }
    }
    f
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

fn unquote(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        let inner = &v[1..v.len() - 1];
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some(other) => out.push(other),
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        return out;
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    // A bare value ends at an inline comment.
    match v.find(" #") {
        Some(at) => v[..at].trim_end().to_owned(),
        None => v.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_without_frontmatter_is_all_body() {
        let s = split("# Hi\n\ntext\n");
        assert_eq!(s.front, Frontmatter::default());
        assert_eq!(s.body, "# Hi\n\ntext\n");
    }

    #[test]
    fn known_keys_parse_and_the_body_follows_the_fence() {
        let s = split("---\ntitle: \"Install: fast\"\ndescription: Get going # trailing\nposition: 3\nversion: v2\nhide: true\nslug: install\nsection: Basics\ncustom: x\n---\nBody\n");
        assert_eq!(s.front.title.as_deref(), Some("Install: fast"));
        assert_eq!(s.front.description.as_deref(), Some("Get going"));
        assert_eq!(s.front.position, Some(3));
        assert_eq!(s.front.version.as_deref(), Some("v2"));
        assert!(s.front.hide);
        assert_eq!(s.front.slug.as_deref(), Some("install"));
        assert_eq!(s.front.section.as_deref(), Some("Basics"));
        assert_eq!(s.front.extra.get("custom").map(String::as_str), Some("x"));
        assert_eq!(s.body, "Body\n");
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn aliases_and_single_quotes() {
        let s = split("---\nsidebar_position: 7\nhidden: yes\ntitle: 'It''s here'\n---\n");
        assert_eq!(s.front.position, Some(7));
        assert!(s.front.hide);
        assert_eq!(s.front.title.as_deref(), Some("It's here"));
    }

    #[test]
    fn nested_values_and_bad_numbers_warn_and_do_not_fail() {
        let s = split("---\ntags:\n  - a\nposition: soon\n---\nx");
        assert_eq!(s.front.position, None);
        assert_eq!(s.warnings.len(), 2, "{:?}", s.warnings);
        assert_eq!(s.body, "x");
    }

    #[test]
    fn an_unclosed_fence_is_not_frontmatter() {
        let s = split("---\ntitle: nope\n\nNo closing fence");
        assert_eq!(s.front, Frontmatter::default());
        assert!(s.body.starts_with("---"));
    }

    #[test]
    fn a_byte_order_mark_and_crlf_are_tolerated() {
        let s = split("\u{feff}---\r\ntitle: A\r\n---\r\nB\r\n");
        assert_eq!(s.front.title.as_deref(), Some("A"));
        assert_eq!(s.body, "B\r\n");
    }
}
