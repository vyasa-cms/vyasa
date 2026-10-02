//! A whole menu as a value: validated, renderable, storable in a
//! proposal.
//!
//! The studio agent stages menu changes against these, the accept path
//! applies them, and — because [`MenuDraft::render_html`] is the only
//! nested-list renderer — a staged menu previews byte-identical to the
//! live one it becomes.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use vyasa_db::repo::{MenuItemRow, MenuRow};

use super::service::{MAX_LABEL_LEN, MAX_MENU_DEPTH};

/// Most links one menu may hold, subtrees included. Generous for a nav,
/// tight enough that a runaway model cannot stage a phone book.
pub const MAX_MENU_ITEMS: usize = 50;

/// Allowed `href` shapes for menu items — the same list the service
/// enforces, here so pure validation refuses what the DB would.
pub(super) fn url_allowed(url: &str) -> bool {
    url.starts_with('/')
        || url.starts_with('#')
        || url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("mailto:")
}

/// One link, possibly with a submenu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuDraftItem {
    /// Link text.
    pub label: String,
    /// Target href.
    pub url: String,
    /// Submenu, depth-limited by [`validate`](MenuDraft::validate).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MenuDraftItem>,
}

/// A complete menu: identity plus every link, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuDraft {
    /// The reference themes address the menu by.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// Theme location slot (`header`, `footer`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Top-level links.
    #[serde(default)]
    pub items: Vec<MenuDraftItem>,
}

impl MenuDraft {
    /// Lifts DB rows into a value (rows arrive parents-before-children,
    /// siblings by sort order — `MenusRepo::items` guarantees both).
    #[must_use]
    pub fn from_rows(menu: &MenuRow, rows: &[MenuItemRow]) -> Self {
        fn tree(rows: &[MenuItemRow], parent: Option<i64>, depth: usize) -> Vec<MenuDraftItem> {
            if depth > MAX_MENU_DEPTH {
                return Vec::new();
            }
            rows.iter()
                .filter(|r| r.parent_id == parent)
                .map(|r| MenuDraftItem {
                    label: r.label.trim().to_owned(),
                    url: r.url.clone(),
                    children: tree(rows, Some(r.id), depth + 1),
                })
                .collect()
        }
        Self {
            slug: menu.slug.clone(),
            name: menu.name.clone(),
            location: menu.location.clone(),
            items: tree(rows, None, 1),
        }
    }

    /// Every rule the service enforces, checked purely; empty means the
    /// draft would apply cleanly.
    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if vyasa_common::slugify(&self.slug) != self.slug
            || self.slug.is_empty()
            || self.slug.chars().count() > 60
        {
            problems.push(format!(
                "slug {:?} may only use lowercase letters, digits and hyphens (max 60)",
                self.slug
            ));
        }
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            problems.push("menu name must be 1..=60 characters".to_owned());
        }
        if count_items(&self.items) > MAX_MENU_ITEMS {
            problems.push(format!("a menu holds at most {MAX_MENU_ITEMS} links"));
        }
        check_items(&self.items, 1, &mut problems);
        problems
    }

    /// Links in the whole menu, subtrees included.
    #[must_use]
    pub fn link_count(&self) -> usize {
        count_items(&self.items)
    }

    /// The one nested `<ul class="vy-menu">` renderer. Everything —
    /// labels, and urls that land inside `href="…"` — is escaped;
    /// operator-entered is not a licence to inject into every visitor's
    /// page.
    #[must_use]
    pub fn render_html(&self) -> String {
        render_level(&self.items, 1)
    }
}

fn count_items(items: &[MenuDraftItem]) -> usize {
    items
        .iter()
        .map(|i| 1 + count_items(&i.children))
        .sum::<usize>()
}

