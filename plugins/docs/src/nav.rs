//! The documentation sections' markup: sidebar, table of contents,
//! pager, breadcrumbs, version banner and search box.
//!
//! The host hands a section everything it may use in the payload's
//! `context` (the entry, the whole published tree of its type, the page's
//! headings); these functions only turn that into HTML. Nothing here
//! queries anything, so every output is a pure function of the payload and
//! caches with the page.

use serde_json::Value;

use crate::blocks::esc;

/// One node of the published tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Entry id (a string: snowflakes do not survive JSON numbers).
    pub id: String,
    /// Parent id, `None` at the top.
    pub parent: Option<String>,
    /// Order among siblings.
    pub position: i64,
    /// Title.
    pub title: String,
    /// URL segment.
    pub slug: String,
    /// Public path.
    pub url: String,
    /// Field `hide_from_nav`.
    pub hide: bool,
    /// Field `version_root`.
    pub version_root: bool,
    /// Field `section_label`.
    pub label: String,
}

/// One heading of the current page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toc {
    /// 2..=6.
    pub level: u8,
    /// `#vy-h-3`.
    pub anchor: String,
    /// Plain text.
    pub text: String,
}

/// The page a section is rendering for.
#[derive(Debug, Clone, Default)]
pub struct Page {
    /// The current entry's id.
    pub id: String,
    /// Ids from the top of the tree down to (not including) the entry.
    pub ancestors: Vec<String>,
    /// The published tree of the entry's type.
    pub tree: Vec<Node>,
    /// The page's headings.
    pub toc: Vec<Toc>,
}

fn node_of(v: &Value) -> Option<Node> {
    let f = &v["fields"];
    Some(Node {
        id: v["id"].as_str()?.to_owned(),
        parent: v["parent_id"].as_str().map(str::to_owned),
        position: v["position"].as_i64().unwrap_or(0),
        title: v["title"].as_str().unwrap_or("").to_owned(),
        slug: v["slug"].as_str().unwrap_or("").to_owned(),
        url: v["url"].as_str()?.to_owned(),
        hide: f["hide_from_nav"].as_bool().unwrap_or(false),
        version_root: f["version_root"].as_bool().unwrap_or(false),
        label: f["section_label"].as_str().unwrap_or("").to_owned(),
    })
}

