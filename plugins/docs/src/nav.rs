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
    /// Field `section_root`.
    pub section_root: bool,
    /// Field `section_label`.
    pub label: String,
    /// Field `cards`: the page is a generated list of its children.
    pub cards: bool,
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
        section_root: f["section_root"].as_bool().unwrap_or(false),
        label: f["section_label"].as_str().unwrap_or("").to_owned(),
        cards: f["cards"].as_bool().unwrap_or(false),
    })
}

/// Reads a `render-section` payload's `context`.
#[must_use]
pub fn parse(payload: &Value) -> Option<Page> {
    let ctx = payload.get("context")?;
    let entry = ctx.get("entry")?;
    let mut tree: Vec<Node> = ctx["tree"]
        .as_array()
        .map(|a| a.iter().filter_map(node_of).collect())
        .unwrap_or_default();
    // The tree is what the host could list; when it lacks the page itself
    // (a tree read that failed or was cut short), the page still knows its
    // own title, so its heading and trail do not vanish with it.
    let id = entry["id"].as_str()?;
    if !tree.iter().any(|n| n.id == id) {
        tree.extend(node_of(entry));
    }
    Some(Page {
        id: id.to_owned(),
        ancestors: entry["ancestors"]
            .as_array()?
            .iter()
            .filter_map(|a| a.as_str().map(str::to_owned))
            .collect(),
        tree,
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

    /// The doc section this page lives under, if any.
    fn section_root(&self) -> Option<&Node> {
        // The deepest section root on the way down: `engine/0.2` wins over its parent `engine`.
        self.ancestors
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(self.id.as_str()))
            .filter_map(|id| self.node(id))
            .rfind(|n| n.section_root)
    }

    /// Whether `n` hangs anywhere under `ancestor`.
    fn is_below(&self, n: &Node, ancestor: &str) -> bool {
        let mut cur = n.parent.as_deref();
        let mut hops = 0;
        while let Some(id) = cur {
            if id == ancestor {
                return true;
            }
            hops += 1;
            if hops > 64 {
                break;
            }
            cur = self.node(id).and_then(|p| p.parent.as_deref());
        }
        false
    }

    /// The nodes the sidebar lists at its top level.
    fn nav_roots(&self) -> Vec<&Node> {
        match self.version_root().or_else(|| self.section_root()) {
            Some(v) => self.children(Some(&v.id)),
            None => self
                .children(None)
                .into_iter()
                .filter(|n| !n.version_root && !n.section_root)
                .collect(),
        }
    }

    /// Visible pages in reading order within the current nav tree.
    fn flat(&self) -> Vec<&Node> {
        fn walk<'a>(p: &'a Page, nodes: Vec<&'a Node>, out: &mut Vec<&'a Node>) {
            for n in nodes {
                if n.hide || n.version_root || n.section_root {
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
pub fn sidebar(page: &Page, latest_label: &str, unreleased: &[String]) -> String {
    let mut out = String::from(
        "<details class=\"docs-drawer\" open><summary class=\"docs-drawer__toggle\">Pages</summary>",
    );
    out.push_str(&version_switcher(page, latest_label, unreleased));
    out.push_str("<nav class=\"docs-nav\" aria-label=\"Documentation\">");
    out.push_str(&list(page, page.nav_roots(), 0));
    out.push_str("</nav></details>");
    out
}

fn list(page: &Page, nodes: Vec<&Node>, depth: usize) -> String {
    let mut out = String::from("<ul class=\"docs-nav__list\">");
    let mut last_label = String::new();
    for n in nodes {
        if n.hide || n.version_root || n.section_root {
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
            .filter(|k| !k.hide && !k.version_root && !k.section_root)
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

fn version_switcher(page: &Page, latest_label: &str, unreleased: &[String]) -> String {
    if page.section_root().is_some() {
        return String::new();
    }
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
        let mark = if unreleased.contains(&r.title) {
            " <span class=\"docs-versions__mark\">unreleased</span>"
        } else {
            ""
        };
        out.push_str(&format!(
            "<li><a class=\"docs-versions__link{}\" href=\"{}\">{}{mark}</a></li>",
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
            toc_text(&t.text)
        ));
    }
    out.push_str("</ol></nav></details>");
    out
}

/// A heading's text for the contents list: escaped, except that a
/// literal `<code>…</code>` pair (the heading's inline code, which arrives
/// as text) is drawn as code rather than shown as tags.
fn toc_text(text: &str) -> String {
    esc(text)
        .replace("&lt;code&gt;", "<code>")
        .replace("&lt;/code&gt;", "</code>")
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
    // A page that is only a list of its children (shown as cards) has them
    // one click away: Next starts at the page after its subtree.
    let after = if flat[i].cards {
        flat.iter()
            .skip(i + 1)
            .find(|n| !page.is_below(n, &flat[i].id))
    } else {
        flat.get(i + 1)
    };
    let next = after.map(|n| link(n, "next", "Next"));
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
    // A section (`Engine 0.2`) is the trail's first entry: its own title and
    // address replace the generic "Docs" and every group above it, so the
    // trail reads "Engine 0.2 › Concepts" and repeats nothing.
    let section = page.section_root();
    let (home_url, home_text) = match section {
        Some(sec) => (sec.url.clone(), sec.title.clone()),
        None => (format!("/{}", page.base()), home_label.to_owned()),
    };
    let mut out = format!(
        "<nav class=\"docs-crumbs\" aria-label=\"Breadcrumb\"><ol><li><a href=\"{}\">{}</a></li>",
        esc(&home_url),
        esc(&home_text)
    );
    let start = section
        .and_then(|sec| page.ancestors.iter().position(|id| *id == sec.id))
        .map_or(0, |i| i + 1);
    for id in page.ancestors.iter().skip(start) {
        if let Some(n) = page.node(id) {
            if n.version_root || n.section_root {
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
    // The page's own `# Heading` became its title and left the body, so
    // the heading is drawn here, under the trail that leads to it.
    out.push_str(&format!(
        "<h1 class=\"docs-title\">{}</h1>",
        esc(&cur.title)
    ));
    out
}

/// The notice on a page of an old version, linking to the same page in the latest.
#[must_use]
pub fn banner(page: &Page, latest_label: &str, unreleased: &[String]) -> String {
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
    let kind = if unreleased.contains(&root.title) {
        "the unreleased documentation for"
    } else {
        "the documentation for"
    };
    format!(
        "<div class=\"docs-banner\" role=\"note\"><span>You are reading {kind} <strong>{}</strong>.</span> <a href=\"{}\">Go to {}</a></div>",
        esc(&root.title),
        esc(&href),
        esc(latest_label)
    )
}

/// The search box: a GET form to `/<base>/search`, which the site turns
/// into its scoped search (`/search?type=doc&term=doc-version:v2`).
#[must_use]
pub fn search(page: &Page, latest_version: &str, placeholder: &str) -> String {
    if let Some(sec) = page.section_root() {
        return search_form(
            page,
            &format!(
                "doc-section:{}",
                crate::plan::slugify(sec.url.trim_start_matches("/docs/")).replace('/', "-")
            ),
            placeholder,
        );
    }
    let version = page.version_root().map_or_else(
        || crate::plan::slugify(latest_version),
        |n| crate::plan::slugify(&n.slug),
    );
    let term = if version.is_empty() {
        String::new()
    } else {
        format!("doc-version:{version}")
    };
    search_form(page, &term, placeholder)
}

fn search_form(page: &Page, term: &str, placeholder: &str) -> String {
    let hidden = if term.is_empty() {
        String::new()
    } else {
        format!(
            "<input type=\"hidden\" name=\"term\" value=\"{}\">",
            esc(term)
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
    fn a_section_has_its_own_sidebar_breadcrumb_and_search_scope() {
        let v = json!({"context": {
            "entry": {"id": "22", "ancestors": ["20"]},
            "tree": [
                n("1", None, 10, "Overview", "/docs/overview", json!({})),
                n("9", None, 900, "v1", "/docs/v1", json!({"version_root": true, "hide_from_nav": true})),
                n("20", None, 950, "SQL reference", "/docs/sql", json!({"section_root": true, "hide_from_nav": true})),
                n("22", Some("20"), 10, "SELECT", "/docs/sql/select", json!({})),
                n("23", Some("20"), 20, "INSERT", "/docs/sql/insert", json!({})),
            ]
        }});
        let p = parse(&v).unwrap();
        let side = sidebar(&p, "latest", &[]);
        assert!(side.contains("SELECT") && side.contains("INSERT"));
        assert!(!side.contains("Overview") && !side.contains("docs-versions"));
        let crumbs = breadcrumbs(&p, "Docs");
        assert!(crumbs.contains("<a href=\"/docs/sql\">SQL reference</a>"));
        assert!(search(&p, "latest", "s").contains("value=\"doc-section:sql\""));
        assert!(pager(&p).contains("INSERT"));
        // The main sidebar never lists a section.
        let main = parse(&json!({"context": {"entry": {"id": "1", "ancestors": []}, "tree": v["context"]["tree"]}})).unwrap();
        let side = sidebar(&main, "latest", &[]);
        assert!(
            side.contains("Overview")
                && !side.contains("SQL reference")
                && !side.contains("SELECT")
        );
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
        let html = sidebar(&p, "Latest", &[]);
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
        let html = sidebar(&p, "Latest", &[]);
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
    fn a_cards_page_pages_on_to_the_page_after_its_children() {
        // Guides is only a list of Install and Deploy: Next must not be Install.
        let mut v = payload("2", &[]);
        v["context"]["tree"][1]["fields"]["cards"] = json!(true);
        let html = pager(&parse(&v).unwrap());
        assert!(!html.contains("/docs/guides/install"), "{html}");
        assert!(
            !html.contains("rel=\"next\""),
            "Guides is the last top page: {html}"
        );
        // A page that has its own text keeps its children as Next.
        let plain = pager(&parse(&payload("2", &[])).unwrap());
        assert!(plain.contains("href=\"/docs/guides/install\""), "{plain}");
        // The first child still goes back to the cards page.
        let mut v = payload("3", &["2"]);
        v["context"]["tree"][1]["fields"]["cards"] = json!(true);
        assert!(pager(&parse(&v).unwrap()).contains("href=\"/docs/guides\""));
    }

    #[test]
    fn a_page_missing_from_the_tree_still_gets_its_heading() {
        let mut v = payload("3", &["2"]);
        v["context"]["entry"]["title"] = json!("Lonely");
        v["context"]["entry"]["url"] = json!("/docs/lonely");
        v["context"]["tree"] = json!([]);
        let html = breadcrumbs(&parse(&v).unwrap(), "Docs");
        assert!(
            html.ends_with("<h1 class=\"docs-title\">Lonely</h1>"),
            "{html}"
        );
    }

    #[test]
    fn breadcrumbs_skip_the_version_entry() {
        let p = parse(&payload("10", &["9"])).unwrap();
        let html = breadcrumbs(&p, "Docs");
        assert!(html.contains("<a href=\"/docs\">Docs</a>"));
        assert!(!html.contains(">v1<"));
        assert!(html.contains("aria-current=\"page\">Old install"));
        assert!(html.ends_with("<h1 class=\"docs-title\">Old install</h1>"));
    }

    #[test]
    fn a_section_page_starts_its_trail_at_the_section_and_repeats_nothing() {
        let mut v = payload("10", &["9"]);
        for node in v["context"]["tree"].as_array_mut().unwrap() {
            if node["id"] == "9" {
                node["fields"]["section_root"] = json!(true);
                node["title"] = json!("Engine 0.2");
            }
        }
        let html = breadcrumbs(&parse(&v).unwrap(), "Docs");
        assert!(!html.contains(">Docs<"), "{html}");
        assert_eq!(html.matches("Engine 0.2").count(), 1, "{html}");
    }

    #[test]
    fn a_literal_code_pair_in_a_heading_is_drawn_as_code_in_the_contents() {
        assert_eq!(
            toc_text("<code>agg</code> and <b>x</b>"),
            "<code>agg</code> and &lt;b&gt;x&lt;/b&gt;"
        );
    }

    #[test]
    fn the_banner_appears_only_on_old_versions_and_links_to_the_same_page() {
        assert_eq!(
            banner(&parse(&payload("3", &["2"])).unwrap(), "latest", &[]),
            ""
        );
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
        let html = banner(&parse(&v).unwrap(), "latest", &[]);
        assert!(html.contains("<strong>v1</strong>"), "{html}");
        assert!(html.contains("href=\"/docs/install\""), "{html}");
        let html = banner(&parse(&payload("11", &["9"])).unwrap(), "latest", &[]);
        assert!(
            html.contains("href=\"/docs\""),
            "no equivalent page: fall back to the docs home: {html}"
        );
    }

    #[test]
    fn an_unreleased_version_is_marked_in_the_switcher_and_the_banner() {
        let p = parse(&payload("10", &["9"])).unwrap();
        let un = vec!["v1".to_owned()];
        let html = sidebar(&p, "Latest", &un);
        assert!(html.contains("docs-versions__mark\">unreleased<"), "{html}");
        assert!(!sidebar(&p, "Latest", &[]).contains("unreleased"));
        assert!(banner(&p, "latest", &un).contains("the unreleased documentation for <strong>v1"));
        assert!(!banner(&p, "latest", &[]).contains("unreleased"));
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
