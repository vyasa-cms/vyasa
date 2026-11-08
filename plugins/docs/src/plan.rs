//! From a list of files to a tree of entries.
//!
//! Folders become hierarchy, `_meta.json` and frontmatter decide order,
//! and every page gets the site path other pages' links should use. Pure:
//! nothing here knows about GitHub or the host.

use std::collections::{BTreeMap, BTreeSet};

/// What the planner needs to know about one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageInfo {
    /// Stable identity across syncs: the page's path in the repository
    /// below the docs folder.
    pub key: String,
    /// The path below this tree's root (`guides/install.md`).
    pub path: String,
    /// Final title: frontmatter, else the first H1, else the file name.
    pub title: String,
    /// Frontmatter `slug`.
    pub slug: Option<String>,
    /// Frontmatter position.
    pub position: Option<i64>,
    /// Frontmatter hide.
    pub hide: bool,
    /// Frontmatter section label.
    pub section: Option<String>,
}

/// A folder's `_meta.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    /// The folder's own title.
    pub title: Option<String>,
    /// Child names (file stem or folder name) in order.
    pub order: Vec<String>,
    /// Child titles by name.
    pub titles: BTreeMap<String, String>,
    /// Children to hide from the sidebar.
    pub hidden: BTreeSet<String>,
}

/// Parses `_meta.json`: either `{"title": "...", "pages": ["a", "b"]}` or
/// an ordered object `{"a": "Title A", "b": {"title": "B", "hidden": true}}`.
#[must_use]
pub fn parse_meta(json: &str) -> Meta {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(json) else {
        return Meta::default();
    };
    let mut meta = Meta::default();
    if let Some(pages) = map.get("pages").and_then(serde_json::Value::as_array) {
        meta.title = map
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        for p in pages.iter().filter_map(serde_json::Value::as_str) {
            if p.starts_with("---") || p.starts_with('[') || p == "..." {
                continue;
            }
            meta.order.push(p.to_owned());
        }
        return meta;
    }
    for (name, value) in &map {
        meta.order.push(name.clone());
        match value {
            serde_json::Value::String(t) => {
                meta.titles.insert(name.clone(), t.clone());
            }
            serde_json::Value::Object(o) => {
                if let Some(t) = o.get("title").and_then(serde_json::Value::as_str) {
                    meta.titles.insert(name.clone(), t.to_owned());
                }
                let hidden = o
                    .get("hidden")
                    .or_else(|| o.get("hide"))
                    .and_then(serde_json::Value::as_bool);
                if hidden == Some(true) {
                    meta.hidden.insert(name.clone());
                }
            }
            _ => {}
        }
    }
    meta
}

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A page from a Markdown file.
    Page,
    /// A folder with no index page.
    Group,
}

/// One entry to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// Identity: the page key, or `@dir/<tree>/<dir>` for a group.
    pub key: String,
    /// Key of the parent entry, `None` at the top.
    pub parent_key: Option<String>,
    /// URL segment.
    pub slug: String,
    /// Title.
    pub title: String,
    /// Order among siblings (10, 20, ...).
    pub position: i32,
    /// Hidden from the sidebar.
    pub hide: bool,
    /// Sidebar group label.
    pub section: Option<String>,
    /// Page or group.
    pub kind: Kind,
    /// Public path.
    pub site_path: String,
}

/// Lowercase ASCII slug of a name (a file stem, a folder, a version);
/// leading order numbers (`01-`) are dropped.
#[must_use]
pub fn slugify(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    let without_order = match trimmed.split_once('-') {
        Some((n, rest))
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() =>
        {
            rest
        }
        _ => trimmed,
    };
    without_order.to_owned()
}

fn order_number(stem: &str) -> Option<i64> {
    let (n, rest) = stem.split_once(['-', '_', '.'])?;
    (!rest.is_empty()).then(|| n.parse().ok()).flatten()
}

fn humanise(name: &str) -> String {
    let s = slugify(name).replace('-', " ");
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| {
        f.to_uppercase().collect::<String>() + c.as_str()
    })
}

fn stem_of(file: &str) -> &str {
    file.rsplit_once('.').map_or(file, |(s, _)| s)
}

fn is_index(file: &str) -> bool {
    matches!(
        stem_of(file).to_ascii_lowercase().as_str(),
        "index" | "readme"
    )
}

/// Where a tree sits in the site.
#[derive(Debug, Clone, Default)]
pub struct Root {
    /// URL base (`docs`).
    pub base: String,
    /// Slugs between the base and this tree (`["v1"]` for an old version).
    pub prefix: Vec<String>,
    /// Key of the entry the tree hangs under (a version root), if any.
    pub parent_key: Option<String>,
    /// Prefix that keeps group keys of different trees apart.
    pub tree: String,
}

