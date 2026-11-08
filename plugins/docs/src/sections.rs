//! Designer sections and the default layout for the `doc` type.

use serde_json::{json, Value};

use crate::nav;
use crate::settings::Settings;

/// The section kinds this plugin owns.
pub const KINDS: &[&str] = &[
    "docs/sidebar",
    "docs/toc",
    "docs/pager",
    "docs/breadcrumbs",
    "docs/search",
    "docs/version-banner",
];

fn section(kind: &str, title: &str, description: &str, schema: &Value, sample: &Value) -> Value {
    json!({
        "kind": kind,
        "title": title,
        "category": "navigation",
        "description": description,
        "settings-schema": schema,
        "sample": sample,
        "binds": false,
        "inline": [],
        // Asks the host for the entry, the published tree of its type and
        // the page's headings (docs/plugin-api.md, "Section context").
        "context": ["entry", "tree", "toc"],
    })
}

/// The `register-sections` reply.
#[must_use]
pub fn descriptors() -> Value {
    let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
    json!([
        section(
            "docs/sidebar",
            "Docs sidebar",
            "The documentation tree, with the current page marked and a version switcher.",
            &none,
            &json!({})
        ),
        section(
            "docs/toc",
            "On this page",
            "The page's h2 and h3 headings.",
            &none,
            &json!({})
        ),
        section(
            "docs/pager",
            "Previous and next",
            "Links to the neighbouring pages in reading order.",
            &none,
            &json!({})
        ),
        section(
            "docs/breadcrumbs",
            "Docs breadcrumbs",
            "Where the page sits in the tree.",
            &none,
            &json!({})
        ),
        section(
            "docs/search",
            "Docs search",
            "A search box scoped to the documentation and the version being read.",
            &json!({"type": "object", "properties": {"placeholder": {"type": "string", "maxLength": 80}}, "additionalProperties": false}),
            &json!({"placeholder": "Search the docs"}),
        ),
        section(
            "docs/version-banner",
            "Version notice",
            "A notice on pages of an older version, linking to the latest.",
            &none,
            &json!({})
        ),
    ])
}

/// The default layout tree for the `doc` type: three columns on a wide
/// screen, one on a phone with the sidebar and the contents as drawers.
#[must_use]
pub fn default_layouts() -> Value {
    json!({
        "doc": [
            {"id": "docs-banner", "kind": "docs/version-banner"},
            {"id": "docs-layout", "kind": "columns", "settings": {"count": 3, "count_tablet": 1, "count_mobile": 1},
             "children": [
                {"id": "docs-side", "kind": "group", "children": [
                    {"id": "docs-search", "kind": "docs/search"},
                    {"id": "docs-sidebar", "kind": "docs/sidebar"}
                ]},
                {"id": "docs-main", "kind": "group", "children": [
                    {"id": "docs-crumbs", "kind": "docs/breadcrumbs"},
                    {"id": "docs-content", "kind": "post-content"},
                    {"id": "docs-pager", "kind": "docs/pager"}
                ]},
                {"id": "docs-aside", "kind": "group", "children": [
                    {"id": "docs-toc", "kind": "docs/toc"}
                ]}
            ]}
        ]
    })
}

