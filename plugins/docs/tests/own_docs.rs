//! The repository's own `docs/` folder, through the importer's pure
//! pipeline: the first customer must keep converting.
//!
//! It reads the Markdown that sits two directories up, so it runs from a
//! checkout of the Vyasa repository (the plugin's `cargo test` does).

use std::collections::BTreeMap;
use std::path::Path;

use vyasa_docs::blocks::{self, Links};
use vyasa_docs::frontmatter;
use vyasa_docs::plan::{self, Kind, PageInfo, Root};

fn docs_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs")
}

#[test]
fn the_repositorys_own_docs_import_in_the_order_its_meta_file_gives() {
    let dir = docs_dir();
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("docs/ exists")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md"))
        .collect();
    files.sort();
    assert!(files.len() >= 19, "{files:?}");

    let meta = plan::parse_meta(
        &std::fs::read_to_string(dir.join("_meta.json")).expect("docs/_meta.json"),
    );
    assert!(!meta.order.is_empty(), "the meta file lists the pages");

    let mut pages = Vec::new();
    let mut warnings = Vec::new();
    for name in &files {
        let src = std::fs::read_to_string(dir.join(name)).expect("readable");
        let split = frontmatter::split(&src);
        warnings.extend(split.warnings.clone());
        let converted = blocks::convert(split.body, &Links::default());
        warnings.extend(converted.warnings);
        let title = split
            .front
            .title
            .clone()
            .or(converted.h1)
            .unwrap_or_default();
        assert!(!title.is_empty(), "{name} has no title");
        assert!(!converted.blocks.is_empty(), "{name} converted to nothing");
        pages.push(PageInfo {
            key: name.clone(),
            path: name.clone(),
            title,
            slug: split.front.slug.clone(),
            position: split.front.position,
            hide: split.front.hide,
            section: split.front.section.clone(),
        });
    }

    let mut metas = BTreeMap::new();
    metas.insert(String::new(), meta.clone());
    let root = Root {
        base: "docs".into(),
        prefix: vec![],
        parent_key: None,
        tree: "main".into(),
    };
    let (planned, plan_warnings) = plan::plan_tree(&pages, &metas, &root);
    assert!(
        plan_warnings.is_empty(),
        "no two pages want one slug: {plan_warnings:?}"
    );
    assert_eq!(planned.len(), files.len());
    assert!(planned
        .iter()
        .all(|p| p.kind == Kind::Page && p.parent_key.is_none()));

    let paths: Vec<&str> = planned.iter().map(|p| p.site_path.as_str()).collect();
    assert_eq!(
        paths[0], "/docs/overview",
        "index.md is the first page: {paths:?}"
    );
    let pos = |p: &str| {
        paths
            .iter()
            .position(|x| *x == p)
            .unwrap_or_else(|| panic!("{p} in {paths:?}"))
    };
    assert!(pos("/docs/architecture") < pos("/docs/deployment"));
    assert!(pos("/docs/deployment") < pos("/docs/themes"));
    assert!(pos("/docs/plugin-api") < pos("/docs/docs-sites"));
    assert_eq!(
        paths.last().copied(),
        Some("/docs/route-access"),
        "generated reference last"
    );

    // Every listed page exists, so the file does not drift from the folder.
    for name in &meta.order {
        assert!(
            files
                .iter()
                .any(|f| f.strip_suffix(".md") == Some(name.as_str())),
            "docs/_meta.json lists {name}, which is not a file in docs/"
        );
    }

    // Degradations are known and few: code blocks inside list items.
    let moved = warnings.iter().filter(|w| w.contains("list item")).count();
    assert!(
        moved <= 6,
        "{moved} blocks moved after their list: {warnings:?}"
    );
    let others: Vec<_> = warnings
        .iter()
        .filter(|w| !w.contains("list item"))
        .collect();
    assert!(others.is_empty(), "unexpected warnings: {others:?}");
}

/// Every `href` in serialised blocks (the quotes are JSON-escaped there).
fn hrefs(json: &str) -> Vec<String> {
    json.split("href=\\\"")
        .skip(1)
        .filter_map(|rest| rest.split("\\\"").next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn links_between_the_docs_become_site_paths() {
    let dir = docs_dir();
    let mut known = BTreeMap::new();
    for entry in std::fs::read_dir(&dir)
        .expect("docs/")
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".md") {
            known.insert(name.clone(), format!("/docs/{}", plan::slugify(stem)));
        }
    }
    let links = Links {
        folder: "docs".into(),
        dir: String::new(),
        known,
        blob_base: "https://github.com/vyasa-cms/vyasa/blob/abc/".into(),
        raw_base: "https://raw.githubusercontent.com/vyasa-cms/vyasa/abc/".into(),
    };
    let src = std::fs::read_to_string(dir.join("MARKETPLACE.md")).expect("MARKETPLACE.md");
    let out = blocks::convert(frontmatter::split(&src).body, &links);
    let all = serde_json::to_string(&out.blocks).expect("serialises");
    assert!(
        all.contains("href=\\\"/docs/publishing\\\""),
        "PUBLISHING.md became a site link: {all}"
    );
    assert!(
        hrefs(&all)
            .iter()
            .all(|h| !h.split('#').next().unwrap_or("").ends_with(".md")),
        "no link to a .md file survives: {:?}",
        hrefs(&all)
    );
    let arch = std::fs::read_to_string(dir.join("ARCHITECTURE.md")).expect("ARCHITECTURE.md");
    let out = blocks::convert(frontmatter::split(&arch).body, &links);
    let all = serde_json::to_string(&out.blocks).expect("serialises");
    assert!(
        hrefs(&all).iter().all(|h| !h.starts_with("../")),
        "links out of the folder point at GitHub: {:?}",
        hrefs(&all)
    );
}