struct Dir<'a> {
    pages: Vec<&'a PageInfo>,
    dirs: BTreeSet<String>,
}

/// Plans one tree.
#[must_use]
pub fn plan_tree(
    pages: &[PageInfo],
    metas: &BTreeMap<String, Meta>,
    root: &Root,
) -> (Vec<Planned>, Vec<String>) {
    let mut by_dir: BTreeMap<String, Dir> = BTreeMap::new();
    by_dir.entry(String::new()).or_insert_with(|| Dir {
        pages: Vec::new(),
        dirs: BTreeSet::new(),
    });
    for p in pages {
        let (dir, _) = p.path.rsplit_once('/').unwrap_or(("", &p.path));
        // Make every ancestor directory exist and know its child.
        let mut acc = String::new();
        for seg in dir.split('/').filter(|s| !s.is_empty()) {
            let parent = acc.clone();
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            by_dir.entry(acc.clone()).or_insert_with(|| Dir {
                pages: Vec::new(),
                dirs: BTreeSet::new(),
            });
            if let Some(d) = by_dir.get_mut(&parent) {
                d.dirs.insert(seg.to_owned());
            }
        }
        if let Some(d) = by_dir.get_mut(dir) {
            d.pages.push(p);
        }
    }
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    walk(
        &by_dir,
        metas,
        root,
        "",
        root.parent_key.clone(),
        &root.prefix,
        &mut out,
        &mut warnings,
    );
    // A hidden folder hides what it holds: out of the sidebar, the pager
    // and search alike. Parents come before their children in `out`.
    let mut hidden: BTreeSet<String> = BTreeSet::new();
    for p in &mut out {
        if p.parent_key.as_ref().is_some_and(|k| hidden.contains(k)) {
            p.hide = true;
        }
        if p.hide {
            hidden.insert(p.key.clone());
        }
    }
    (out, warnings)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    by_dir: &BTreeMap<String, Dir>,
    metas: &BTreeMap<String, Meta>,
    root: &Root,
    dir: &str,
    parent_key: Option<String>,
    path_slugs: &[String],
    out: &mut Vec<Planned>,
    warnings: &mut Vec<String>,
) {
    let Some(d) = by_dir.get(dir) else { return };
    let meta = metas.get(dir).cloned().unwrap_or_default();
    // Children: (sort rank, name, what).
    struct Child<'a> {
        name: String,
        rank: (u8, i64, String),
        page: Option<&'a PageInfo>,
        dir: Option<String>,
    }
    let mut kids: Vec<Child> = Vec::new();
    for p in &d.pages {
        let file = p.path.rsplit_once('/').map_or(p.path.as_str(), |(_, f)| f);
        if is_index(file) && !dir.is_empty() {
            continue; // the folder's own page
        }
        let stem = stem_of(file).to_owned();
        let listed = meta.order.iter().position(|n| *n == stem);
        let rank = match (listed, p.position.or_else(|| order_number(&stem))) {
            (Some(i), _) => (0, i as i64, stem.clone()),
            (None, _) if is_index(file) => (0, -1, stem.clone()),
            (None, Some(n)) => (1, n, stem.clone()),
            (None, None) => (2, 0, stem.clone()),
        };
        kids.push(Child {
            name: stem,
            rank,
            page: Some(p),
            dir: None,
        });
    }
    for sub in &d.dirs {
        let listed = meta.order.iter().position(|n| n == sub);
        let rank = match (listed, order_number(sub)) {
            (Some(i), _) => (0, i as i64, sub.clone()),
            (None, Some(n)) => (1, n, sub.clone()),
            (None, None) => (3, 0, sub.clone()),
        };
        kids.push(Child {
            name: sub.clone(),
            rank,
            page: None,
            dir: Some(sub.clone()),
        });
    }
    kids.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| a.name.cmp(&b.name)));

    let mut used: BTreeSet<String> = BTreeSet::new();
    for (i, kid) in kids.iter().enumerate() {
        let position = i32::try_from((i + 1) * 10).unwrap_or(i32::MAX);
        let hidden_by_meta = meta.hidden.contains(&kid.name);
        let titled = meta.titles.get(&kid.name).cloned();
        match (&kid.page, &kid.dir) {
            (Some(p), _) => {
                let slug_base = p
                    .slug
                    .as_deref()
                    .map(slugify)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        let s = slugify(&kid.name);
                        if s.is_empty() || (dir.is_empty() && is_index(&format!("{}.md", kid.name)))
                        {
                            "overview".to_owned()
                        } else {
                            s
                        }
                    });
                let slug = unique(&slug_base, &mut used, warnings);
                let mut slugs = path_slugs.to_vec();
                slugs.push(slug.clone());
                out.push(Planned {
                    key: p.key.clone(),
                    parent_key: parent_key.clone(),
                    slug,
                    title: titled.unwrap_or_else(|| p.title.clone()),
                    position,
                    hide: p.hide || hidden_by_meta,
                    section: p.section.clone(),
                    kind: Kind::Page,
                    site_path: site_path(&root.base, &slugs),
                });
            }
            (None, Some(sub)) => {
                let sub_path = if dir.is_empty() {
                    sub.clone()
                } else {
                    format!("{dir}/{sub}")
                };
                let index = by_dir.get(&sub_path).and_then(|sd| {
                    sd.pages.iter().find(|p| {
                        is_index(p.path.rsplit_once('/').map_or(p.path.as_str(), |(_, f)| f))
                    })
                });
                let sub_meta = metas.get(&sub_path);
                let (key, title, slug_base, hide, section, kind) = match index {
                    Some(p) => (
                        p.key.clone(),
                        titled
                            .or_else(|| sub_meta.and_then(|m| m.title.clone()))
                            .unwrap_or_else(|| p.title.clone()),
                        p.slug
                            .as_deref()
                            .map(slugify)
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| slugify(sub)),
                        p.hide || hidden_by_meta,
                        p.section.clone(),
                        Kind::Page,
                    ),
                    None => (
                        format!("@dir/{}/{}", root.tree, sub_path),
                        titled
                            .or_else(|| sub_meta.and_then(|m| m.title.clone()))
                            .unwrap_or_else(|| humanise(sub)),
                        slugify(sub),
                        hidden_by_meta,
                        None,
                        Kind::Group,
                    ),
                };
                let slug = unique(
                    &if slug_base.is_empty() {
                        "section".to_owned()
                    } else {
                        slug_base
                    },
                    &mut used,
                    warnings,
                );
                let mut slugs = path_slugs.to_vec();
                slugs.push(slug.clone());
                out.push(Planned {
                    key: key.clone(),
                    parent_key: parent_key.clone(),
                    slug,
                    title,
                    position,
                    hide,
                    section,
                    kind,
                    site_path: site_path(&root.base, &slugs),
                });
                walk(
                    by_dir,
                    metas,
                    root,
                    &sub_path,
                    Some(key),
                    &slugs,
                    out,
                    warnings,
                );
            }
            _ => {}
        }
    }
}

