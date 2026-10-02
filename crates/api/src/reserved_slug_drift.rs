//! The reserved content-type slugs cannot drift from the router.
//!
//! A content type's slug is its URL: `/{slug}` lists it and
//! `/{slug}/{entry}` shows one. Every first path segment the site serves
//! itself must therefore be refused as a slug, or an administrator (or a
//! plugin) could create a type that the router never reaches — or one
//! that hides a page of the site's own. The list lives in core
//! (`RESERVED_TYPE_SLUGS`); this test derives the segments from the
//! router's own source, so adding a route without reserving its first
//! segment fails the build.

/// Every literal first path segment registered in `src` with `.route(`
/// or `.nest(`, a file name reduced to its stem (`feed.xml` → `feed`).
fn first_segments(src: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for needle in [".route(\n", ".route(", ".nest(\n", ".nest("] {
        let mut rest = src;
        while let Some(pos) = rest.find(needle) {
            rest = &rest[pos + needle.len()..];
            let Some(open) = rest.find('"') else { break };
            // Only the first argument: the quote must come before any
            // comma or closing parenthesis.
            let head = &rest[..open];
            if head.contains(',') || head.contains(')') {
                continue;
            }
            let after = &rest[open + 1..];
            let Some(close) = after.find('"') else { break };
            let path = &after[..close];
            let Some(first) = path.strip_prefix('/').and_then(|p| p.split('/').next()) else {
                continue;
            };
            if first.is_empty() || first.starts_with('{') {
                continue;
            }
            let stem = first.split('.').next().unwrap_or(first);
            out.insert(stem.to_owned());
        }
    }
    out
}

#[test]
fn every_first_segment_the_site_serves_is_a_reserved_type_slug() {
    // The root router in `app_router`: from its `axum::Router::new()` at
    // the start of a line (the GraphQL router before it is nested under
    // `/api`, so its paths are not first segments).
    let main = include_str!("main.rs");
    let app = &main[main.find("fn app_router(").expect("app_router")..];
    let root = &app[app.find("\n    axum::Router::new()").expect("root router")..];
    let root = &root[..root.find("\n}\n").unwrap_or(root.len())];
    let mut found = first_segments(root);
    found.extend(first_segments(include_str!("public/mod.rs")));
    // The scan itself must see the router, or it proves nothing.
    for expected in [
        "admin", "api", "post", "category", "feed", "sitemap", "llms",
    ] {
        assert!(
            found.contains(expected),
            "scan missed {expected}: {found:?}"
        );
    }
    let missing: Vec<&String> = found
        .iter()
        .filter(|s| !vyasa_core::content::RESERVED_TYPE_SLUGS.contains(&s.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "first path segments the router serves that a content type could take: {missing:?}; \
         add them to vyasa_core::content::RESERVED_TYPE_SLUGS"
    );
    // Plugins share the namespace and the rule.
    for slug in &found {
        assert!(
            !crate::plugin_surface::is_valid_post_type_slug(slug),
            "a plugin could declare {slug:?}"
        );
    }
}

#[test]
fn the_segment_scan_reads_first_arguments_only() {
    let src = r#"
        .route("/feed.xml", get(x))
        .route(
            "/thing/{id}",
            get(y),
        )
        .route("/{slug}", get(z))
        .nest("/api", other("/not-this"))
    "#;
    let found = first_segments(src);
    assert_eq!(
        found.into_iter().collect::<Vec<_>>(),
        ["api", "feed", "thing"]
    );
}