/// Reads a `render-section` payload's `context`.
#[must_use]
pub fn parse(payload: &Value) -> Option<Page> {
    let ctx = payload.get("context")?;
    let entry = ctx.get("entry")?;
    Some(Page {
        id: entry["id"].as_str()?.to_owned(),
        ancestors: entry["ancestors"]
            .as_array()?
            .iter()
            .filter_map(|a| a.as_str().map(str::to_owned))
            .collect(),
        tree: ctx["tree"]
            .as_array()
            .map(|a| a.iter().filter_map(node_of).collect())
            .unwrap_or_default(),
        toc: ctx["toc"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(Toc {
                            level: u8::try_from(t["level"].as_u64()?).ok()?,
                            anchor: t["anchor"].as_str()?.to_owned(),
                            text: t["text"].as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

impl Page {
    fn node(&self, id: &str) -> Option<&Node> {
        self.tree.iter().find(|n| n.id == id)
    }

    fn current(&self) -> Option<&Node> {
        self.node(&self.id)
    }

    /// Children of `parent` (`None` = top level), in order.
    fn children(&self, parent: Option<&str>) -> Vec<&Node> {
        let mut v: Vec<&Node> = self
            .tree
            .iter()
            .filter(|n| n.parent.as_deref() == parent)
            .collect();
        v.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));
        v
    }

    /// The version entry this page lives under, if it is not in the latest tree.
    fn version_root(&self) -> Option<&Node> {
        let top = self
            .ancestors
            .first()
            .map(String::as_str)
            .or(Some(self.id.as_str()))?;
        self.node(top).filter(|n| n.version_root)
    }

    /// The nodes the sidebar lists at its top level.
    fn nav_roots(&self) -> Vec<&Node> {
        match self.version_root() {
            Some(v) => self.children(Some(&v.id)),
            None => self
                .children(None)
                .into_iter()
                .filter(|n| !n.version_root)
                .collect(),
        }
    }

    /// Visible pages in reading order within the current nav tree.
    fn flat(&self) -> Vec<&Node> {
        fn walk<'a>(p: &'a Page, nodes: Vec<&'a Node>, out: &mut Vec<&'a Node>) {
            for n in nodes {
                if n.hide || n.version_root {
                    continue;
                }
                out.push(n);
                walk(p, p.children(Some(&n.id)), out);
            }
        }
        let mut out = Vec::new();
        walk(self, self.nav_roots(), &mut out);
        out
    }

    fn base(&self) -> String {
        self.current()
            .and_then(|n| n.url.strip_prefix('/'))
            .and_then(|u| u.split('/').next())
            .map_or_else(|| "docs".to_owned(), str::to_owned)
    }
}

/// The sidebar: a collapsible tree with the current page marked, a version
/// switcher when the site has more than one version.
#[must_use]
pub fn sidebar(page: &Page, latest_label: &str) -> String {
    let mut out = String::from(
        "<details class=\"docs-drawer\" open><summary class=\"docs-drawer__toggle\">Menu</summary>",
    );
    out.push_str(&version_switcher(page, latest_label));
    out.push_str("<nav class=\"docs-nav\" aria-label=\"Documentation\">");
    out.push_str(&list(page, page.nav_roots(), 0));
    out.push_str("</nav></details>");
    out
}

fn list(page: &Page, nodes: Vec<&Node>, depth: usize) -> String {
    let mut out = String::from("<ul class=\"docs-nav__list\">");
    let mut last_label = String::new();
    for n in nodes {
        if n.hide || n.version_root {
            continue;
        }
        if !n.label.is_empty() && n.label != last_label {
            out.push_str(&format!(
                "<li class=\"docs-nav__label\">{}</li>",
                esc(&n.label)
            ));
            last_label.clone_from(&n.label);
        }
        let kids = page.children(Some(&n.id));
        let is_current = n.id == page.id;
        let on_path = page.ancestors.contains(&n.id) || is_current;
        let link = format!(
            "<a class=\"docs-nav__link{}\" href=\"{}\"{}>{}</a>",
            if is_current { " is-current" } else { "" },
            esc(&n.url),
            if is_current {
                " aria-current=\"page\""
            } else {
                ""
            },
            esc(&n.title)
        );
        let visible_kids: Vec<&Node> = kids
            .iter()
            .copied()
            .filter(|k| !k.hide && !k.version_root)
            .collect();
        if visible_kids.is_empty() {
            out.push_str(&format!("<li class=\"docs-nav__item\">{link}</li>"));
        } else {
            out.push_str(&format!(
                "<li class=\"docs-nav__item docs-nav__group\"><details{}><summary>{link}</summary>{}</details></li>",
                if on_path || depth == 0 { " open" } else { "" },
                list(page, kids, depth + 1)
            ));
        }
    }
    out.push_str("</ul>");
    out
}

fn version_switcher(page: &Page, latest_label: &str) -> String {
    let roots: Vec<&Node> = page
        .children(None)
        .into_iter()
        .filter(|n| n.version_root)
        .collect();
    if roots.is_empty() {
        return String::new();
    }
    let current_root = page.version_root().map(|n| n.id.as_str());
    let mut out = String::from(
        "<div class=\"docs-versions\"><span class=\"docs-versions__label\">Version</span><ul>",
    );
    out.push_str(&format!(
        "<li><a class=\"docs-versions__link{}\" href=\"/{}\">{}</a></li>",
        if current_root.is_none() {
            " is-current"
        } else {
            ""
        },
        esc(&page.base()),
        esc(latest_label)
    ));
    for r in roots {
        out.push_str(&format!(
            "<li><a class=\"docs-versions__link{}\" href=\"{}\">{}</a></li>",
            if current_root == Some(r.id.as_str()) {
                " is-current"
            } else {
                ""
            },
            esc(&r.url),
            esc(&r.title)
        ));
    }
    out.push_str("</ul></div>");
    out
}

/// "On this page": the h2 and h3 headings. Empty with fewer than two.
#[must_use]
pub fn toc(page: &Page) -> String {
    let items: Vec<&Toc> = page
        .toc
        .iter()
        .filter(|t| t.level == 2 || t.level == 3)
        .collect();
    if items.len() < 2 {
        return String::new();
    }
    let mut out = String::from("<details class=\"docs-toc__drawer\" open><summary class=\"docs-toc__toggle\">On this page</summary><nav class=\"docs-toc\" aria-label=\"On this page\"><ol>");
    for t in items {
        out.push_str(&format!(
            "<li class=\"docs-toc__l{}\"><a href=\"{}\">{}</a></li>",
            t.level,
            esc(&t.anchor),
            esc(&t.text)
        ));
    }
    out.push_str("</ol></nav></details>");
    out
}

/// Previous and next page in reading order.
#[must_use]
pub fn pager(page: &Page) -> String {
    let flat = page.flat();
    let Some(i) = flat.iter().position(|n| n.id == page.id) else {
        return String::new();
    };
    let link = |n: &Node, rel: &str, word: &str| {
        format!("<a class=\"docs-pager__link docs-pager__{rel}\" rel=\"{rel}\" href=\"{}\"><span class=\"docs-pager__hint\">{word}</span><span class=\"docs-pager__title\">{}</span></a>", esc(&n.url), esc(&n.title))
    };
    let prev = i
        .checked_sub(1)
        .and_then(|p| flat.get(p))
        .map(|n| link(n, "prev", "Previous"));
    let next = flat.get(i + 1).map(|n| link(n, "next", "Next"));
    if prev.is_none() && next.is_none() {
        return String::new();
    }
    format!(
        "<nav class=\"docs-pager\" aria-label=\"Pager\">{}{}</nav>",
        prev.unwrap_or_default(),
        next.unwrap_or_default()
    )
}

/// Docs › Section › Page.
#[must_use]
pub fn breadcrumbs(page: &Page, home_label: &str) -> String {
    let Some(cur) = page.current() else {
        return String::new();
    };
    let mut out = format!(
        "<nav class=\"docs-crumbs\" aria-label=\"Breadcrumb\"><ol><li><a href=\"/{}\">{}</a></li>",
        esc(&page.base()),
        esc(home_label)
    );
    for id in &page.ancestors {
        if let Some(n) = page.node(id) {
            if n.version_root {
                continue;
            }
            out.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                esc(&n.url),
                esc(&n.title)
            ));
        }
    }
    out.push_str(&format!(
        "<li aria-current=\"page\">{}</li></ol></nav>",
        esc(&cur.title)
    ));
    out
}

/// The notice on a page of an old version, linking to the same page in the latest.
#[must_use]
pub fn banner(page: &Page, latest_label: &str) -> String {
    let Some(root) = page.version_root() else {
        return String::new();
    };
    // The same page in the latest tree: follow the slugs below the version.
    let mut target: Option<&Node> = None;
    let mut parent: Option<&str> = None;
    let chain: Vec<&Node> = page
        .ancestors
        .iter()
        .skip(1)
        .filter_map(|id| page.node(id))
        .chain(page.current())
        .collect();
    for step in chain {
        let found = page
            .children(parent)
            .into_iter()
            .find(|n| n.slug == step.slug && !n.version_root);
        match found {
            Some(n) => {
                target = Some(n);
                parent = Some(&n.id);
            }
            None => {
                target = None;
                break;
            }
        }
    }
    let href = target.map_or_else(|| format!("/{}", page.base()), |n| n.url.clone());
    format!(
        "<div class=\"docs-banner\" role=\"note\"><span>You are reading the documentation for <strong>{}</strong>.</span> <a href=\"{}\">Go to {}</a></div>",
        esc(&root.title),
        esc(&href),
        esc(latest_label)
    )
}

/// The search box: a GET form to `/<base>/search`, which the site turns
/// into its scoped search (`/search?type=doc&term=doc-version:v2`).
#[must_use]
pub fn search(page: &Page, latest_version: &str, placeholder: &str) -> String {
    let version = page.version_root().map_or_else(
        || crate::plan::slugify(latest_version),
        |n| crate::plan::slugify(&n.slug),
    );
    let hidden = if version.is_empty() {
        String::new()
    } else {
        format!(
            "<input type=\"hidden\" name=\"term\" value=\"doc-version:{}\">",
            esc(&version)
        )
    };
    format!(
        "<form class=\"docs-search\" role=\"search\" action=\"/{}/search\" method=\"get\"><input class=\"docs-search__input\" type=\"search\" name=\"q\" placeholder=\"{}\" aria-label=\"{}\" autocomplete=\"off\">{hidden}<button class=\"docs-search__button\" type=\"submit\">Search</button></form>",
        esc(&page.base()),
        esc(placeholder),
        esc(placeholder)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn n(id: &str, parent: Option<&str>, pos: i64, title: &str, url: &str, fields: Value) -> Value {
        json!({"id": id, "parent_id": parent, "position": pos, "title": title, "slug": url.rsplit('/').next().unwrap(), "url": url, "fields": fields})
    }

    fn payload(current: &str, ancestors: &[&str]) -> Value {
        json!({"kind": "docs/sidebar", "context": {
            "entry": {"id": current, "ancestors": ancestors},
            "toc": [{"level":2,"anchor":"#vy-h-1","text":"Intro"},{"level":3,"anchor":"#vy-h-2","text":"Deep"},{"level":2,"anchor":"#vy-h-3","text":"More <b>"}],
            "tree": [
                n("1", None, 10, "Overview", "/docs/overview", json!({})),
                n("2", None, 20, "Guides", "/docs/guides", json!({"section_label": "Learn"})),
                n("3", Some("2"), 10, "Install", "/docs/guides/install", json!({})),
                n("4", Some("2"), 20, "Secret", "/docs/guides/secret", json!({"hide_from_nav": true})),
                n("5", Some("2"), 30, "Deploy", "/docs/guides/deploy", json!({})),
                n("9", None, 900, "v1", "/docs/v1", json!({"version_root": true, "hide_from_nav": true})),
                n("10", Some("9"), 10, "Old install", "/docs/v1/install", json!({})),
                n("11", Some("9"), 20, "Old guide", "/docs/v1/guide", json!({})),
            ]
        }})
    }

    #[test]
    fn the_payload_parses() {
        let p = parse(&payload("3", &["2"])).unwrap();
        assert_eq!(p.id, "3");
        assert_eq!(p.tree.len(), 8);
        assert_eq!(p.toc.len(), 3);
        assert!(p.node("9").unwrap().version_root);
        assert!(parse(&json!({"kind": "x"})).is_none());
    }

    #[test]
    fn the_sidebar_marks_the_current_page_opens_its_path_and_hides_hidden_pages() {
        let p = parse(&payload("3", &["2"])).unwrap();
        let html = sidebar(&p, "Latest");
        assert!(html.contains("aria-current=\"page\""), "{html}");
        assert_eq!(html.matches("aria-current").count(), 1);
        assert!(
            html.contains("<li class=\"docs-nav__label\">Learn</li>"),
            "{html}"
        );
        assert!(!html.contains("Secret"), "hidden page leaked into nav");
        assert!(
            !html.contains("Old install"),
            "old version leaked into latest nav"
        );
        assert!(html.contains("<details open><summary><a class=\"docs-nav__link\" href=\"/docs/guides\">Guides</a>"), "{html}");
        assert!(html.contains("docs-versions"), "{html}");
        assert!(html.contains("href=\"/docs/v1\""));
    }

    #[test]
    fn inside_an_old_version_the_nav_is_that_versions_pages() {
        let p = parse(&payload("10", &["9"])).unwrap();
        let html = sidebar(&p, "Latest");
        assert!(html.contains("Old guide") && html.contains("Old install"));
        assert!(!html.contains("Overview"));
    }

    #[test]
    fn the_toc_lists_h2_and_h3_escaped_and_is_empty_for_short_pages() {
        let p = parse(&payload("3", &["2"])).unwrap();
        let html = toc(&p);
        assert!(html.contains("docs-toc__l3"));
        assert!(html.contains("More &lt;b&gt;"), "{html}");
        let mut short = p.clone();
        short.toc.truncate(1);
        assert_eq!(toc(&short), "");
    }

    #[test]
    fn the_pager_walks_visible_pages_in_reading_order() {
        let p = parse(&payload("3", &["2"])).unwrap();
        let html = pager(&p);
        assert!(
            html.contains("href=\"/docs/guides\""),
            "prev of install is its folder: {html}"
        );
        assert!(
            html.contains("href=\"/docs/guides/deploy\""),
            "next skips the hidden page: {html}"
        );
        let first = parse(&payload("1", &[])).unwrap();
        assert!(!pager(&first).contains("rel=\"prev\""));
    }

    #[test]
    fn breadcrumbs_skip_the_version_entry() {
        let p = parse(&payload("10", &["9"])).unwrap();
        let html = breadcrumbs(&p, "Docs");
        assert!(html.contains("<a href=\"/docs\">Docs</a>"));
        assert!(!html.contains(">v1<"));
        assert!(html.contains("aria-current=\"page\">Old install"));
    }

    #[test]
    fn the_banner_appears_only_on_old_versions_and_links_to_the_same_page() {
        assert_eq!(banner(&parse(&payload("3", &["2"])).unwrap(), "latest"), "");
        let mut v = payload("10", &["9"]);
        // Give the latest tree an "install" at the top so the slug chain matches.
        v["context"]["tree"].as_array_mut().unwrap().push(n(
            "12",
            None,
            30,
            "Install",
            "/docs/install",
            json!({}),
        ));
        let html = banner(&parse(&v).unwrap(), "latest");
        assert!(html.contains("<strong>v1</strong>"), "{html}");
        assert!(html.contains("href=\"/docs/install\""), "{html}");
        let html = banner(&parse(&payload("11", &["9"])).unwrap(), "latest");
        assert!(
            html.contains("href=\"/docs\""),
            "no equivalent page: fall back to the docs home: {html}"
        );
    }

    #[test]
    fn search_carries_the_version_and_escapes() {
        let p = parse(&payload("3", &["2"])).unwrap();
        let html = search(&p, "v2", "Search \"docs\"");
        assert!(html.contains("action=\"/docs/search\""));
        assert!(html.contains("name=\"term\" value=\"doc-version:v2\""));
        assert!(html.contains("&quot;docs&quot;"));
        let old = parse(&payload("10", &["9"])).unwrap();
        assert!(search(&old, "v2", "s").contains("value=\"doc-version:v1\""));
    }
}