fn unique(base: &str, used: &mut BTreeSet<String>, warnings: &mut Vec<String>) -> String {
    let mut candidate = base.to_owned();
    let mut n = 2;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    if candidate != base {
        warnings.push(format!(
            "two siblings both want the slug {base}; one became {candidate}"
        ));
    }
    candidate
}

fn site_path(base: &str, slugs: &[String]) -> String {
    format!("/{base}/{}", slugs.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(path: &str, title: &str) -> PageInfo {
        PageInfo {
            key: path.to_owned(),
            path: path.to_owned(),
            title: title.to_owned(),
            slug: None,
            position: None,
            hide: false,
            section: None,
        }
    }

    fn root() -> Root {
        Root {
            base: "docs".into(),
            prefix: vec![],
            parent_key: None,
            tree: "latest".into(),
        }
    }

    fn summary(p: &[Planned]) -> Vec<(String, Option<String>, i32, String)> {
        p.iter()
            .map(|e| {
                (
                    e.site_path.clone(),
                    e.parent_key.clone(),
                    e.position,
                    e.title.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn slugs_drop_extensions_and_order_prefixes() {
        assert_eq!(slugify("FEATURE-MATRIX"), "feature-matrix");
        assert_eq!(slugify("01-getting-started"), "getting-started");
        assert_eq!(slugify("plugin-api"), "plugin-api");
        assert_eq!(slugify("v1.2"), "v1-2");
        assert_eq!(slugify("2024"), "2024");
        assert_eq!(slugify("Hello World!"), "hello-world");
    }

    #[test]
    fn a_flat_folder_is_ordered_by_meta_then_position_then_name() {
        let pages = vec![
            page("zeta.md", "Zeta"),
            page("alpha.md", "Alpha"),
            page("beta.md", "Beta"),
            page("gamma.md", "Gamma"),
        ];
        let mut metas = BTreeMap::new();
        metas.insert(String::new(), parse_meta(r#"{"pages":["beta","alpha"]}"#));
        let (planned, w) = plan_tree(&pages, &metas, &root());
        assert!(w.is_empty());
        let order: Vec<_> = planned.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(order, ["beta", "alpha", "gamma", "zeta"]);
        assert_eq!(planned[0].position, 10);
        assert_eq!(planned[3].position, 40);
    }

    #[test]
    fn everything_inside_a_hidden_folder_is_hidden() {
        let pages = vec![
            page("index.md", "Welcome"),
            page("guide.md", "Guide"),
            page("specs/a.md", "A"),
            page("specs/deep/b.md", "B"),
        ];
        let mut metas = BTreeMap::new();
        metas.insert(
            String::new(),
            parse_meta(r#"{"specs": {"title": "Specs", "hidden": true}}"#),
        );
        let (planned, _) = plan_tree(&pages, &metas, &root());
        for p in &planned {
            let inside = p.site_path.starts_with("/docs/specs");
            assert_eq!(p.hide, inside, "{} hide={}", p.site_path, p.hide);
        }
        assert!(planned.iter().any(|p| p.site_path == "/docs/specs/deep/b"));
    }

    #[test]
    fn folders_nest_and_an_index_page_is_the_folders_page() {
        let pages = vec![
            page("index.md", "Welcome"),
            page("guides/index.md", "Guides"),
            page("guides/install.md", "Install"),
            page("guides/deep/more.md", "More"),
        ];
        let (planned, _) = plan_tree(&pages, &BTreeMap::new(), &root());
        let s = summary(&planned);
        assert_eq!(s[0], ("/docs/overview".into(), None, 10, "Welcome".into()));
        assert_eq!(s[1], ("/docs/guides".into(), None, 20, "Guides".into()));
        assert_eq!(
            s[2],
            (
                "/docs/guides/install".into(),
                Some("guides/index.md".into()),
                10,
                "Install".into()
            )
        );
        assert_eq!(
            s[3],
            (
                "/docs/guides/deep".into(),
                Some("guides/index.md".into()),
                20,
                "Deep".into()
            )
        );
        assert_eq!(s[3].1.as_deref(), Some("guides/index.md"));
        assert_eq!(planned[3].kind, Kind::Group);
        assert_eq!(
            s[4],
            (
                "/docs/guides/deep/more".into(),
                Some("@dir/latest/guides/deep".into()),
                10,
                "More".into()
            )
        );
    }

    #[test]
    fn frontmatter_slug_position_and_hide_are_honoured() {
        let mut a = page("a.md", "A");
        a.slug = Some("first-page".into());
        a.position = Some(2);
        let mut b = page("b.md", "B");
        b.position = Some(1);
        b.hide = true;
        let (planned, _) = plan_tree(&[a, b], &BTreeMap::new(), &root());
        assert_eq!(planned[0].slug, "b");
        assert!(planned[0].hide);
        assert_eq!(planned[1].slug, "first-page");
    }

    #[test]
    fn colliding_slugs_are_suffixed_with_a_warning() {
        let (planned, w) = plan_tree(
            &[page("a/x.md", "X1"), page("a-x.md", "X2")],
            &BTreeMap::new(),
            &root(),
        );
        assert_eq!(planned.len(), 3); // the group "a" plus both pages
        let (planned2, w2) = plan_tree(
            &[page("x.md", "One"), page("x.mdx", "Two")],
            &BTreeMap::new(),
            &root(),
        );
        assert_eq!(planned2[1].slug, "x-2");
        assert_eq!(w2.len(), 1);
        let _ = (planned, w);
    }

    #[test]
    fn meta_titles_and_hidden_apply_in_the_ordered_object_shape() {
        let mut metas = BTreeMap::new();
        metas.insert(
            String::new(),
            parse_meta(r#"{"intro": "Start here", "secret": {"title": "Secret", "hidden": true}}"#),
        );
        let (planned, _) = plan_tree(
            &[page("secret.md", "s"), page("intro.md", "i")],
            &metas,
            &root(),
        );
        assert_eq!(planned[0].title, "Start here");
        assert_eq!(planned[1].title, "Secret");
        assert!(planned[1].hide);
    }

    #[test]
    fn an_old_version_hangs_under_its_version_entry() {
        let r = Root {
            base: "docs".into(),
            prefix: vec!["v1".into()],
            parent_key: Some("@version/v1".into()),
            tree: "v1".into(),
        };
        let (planned, _) = plan_tree(
            &[page("v1/install.md", "Install")]
                .iter()
                .map(|p| PageInfo {
                    path: "install.md".into(),
                    ..p.clone()
                })
                .collect::<Vec<_>>(),
            &BTreeMap::new(),
            &r,
        );
        assert_eq!(planned[0].site_path, "/docs/v1/install");
        assert_eq!(planned[0].parent_key.as_deref(), Some("@version/v1"));
    }

    #[test]
    fn numbered_prefixes_order_pages() {
        let (planned, _) = plan_tree(
            &[
                page("02-b.md", "B"),
                page("01-a.md", "A"),
                page("10-c.md", "C"),
            ],
            &BTreeMap::new(),
            &root(),
        );
        let order: Vec<_> = planned.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(order, ["a", "b", "c"]);
    }
}
