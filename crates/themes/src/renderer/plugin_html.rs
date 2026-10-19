//! Sanitising what plugins emit, and filling the child slots of plugin blocks.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

fn set<const N: usize>(items: [&'static str; N]) -> HashSet<&'static str> {
    items.into_iter().collect()
}

/// `role` values a plugin may use.
const ROLES: &[&str] = &[
    "tab",
    "tablist",
    "tabpanel",
    "navigation",
    "note",
    "region",
    "search",
    "list",
    "listitem",
    "group",
    "presentation",
    "status",
];

/// Form control names that would shadow a property of the form itself
/// (DOM clobbering): `<input name="action">` makes `form.action` the input.
const FORM_NAMES_DENIED: &[&str] = &[
    "action", "method", "submit", "reset", "elements", "length", "name", "id", "target", "enctype",
    "encoding",
];

fn allowed_tags() -> HashSet<&'static str> {
    set([
        "a",
        "p",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "ul",
        "ol",
        "li",
        "blockquote",
        "pre",
        "code",
        "strong",
        "em",
        "b",
        "i",
        "u",
        "s",
        "br",
        "hr",
        "span",
        "div",
        "img",
        "figure",
        "figcaption",
        "table",
        "thead",
        "tbody",
        "tr",
        "th",
        "td",
        "caption",
        "dl",
        "dt",
        "dd",
        "small",
        "time",
        "mark",
        "kbd",
        "sub",
        "sup",
        "abbr",
        "nav",
        "aside",
        "section",
        "header",
        "footer",
        "details",
        "summary",
        "button",
        "form",
        "input",
        "label",
    ])
}

fn allowed_tag_attributes() -> HashMap<&'static str, HashSet<&'static str>> {
    HashMap::from([
        ("a", set(["href", "title", "rel"])),
        (
            "img",
            set(["src", "alt", "title", "width", "height", "loading"]),
        ),
        ("time", set(["datetime"])),
        ("ol", set(["start"])),
        ("th", set(["colspan", "rowspan", "scope"])),
        ("td", set(["colspan", "rowspan"])),
        ("details", set(["open"])),
        ("button", set(["type"])),
        ("form", set(["action", "method"])),
        (
            "input",
            set(["type", "name", "value", "placeholder", "autocomplete"]),
        ),
        ("label", set(["for"])),
        ("abbr", set(["title"])),
    ])
}

/// Decides one attribute: keep it, rewrite it, or drop it (`None`).
fn filter_attribute<'u>(element: &str, attribute: &str, value: &'u str) -> Option<Cow<'u, str>> {
    let v = value.trim();
    let keep = || Some(Cow::Owned(v.to_owned()));
    match (element, attribute) {
        (_, "id") => {
            let ok = !v.is_empty()
                && v.len() <= 64
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
            if !ok {
                None
            } else if v.starts_with("pl-") {
                keep()
            } else {
                Some(Cow::Owned(format!("pl-{v}")))
            }
        }
        (_, "role") => ROLES.contains(&v).then(keep).flatten(),
        (_, "tabindex") => matches!(v, "0" | "-1").then(keep).flatten(),
        ("form", "action") => (v.starts_with('/') && !v.starts_with("//") && !v.contains('\\'))
            .then(keep)
            .flatten(),
        ("input", "type") => matches!(v, "search" | "text" | "hidden")
            .then(keep)
            .flatten(),
        ("input", "name") => {
            let ok = !v.is_empty()
                && v.len() <= 32
                && v.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_')
                })
                && !FORM_NAMES_DENIED.contains(&v);
            ok.then(keep).flatten()
        }
        ("input", "value") if v.len() > 256 => None,
        ("a", "rel") => Some(Cow::Owned(safe_rel(v))),
        _ => Some(Cow::Borrowed(value)),
    }
}

/// Link relations a plugin may set; anything else is dropped.
const RELS: &[&str] = &["prev", "next", "nofollow", "ugc", "sponsored", "external"];

/// Keeps the allowed relations a plugin wrote and always adds
/// `noopener noreferrer`, so a pager can say `prev`/`next` without
/// giving up the safe default.
fn safe_rel(value: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for token in value.split_ascii_whitespace() {
        let token = RELS.iter().find(|r| r.eq_ignore_ascii_case(token));
        if let Some(t) = token {
            if !out.contains(t) {
                out.push(t);
            }
        }
    }
    out.extend(["noopener", "noreferrer"]);
    out.join(" ")
}