/// Renders one section. An unchanged payload means "not mine, or nothing
/// to show", which the host reads as such.
#[must_use]
pub fn render(payload: &str, settings: &Settings) -> String {
    let Ok(req) = serde_json::from_str::<Value>(payload) else {
        return payload.to_owned();
    };
    let Some(kind) = req["kind"].as_str() else {
        return payload.to_owned();
    };
    if !KINDS.contains(&kind) {
        return payload.to_owned();
    }
    let Some(page) = nav::parse(&req) else {
        // No entry in play: the studio previewing a template, or a page that is not a doc.
        return if req["editor"].as_bool().unwrap_or(false) {
            let label = descriptors()
                .as_array()
                .and_then(|a| {
                    a.iter()
                        .find(|d| d["kind"] == kind)
                        .map(|d| d["title"].as_str().unwrap_or("").to_owned())
                })
                .unwrap_or_default();
            format!(
                "<div class=\"docs-placeholder\">{} appears here on each documentation page.</div>",
                crate::blocks::esc(&label)
            )
        } else {
            payload.to_owned()
        };
    };
    let html = match kind {
        "docs/sidebar" => nav::sidebar(&page, &settings.latest_label),
        "docs/toc" => nav::toc(&page),
        "docs/pager" => nav::pager(&page),
        "docs/breadcrumbs" => nav::breadcrumbs(&page, &settings.docs_title),
        "docs/version-banner" => nav::banner(&page, &settings.latest_label),
        _ => {
            let placeholder = req["settings"]["placeholder"]
                .as_str()
                .filter(|p| !p.is_empty())
                .unwrap_or(&settings.search_placeholder);
            nav::search(&page, &settings.latest_version, placeholder)
        }
    };
    if html.is_empty() {
        payload.to_owned()
    } else {
        html
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(kind: &str, editor: bool, with_context: bool) -> String {
        let mut v = json!({"kind": kind, "settings": {}, "bound": [], "editor": editor});
        if with_context {
            v["context"] = json!({
                "entry": {"id": "2", "ancestors": ["1"]},
                "toc": [{"level": 2, "anchor": "#vy-h-1", "text": "A"}, {"level": 2, "anchor": "#vy-h-2", "text": "B"}],
                "tree": [
                    {"id": "1", "parent_id": null, "position": 10, "title": "Guides", "slug": "guides", "url": "/docs/guides", "fields": {}},
                    {"id": "2", "parent_id": "1", "position": 10, "title": "Install", "slug": "install", "url": "/docs/guides/install", "fields": {}}
                ]
            });
        }
        v.to_string()
    }

    #[test]
    fn every_kind_is_declared_once_with_a_namespace_and_the_context_it_needs() {
        let d = descriptors();
        let kinds: Vec<_> = d
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, KINDS);
        for s in d.as_array().unwrap() {
            assert!(s["kind"].as_str().unwrap().starts_with("docs/"));
            assert_eq!(s["category"], "navigation");
            assert_eq!(s["context"], json!(["entry", "tree", "toc"]));
            assert_eq!(s["binds"], false);
        }
    }

    #[test]
    fn the_default_layout_uses_only_known_kinds_with_unique_ids() {
        let tree = default_layouts();
        let mut ids = Vec::new();
        let mut kinds = Vec::new();
        fn walk(v: &Value, ids: &mut Vec<String>, kinds: &mut Vec<String>) {
            for s in v.as_array().unwrap() {
                ids.push(s["id"].as_str().unwrap().to_owned());
                kinds.push(s["kind"].as_str().unwrap().to_owned());
                if let Some(c) = s.get("children") {
                    walk(c, ids, kinds);
                }
            }
        }
        walk(&tree["doc"], &mut ids, &mut kinds);
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
        for k in &kinds {
            assert!(
                KINDS.contains(&k.as_str())
                    || ["columns", "group", "post-content"].contains(&k.as_str()),
                "{k}"
            );
        }
        assert!(kinds.contains(&"post-content".to_owned()));
        assert!(
            ids.contains(&"docs-layout".to_owned()),
            "the stylesheet keys its grid on this id"
        );
    }

    #[test]
    fn rendering_dispatches_and_unchanged_means_not_mine() {
        let s = Settings::default();
        assert!(render(&payload("docs/sidebar", false, true), &s).contains("docs-nav"));
        assert!(render(&payload("docs/pager", false, true), &s).contains("rel=\"prev\""));
        assert!(render(&payload("docs/breadcrumbs", false, true), &s).contains("Guides"));
        assert!(
            render(&payload("docs/search", false, true), &s).contains("action=\"/docs/search\"")
        );
        let foreign = payload("store/grid", false, true);
        assert_eq!(render(&foreign, &s), foreign);
        assert_eq!(render("not json", &s), "not json");
    }

    #[test]
    fn without_an_entry_the_studio_gets_a_placeholder_and_visitors_nothing() {
        let s = Settings::default();
        assert!(render(&payload("docs/sidebar", true, false), &s).contains("docs-placeholder"));
        let p = payload("docs/sidebar", false, false);
        assert_eq!(render(&p, &s), p);
    }

    #[test]
    fn the_search_section_setting_overrides_the_site_wide_placeholder() {
        let s = Settings::default();
        let mut v: Value = serde_json::from_str(&payload("docs/search", false, true)).unwrap();
        v["settings"]["placeholder"] = json!("Find a guide");
        assert!(render(&v.to_string(), &s).contains("placeholder=\"Find a guide\""));
    }
}