fn check_items(items: &[MenuDraftItem], depth: usize, problems: &mut Vec<String>) {
    for item in items {
        let label = item.label.trim();
        if label.is_empty() || label.chars().count() > MAX_LABEL_LEN {
            problems.push(format!(
                "label {:?} must be 1..={MAX_LABEL_LEN} characters",
                item.label
            ));
        }
        if !url_allowed(&item.url) {
            problems.push(format!(
                "menu url {:?} must start with /, #, http(s):// or mailto:",
                item.url
            ));
        }
        if !item.children.is_empty() && depth >= MAX_MENU_DEPTH {
            problems.push(format!(
                "{label:?} nests too deep — menus nest at most {MAX_MENU_DEPTH} levels"
            ));
        } else {
            check_items(&item.children, depth + 1, problems);
        }
    }
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn render_level(items: &[MenuDraftItem], depth: usize) -> String {
    if items.is_empty() || depth > MAX_MENU_DEPTH {
        return String::new();
    }
    let mut out = String::from("<ul class=\"vy-menu\">");
    for item in items {
        let _ = write!(
            out,
            "<li><a href=\"{}\">{}</a>{}</li>",
            esc(&item.url),
            esc(item.label.trim()),
            render_level(&item.children, depth + 1)
        );
    }
    out.push_str("</ul>");
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn link(label: &str, url: &str) -> MenuDraftItem {
        MenuDraftItem {
            label: label.into(),
            url: url.into(),
            children: Vec::new(),
        }
    }

    fn draft(items: Vec<MenuDraftItem>) -> MenuDraft {
        MenuDraft {
            slug: "main".into(),
            name: "Main".into(),
            location: None,
            items,
        }
    }

    #[test]
    fn a_clean_draft_validates_and_renders_nested() {
        let d = draft(vec![
            link("Home", "/"),
            MenuDraftItem {
                children: vec![link("API", "/api-docs")],
                ..link("Docs", "https://docs.example.com")
            },
        ]);
        assert!(d.validate().is_empty());
        let html = d.render_html();
        assert_eq!(
            html,
            "<ul class=\"vy-menu\"><li><a href=\"/\">Home</a></li>\
             <li><a href=\"https://docs.example.com\">Docs</a>\
             <ul class=\"vy-menu\"><li><a href=\"/api-docs\">API</a></li></ul></li></ul>"
        );
    }

    #[test]
    fn labels_and_urls_are_escaped_quotes_included() {
        let d = draft(vec![link("R&D <lab>", "/a\"b'c")]);
        let html = d.render_html();
        assert!(html.contains("R&amp;D &lt;lab&gt;"), "{html}");
        assert!(html.contains("href=\"/a&quot;b&#39;c\""), "{html}");
    }

    #[test]
    fn the_service_rules_hold_purely() {
        let bad = MenuDraft {
            slug: "Not A Slug".into(),
            name: String::new(),
            location: None,
            items: vec![
                link("", "/x"),
                link("Evil", "javascript:alert(1)"),
                MenuDraftItem {
                    children: vec![MenuDraftItem {
                        children: vec![MenuDraftItem {
                            children: vec![link("Too deep", "/d")],
                            ..link("Three", "/3")
                        }],
                        ..link("Two", "/2")
                    }],
                    ..link("One", "/1")
                },
            ],
        };
        let problems = bad.validate().join("\n");
        assert!(problems.contains("slug"), "{problems}");
        assert!(problems.contains("menu name"), "{problems}");
        assert!(problems.contains("1..=80 characters"), "{problems}");
        assert!(problems.contains("javascript:alert(1)"), "{problems}");
        assert!(problems.contains("nests too deep"), "{problems}");
    }

    #[test]
    fn from_rows_rebuilds_the_tree_and_round_trips_serde() {
        let menu = MenuRow {
            id: 1,
            name: "Main".into(),
            slug: "main".into(),
            location: Some("header".into()),
        };
        let rows = vec![
            MenuItemRow {
                id: 10,
                menu_id: 1,
                parent_id: None,
                label: "Home".into(),
                url: "/".into(),
                sort_order: 0,
            },
            MenuItemRow {
                id: 11,
                menu_id: 1,
                parent_id: None,
                label: "Docs".into(),
                url: "/docs".into(),
                sort_order: 1,
            },
            MenuItemRow {
                id: 12,
                menu_id: 1,
                parent_id: Some(11),
                label: "API".into(),
                url: "/api".into(),
                sort_order: 0,
            },
        ];
        let d = MenuDraft::from_rows(&menu, &rows);
        assert_eq!(d.items.len(), 2);
        assert_eq!(d.items[1].children[0].label, "API");
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(serde_json::from_value::<MenuDraft>(json).unwrap(), d);
    }
}