/// Sanitizes HTML produced by a plugin block or section.
///
/// The allowlist of the raw-HTML block plus what a plugin needs to ship
/// ordinary page furniture: `class`; landmarks and disclosure (`nav`,
/// `aside`, `details`, `summary`); `button`s that cannot submit anything; a
/// GET `form` that can only go to this site; `id`, `role`, `aria-*` and
/// `data-*` so tabs and trees can be built and enhanced by the plugin's own
/// script. Anything that carries behaviour on its own stays out: `on*`,
/// `style`, `srcset`, `script`, `iframe`, `object`.
///
/// Attributes that could be abused are rewritten rather than trusted:
/// - `id` is prefixed `pl-`, so a plugin cannot shadow an element or a
///   global the theme's script looks up by id;
/// - a `button` is always `type="button"`;
/// - a `form` is always `method="get"` and keeps an `action` only when it is
///   a path on this site.
#[must_use]
pub fn sanitize_plugin_html(html: &str) -> String {
    let mut builder = ammonia::Builder::default();
    builder
        .tags(allowed_tags())
        .tag_attributes(allowed_tag_attributes())
        .generic_attributes(set(["class", "id", "role", "lang", "dir", "tabindex"]))
        .generic_attribute_prefixes(set(["aria-", "data-"]))
        .set_tag_attribute_value("button", "type", "button")
        .set_tag_attribute_value("form", "method", "get")
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(None)
        .attribute_filter(filter_attribute);
    add_default_rel(&builder.clean(html).to_string())
}

/// Gives every link that has no `rel` the safe default. The filter above
/// only sees attributes that exist, so a bare `<a>` is handled here, on
/// the sanitiser's own output, where `<a` is followed by a space and
/// `>` inside an attribute value is escaped as `&gt;`.
fn add_default_rel(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<a ") {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        let end = tail.find('>').map_or(tail.len(), |e| e + 1);
        let tag = &tail[..end];
        if tag.contains(" rel=\"") || !tag.ends_with('>') {
            out.push_str(tag);
        } else {
            out.push_str(&tag[..tag.len() - 1]);
            out.push_str(" rel=\"noopener noreferrer\">");
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Replaces `[[vy-child-N]]` in a plugin block's sanitised HTML with the
/// N-th rendered child.
///
/// Children are the document's own blocks, rendered (and sanitised) by the
/// core renderer, so putting them inside markup a plugin wrote adds no new
/// trust. A token naming a child that does not exist is removed. If the
/// plugin placed no token at all, the children follow its markup rather
/// than disappear: a plugin that forgot slots must not eat content.
#[must_use]
pub fn fill_child_slots(html: &str, children: &[String]) -> String {
    const OPEN: &str = "[[vy-child-";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let mut placed = false;
    while let Some(at) = rest.find(OPEN) {
        out.push_str(&rest[..at]);
        let after = &rest[at + OPEN.len()..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        match (
            digits.parse::<usize>(),
            after[digits.len()..].strip_prefix("]]"),
        ) {
            (Ok(n), Some(tail)) if !digits.is_empty() => {
                if let Some(child) = children.get(n) {
                    out.push_str(child);
                }
                placed = true;
                rest = tail;
            }
            _ => {
                out.push_str(OPEN);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    if !placed {
        for c in children {
            out.push_str(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_docs_blocks_emit_survives() {
        let html = r#"<div class="docs-tabs" data-docs-tabs data-group="pkg"><div class="docs-tabs__list" role="tablist"><button type="button" class="docs-tabs__tab is-active" role="tab" aria-selected="true" data-tab="0" data-label="npm">npm</button></div><div class="docs-tabs__panel" role="tabpanel" data-panel="0"><p class="docs-tabs__label">npm</p>[[vy-child-0]]</div></div>"#;
        let out = sanitize_plugin_html(html);
        for needle in [
            "role=\"tablist\"",
            "role=\"tab\"",
            "aria-selected=\"true\"",
            "data-tab=\"0\"",
            "data-group=\"pkg\"",
            "class=\"docs-tabs__tab is-active\"",
            "[[vy-child-0]]",
            "<button",
        ] {
            assert!(out.contains(needle), "{needle} missing from {out}");
        }
    }

    #[test]
    fn navigation_markup_survives() {
        let html = r#"<details class="docs-drawer" open><summary class="docs-drawer__toggle">Menu</summary><nav class="docs-nav" aria-label="Documentation"><ul><li><a class="is-current" href="/docs/a" aria-current="page">A</a></li></ul></nav></details>"#;
        let out = sanitize_plugin_html(html);
        for needle in [
            "<details",
            " open",
            "<summary",
            "<nav",
            "aria-label=\"Documentation\"",
            "aria-current=\"page\"",
            "href=\"/docs/a\"",
        ] {
            assert!(out.contains(needle), "{needle} missing from {out}");
        }
    }

    #[test]
    fn behaviour_is_removed() {
        let html = r#"<div onclick="x()" style="position:fixed" class="ok"><script>alert(1)</script><iframe src="//e"></iframe><a href="javascript:alert(1)">x</a><img src="a.png" onerror="y()" srcset="z"><object data="x"></object></div>"#;
        let out = sanitize_plugin_html(html);
        for bad in [
            "onclick",
            "style=",
            "<script",
            "alert(1)",
            "<iframe",
            "javascript:",
            "onerror",
            "srcset",
            "<object",
        ] {
            assert!(!out.contains(bad), "{bad} survived in {out}");
        }
        assert!(out.contains("class=\"ok\""));
    }

    #[test]
    fn ids_are_prefixed_and_roles_are_allowlisted() {
        let out = sanitize_plugin_html(
            r#"<div id="x" role="tab"></div><div id="pl-y" role="button"></div><div id="has space"></div>"#,
        );
        assert!(out.contains("id=\"pl-x\""), "{out}");
        assert!(out.contains("id=\"pl-y\""), "{out}");
        assert!(!out.contains("role=\"button\""), "{out}");
        assert!(!out.contains("has space"), "{out}");
    }

    #[test]
    fn buttons_never_submit_and_forms_stay_on_the_site() {
        let out = sanitize_plugin_html(
            r#"<form action="/docs/search" method="post"><input type="search" name="q"><input type="file" name="f"><input name="action"><button type="submit">Go</button></form>"#,
        );
        assert!(out.contains("action=\"/docs/search\""), "{out}");
        assert!(out.contains("method=\"get\""), "{out}");
        assert!(out.contains("type=\"button\""), "{out}");
        assert!(!out.contains("type=\"submit\""), "{out}");
        assert!(!out.contains("type=\"file\""), "{out}");
        assert!(!out.contains("name=\"action\""), "{out}");
        for evil in ["https://evil.example/", "//evil.example/", "javascript:x"] {
            let out =
                sanitize_plugin_html(&format!(r#"<form action="{evil}"><input name="q"></form>"#));
            assert!(
                !out.contains("evil") && !out.contains("javascript"),
                "{evil}: {out}"
            );
        }
    }

    #[test]
    fn tabindex_only_zero_or_minus_one() {
        let out = sanitize_plugin_html(r#"<div tabindex="0"></div><div tabindex="5"></div>"#);
        assert_eq!(out.matches("tabindex").count(), 1, "{out}");
    }

    #[test]
    fn links_get_a_safe_rel() {
        assert!(sanitize_plugin_html(r#"<a href="https://x.test">x</a>"#)
            .contains("rel=\"noopener noreferrer\""));
        assert_eq!(
            sanitize_plugin_html(r#"<a href="/x" title="a > b">x</a>"#),
            r#"<a href="/x" title="a &gt; b" rel="noopener noreferrer">x</a>"#
        );
    }

    #[test]
    fn allowed_rels_survive_and_others_are_dropped() {
        let out = sanitize_plugin_html(
            r#"<a href="/a" rel="prev">a</a><a href="/b" rel="NEXT opener me">b</a>"#,
        );
        assert!(out.contains(r#"rel="prev noopener noreferrer""#), "{out}");
        assert!(out.contains(r#"rel="next noopener noreferrer""#), "{out}");
        assert!(!out.contains("opener me"), "{out}");
        assert_eq!(out.matches("rel=").count(), 2, "{out}");
    }
}

#[cfg(test)]
mod slot_tests {
    use super::fill_child_slots;

    fn kids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn slots_are_filled_in_place() {
        let out = fill_child_slots(
            "<ol><li>[[vy-child-0]]</li><li>[[vy-child-1]]</li></ol>",
            &kids(&["<p>a</p>", "<p>b</p>"]),
        );
        assert_eq!(out, "<ol><li><p>a</p></li><li><p>b</p></li></ol>");
    }

    #[test]
    fn a_slot_for_a_missing_child_is_removed_and_order_is_free() {
        assert_eq!(
            fill_child_slots(
                "[[vy-child-1]]|[[vy-child-0]]|[[vy-child-7]]",
                &kids(&["A", "B"])
            ),
            "B|A|"
        );
    }

    #[test]
    fn children_without_slots_follow_the_markup() {
        assert_eq!(
            fill_child_slots("<div>x</div>", &kids(&["<p>a</p>"])),
            "<div>x</div><p>a</p>"
        );
        assert_eq!(fill_child_slots("<div>x</div>", &[]), "<div>x</div>");
    }

    #[test]
    fn malformed_tokens_are_left_alone() {
        assert_eq!(
            fill_child_slots("[[vy-child-]] [[vy-child-x]] [[vy-child-1", &kids(&["A"])),
            "[[vy-child-]] [[vy-child-x]] [[vy-child-1A"
        );
    }
}
