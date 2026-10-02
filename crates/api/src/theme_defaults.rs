//! Embedded starter themes + first-boot auto-install.
//!
//! The five `.vytheme` packages are assembled at runtime from files
//! embedded with `include_str!` (zip writing is cheap and keeps the source
//! tree editable without a build step). On `serve` startup, when no theme
//! has ever been installed, the blog default is installed **and activated**
//! so a fresh binary serves a real site immediately.

use std::io::Write as _;

use sha2::Digest as _;

use vyasa_themes::package::parse_vytheme;

/// One embedded package ready for installation.
pub struct EmbeddedTheme {
    /// Package name from the manifest.
    pub name: String,
    /// Zip bytes of the `.vytheme`.
    pub bytes: Vec<u8>,
}

fn zip_entries(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, data) in entries {
            if w.start_file((*name).to_owned(), zip::write::SimpleFileOptions::default())
                .is_err()
            {
                unreachable!("zip writer rejected {name}");
            }
            if w.write_all(data).is_err() {
                unreachable!("zip write failed for {name}");
            }
        }
        if w.finish().is_err() {
            unreachable!("zip finish failed");
        }
    }
    buf.into_inner()
}

// Package directory, manifest, tokens, layout, template and bundled files.
type ThemeFiles<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Option<(&'a str, &'a str)>,
    &'a [(&'a str, &'a [u8])],
);

fn embedded_sets() -> [ThemeFiles<'static>; 5] {
    let sets: [ThemeFiles; 5] = [
        (
            "blog",
            include_str!("../../../themes-starter/blog/manifest.toml"),
            include_str!("../../../themes-starter/blog/tokens.json"),
            include_str!("../../../themes-starter/blog/layout.json"),
            Some((
                "templates/single.tera",
                include_str!("../../../themes-starter/blog/templates/single.tera"),
            )),
            &[
                (
                    "assets/theme.css",
                    include_bytes!("../../../themes-starter/blog/assets/theme.css"),
                ),
                (
                    "assets/images/editorial.svg",
                    include_bytes!("../../../themes-starter/blog/assets/images/editorial.svg"),
                ),
            ],
        ),
        (
            "portfolio",
            include_str!("../../../themes-starter/portfolio/manifest.toml"),
            include_str!("../../../themes-starter/portfolio/tokens.json"),
            include_str!("../../../themes-starter/portfolio/layout.json"),
            Some((
                "templates/single.tera",
                include_str!("../../../themes-starter/portfolio/templates/single.tera"),
            )),
            &[
                (
                    "assets/theme.css",
                    include_bytes!("../../../themes-starter/portfolio/assets/theme.css"),
                ),
                (
                    "assets/images/hero.svg",
                    include_bytes!("../../../themes-starter/portfolio/assets/images/hero.svg"),
                ),
            ],
        ),
        (
            "docs",
            include_str!("../../../themes-starter/docs/manifest.toml"),
            include_str!("../../../themes-starter/docs/tokens.json"),
            include_str!("../../../themes-starter/docs/layout.json"),
            Some((
                "templates/page.tera",
                include_str!("../../../themes-starter/docs/templates/page.tera"),
            )),
            &[(
                "assets/theme.css",
                include_bytes!("../../../themes-starter/docs/assets/theme.css"),
            )],
        ),
        (
            "storefront-lite",
            include_str!("../../../themes-starter/storefront-lite/manifest.toml"),
            include_str!("../../../themes-starter/storefront-lite/tokens.json"),
            include_str!("../../../themes-starter/storefront-lite/layout.json"),
            Some((
                "templates/index.tera",
                include_str!("../../../themes-starter/storefront-lite/templates/index.tera"),
            )),
            &[
                (
                    "assets/theme.css",
                    include_bytes!("../../../themes-starter/storefront-lite/assets/theme.css"),
                ),
                (
                    "assets/images/objects.svg",
                    include_bytes!(
                        "../../../themes-starter/storefront-lite/assets/images/objects.svg"
                    ),
                ),
            ],
        ),
        (
            "technology",
            include_str!("../../../themes-starter/technology/manifest.toml"),
            include_str!("../../../themes-starter/technology/tokens.json"),
            include_str!("../../../themes-starter/technology/layout.json"),
            None,
            &[(
                "assets/theme.css",
                include_bytes!("../../../themes-starter/technology/assets/theme.css"),
            )],
        ),
    ];

    sets
}

/// Build portable packages from the five embedded starter directories.
///
/// # Panics
/// If an embedded package is invalid; unit tests cover each package.
#[must_use]
pub fn embedded_packages() -> Vec<EmbeddedTheme> {
    let sets = embedded_sets();
    let mut out = Vec::with_capacity(sets.len());
    for (dir, manifest, tokens, layout, template, files) in sets {
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("manifest.toml", manifest.as_bytes()),
            ("tokens.json", tokens.as_bytes()),
            ("layout.json", layout.as_bytes()),
        ];
        if let Some((path, src)) = template {
            entries.push((path, src.as_bytes()));
        }
        entries.extend_from_slice(files);
        let bytes = zip_entries(&entries);
        let name = parse_vytheme(&bytes)
            .unwrap_or_else(|e| panic!("embedded {dir} package invalid: {e}"))
            .manifest
            .name;
        out.push(EmbeddedTheme { name, bytes });
    }
    out
}

/// Option recording every starter this site has ever had installed.
///
/// A starter an operator deletes must stay deleted. The list is of
/// starters *seen*, not starters dismissed, so the delete path needs no
/// hook: anything named here and absent from `themes` was removed on
/// purpose, and anything absent from both is new in this release.
pub const STARTERS_SEEN_OPTION: &str = "starter_themes_seen";

/// Installs starters this site has never seen, without modifying
/// existing versions or resurrecting deleted ones. Activates the blog
/// default only when no theme exists yet.
///
/// # Errors
/// Returns [`AppError`](vyasa_common::AppError) on repository failure;
/// per-package install conflicts are skipped (already installed).
pub async fn bootstrap_starters(
    themes: &vyasa_db::repo::ThemesRepo,
    options: &vyasa_db::repo::OptionsRepo,
    cache: Option<&crate::render_cache::RenderCache>,
) -> Result<(), vyasa_common::AppError> {
    let recorded = options
        .get_or_default(STARTERS_SEEN_OPTION, serde_json::Value::Null)
        .await?;
    // No record yet means this is the first boot of a release that keeps
    // one: every starter the old code would have installed is installed
    // this once, then remembered. (The old code reinstalled them on every
    // boot, so on an upgraded site they are all present already.)
    let mut seen: std::collections::BTreeSet<String> = recorded
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let before = seen.clone();
    // A site with no themes at all cannot render a page, so the record of
    // deleted starters gives way: that is a fresh install, not a choice.
    let no_themes = themes.list().await?.is_empty();
    install_starters(themes, cache, |name| no_themes || !before.contains(name)).await?;
    // Everything embedded is now either installed or deliberately gone,
    // so all of it counts as seen. Recorded after the installs: a failed
    // install is retried next boot rather than remembered as dismissed.
    for pkg in embedded_packages() {
        seen.insert(pkg.name);
    }
    let next = serde_json::json!(seen);
    if recorded != next {
        options.set(STARTERS_SEEN_OPTION, &next).await?;
    }
    Ok(())
}

/// Installs every embedded starter `wanted` says yes to and that is not
/// already installed, activating the blog on a site with no themes.
async fn install_starters(
    themes: &vyasa_db::repo::ThemesRepo,
    cache: Option<&crate::render_cache::RenderCache>,
    wanted: impl Fn(&str) -> bool,
) -> Result<(), vyasa_common::AppError> {
    let installed = themes.list().await?;
    let first_boot = installed.is_empty();
    let mut activate_name = String::new();
    let mut activate_version = 0i32;
    for pkg in embedded_packages() {
        if installed.iter().any(|theme| theme.name == pkg.name) || !wanted(&pkg.name) {
            continue;
        }
        let parsed = match parse_vytheme(&pkg.bytes) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("skipping embedded theme {}: {e}", pkg.name);
                continue;
            }
        };
        let templates = if parsed.templates.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&parsed.templates).unwrap_or_default())
        };
        // Already-installed versions are skipped silently (idempotent boot).
        // The starter's own stylesheet and bundled files go in exactly as
        // an upload's would; for years the embedded path dropped both.
        let package_version = i32::try_from(parsed.manifest.version).unwrap_or(1);
        match themes
            .insert_version(
                &parsed.manifest.name,
                themes.latest_version(&parsed.manifest.name).await? + 1,
                parsed.tokens_json,
                parsed.layout_json,
                templates,
                parsed.assets_json,
                None,
            )
            .await
        {
            Ok(row) => {
                themes.set_package_version(row.id, package_version).await?;
                let inputs: Vec<vyasa_db::repo::ThemeFileInput> = parsed
                    .files
                    .iter()
                    .map(|f| vyasa_db::repo::ThemeFileInput {
                        path: f.path.clone(),
                        content_type: f.content_type.to_owned(),
                        sha256: hex::encode(sha2::Sha256::digest(&f.bytes)),
                        bytes: f.bytes.clone(),
                    })
                    .collect();
                if !inputs.is_empty() {
                    themes.put_files(row.id, &inputs).await?;
                }
            }
            Err(vyasa_common::AppError::Conflict { .. }) => {}
            Err(other) => return Err(other),
        }

        if first_boot && parsed.manifest.name == "blog" {
            activate_name = parsed.manifest.name.clone();
            activate_version = themes.latest_version(&parsed.manifest.name).await?;
        }
    }
    if !activate_name.is_empty() {
        themes.set_active(&activate_name, activate_version).await?;
        if let Some(c) = cache {
            c.invalidate_theme();
        }
        tracing::info!("first boot: activated default theme '{activate_name}'");
    }
    Ok(())
}

/// Seeds a starter navigation menu on a site that has none.
///
/// Every starter theme addresses the menu `main`, and a `menu` block
/// whose menu does not exist renders nothing — so a brand-new site had
/// no navigation at all. One Home link is not much, but it is a *real*
/// menu the operator can add to in the admin, which the hard-coded
/// `nav` region never was.
///
/// No-op once any menu exists, matching the theme bootstrap beside it:
/// this is a first-boot convenience, never something that reappears
/// after someone deletes it.
///
/// # Errors
/// Returns [`AppError`](vyasa_common::AppError) on repository failure.
pub async fn bootstrap_menu(
    menus: &vyasa_db::repo::MenusRepo,
) -> Result<(), vyasa_common::AppError> {
    let service = vyasa_core::menu::MenuService::new(menus.clone());
    if !service.list().await?.is_empty() {
        return Ok(());
    }
    let draft = vyasa_core::menu::MenuDraft {
        slug: "main".to_owned(),
        name: "Main".to_owned(),
        location: Some("header".to_owned()),
        items: vec![vyasa_core::menu::MenuDraftItem {
            label: "Home".to_owned(),
            url: "/".to_owned(),
            children: Vec::new(),
        }],
    };
    service.apply_draft(&draft).await?;
    tracing::info!("first boot: created the default 'main' menu");
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::pedantic, clippy::unwrap_used)]

    use super::*;

    use vyasa_testkit::TestDb;
    use vyasa_themes::{Engine, Layout, PageContext, SiteMeta, TemplateType, TokenSet};

    #[test]
    fn all_starter_packages_are_valid_and_named() {
        let pkgs = embedded_packages();
        assert_eq!(pkgs.len(), 5);
        let names: Vec<&str> = pkgs.iter().map(|p| p.name.as_str()).collect();
        for expected in ["blog", "portfolio", "docs", "storefront-lite", "technology"] {
            assert!(names.contains(&expected), "missing {expected}: {names:?}");
        }
    }

    #[test]
    fn packages_have_distinct_tokens_and_layouts() {
        let pkgs = embedded_packages();
        let mut token_sets = std::collections::BTreeSet::new();
        let mut layouts = std::collections::BTreeSet::new();
        for p in &pkgs {
            let parsed = vyasa_themes::package::parse_vytheme(&p.bytes).expect("valid package");
            assert!(
                parsed
                    .assets_json
                    .as_ref()
                    .and_then(|a| a["css"].as_str())
                    .is_some_and(|css| css.contains("--vy-color")),
                "{} must ship token-driven CSS",
                p.name
            );
            token_sets.insert(parsed.tokens_json.to_string());
            layouts.insert(parsed.layout_json.to_string());
            if p.name == "docs" {
                assert!(
                    parsed.templates.contains_key("page.html"),
                    "docs override registered under page.html"
                );
            }
            if p.name == "storefront-lite" {
                assert!(
                    parsed.templates.contains_key("index.html"),
                    "storefront override registered under index.html"
                );
            }
        }
        assert_eq!(token_sets.len(), 5, "token sets must differ per theme");
        assert_eq!(layouts.len(), 5, "layouts must differ per theme");
    }

    /// First-boot bootstrap against the shared test database.
    #[tokio::test]
    async fn first_boot_installs_and_activates_blog() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let themes = vyasa_db::repo::ThemesRepo::new(pool.clone());
        let options = vyasa_db::repo::OptionsRepo::new(pool.clone());
        bootstrap_starters(&themes, &options, None)
            .await
            .expect("bootstrap");
        // Blog active out of the box; all five installed.
        let active = themes
            .get_active()
            .await
            .expect("query")
            .expect("active theme");
        assert_eq!(active.name, "blog");
        assert_eq!(themes.list().await.expect("list").len(), 5);
        // The portfolio starter ships a picture, and it must arrive with
        // the row rather than being dropped on the embedded path.
        let portfolio = themes
            .list()
            .await
            .expect("list")
            .into_iter()
            .find(|t| t.name == "portfolio")
            .expect("portfolio installed");
        assert_eq!(
            themes.file_paths(portfolio.id).await.expect("files"),
            vec!["images/hero.svg".to_owned()]
        );

        // Second boot is a no-op (does not duplicate or re-activate).
        sqlx::query("UPDATE themes SET is_active = false WHERE name = 'docs'")
            .execute(&pool)
            .await
            .expect("deactivate docs");
        bootstrap_starters(&themes, &options, None)
            .await
            .expect("second boot");
        assert_eq!(themes.list().await.expect("list").len(), 5);
        pool.close().await;
    }

    /// Starters new in a release are offered; starters an operator
    /// deleted stay deleted.
    #[tokio::test]
    async fn deleted_starters_stay_deleted_and_new_ones_arrive() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let themes = vyasa_db::repo::ThemesRepo::new(pool.clone());
        let options = vyasa_db::repo::OptionsRepo::new(pool.clone());
        let names = |themes: Vec<vyasa_db::repo::themes::ThemeRow>| {
            let mut n: Vec<String> = themes.into_iter().map(|t| t.name).collect();
            n.sort();
            n.dedup();
            n
        };

        bootstrap_starters(&themes, &options, None)
            .await
            .expect("first boot");
        assert_eq!(themes.list().await.expect("list").len(), 5);
        assert_eq!(
            themes.get_active().await.expect("q").expect("active").name,
            "blog"
        );

        // An operator deletes a starter: it must not come back on the
        // next boot, nor the one after (commit 6c3197e brought it back on
        // every restart).
        themes.set_active("docs", 1).await.expect("choose docs");
        sqlx::query("DELETE FROM themes WHERE name = 'technology'")
            .execute(&pool)
            .await
            .expect("operator deletes technology");
        for _ in 0..2 {
            bootstrap_starters(&themes, &options, None)
                .await
                .expect("reboot");
            let now = names(themes.list().await.expect("list"));
            assert!(!now.contains(&"technology".to_owned()), "{now:?}");
            assert_eq!(now.len(), 4);
        }

        // A starter this site has never seen (new in a release) is still
        // offered on upgrade, without touching the chosen theme.
        let seen: Vec<String> =
            serde_json::from_value(options.get(STARTERS_SEEN_OPTION).await.expect("recorded"))
                .expect("list");
        let fewer: Vec<&String> = seen.iter().filter(|n| *n != "portfolio").collect();
        options
            .set(STARTERS_SEEN_OPTION, &serde_json::json!(fewer))
            .await
            .expect("simulate older release");
        sqlx::query("DELETE FROM themes WHERE name = 'portfolio'")
            .execute(&pool)
            .await
            .expect("absent from the older release");
        bootstrap_starters(&themes, &options, None)
            .await
            .expect("upgrade");
        let now = names(themes.list().await.expect("list"));
        assert!(now.contains(&"portfolio".to_owned()), "{now:?}");
        assert!(!now.contains(&"technology".to_owned()), "{now:?}");
        assert_eq!(
            themes.get_active().await.expect("q").expect("active").name,
            "docs"
        );

        // With every theme gone the site cannot render, so the starters
        // come back despite the record, and the blog is live again.
        sqlx::query("TRUNCATE themes CASCADE")
            .execute(&pool)
            .await
            .expect("wipe all");
        bootstrap_starters(&themes, &options, None)
            .await
            .expect("empty boot");
        assert_eq!(themes.list().await.expect("list").len(), 5);
        assert_eq!(
            themes.get_active().await.expect("q").expect("active").name,
            "blog"
        );
        pool.close().await;
    }

    /// Render fixture: portfolio single renders its case-study template.
    #[test]
    fn portfolio_renders_case_study_snapshot() {
        use std::future::ready;

        let pkg = embedded_packages()
            .into_iter()
            .find(|p| p.name == "portfolio")
            .expect("portfolio present");
        let parsed = parse_vytheme(&pkg.bytes).expect("valid");
        let mut engine = Engine::builtin().expect("engine");
        let overrides: Vec<(&str, &str)> = parsed
            .templates
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        engine
            .install_theme_templates(&overrides)
            .expect("override installs");
        let layout: Layout = serde_json::from_value(parsed.layout_json).expect("layout json");
        let tokens: TokenSet = serde_json::from_value(parsed.tokens_json).expect("tokens json");

        struct NullQ;
        impl vyasa_themes::ContentQueries for NullQ {
            fn entries<'a>(
                &'a self,
                _source: &'a str,
                _sort: vyasa_themes::EntrySort,
                _limit: u32,
                _term: Option<&'a str>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<Vec<vyasa_themes::PostCardData>, String>,
                        > + Send
                        + 'a,
                >,
            > {
                Box::pin(ready(Ok(Vec::new())))
            }
            fn categories<'a>(
                &'a self,
                _s: bool,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<Vec<vyasa_themes::TermLinkData>, String>,
                        > + Send
                        + 'a,
                >,
            > {
                Box::pin(ready(Ok(Vec::new())))
            }
            fn popular_tags<'a>(
                &'a self,
                _c: u32,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<Vec<vyasa_themes::TermLinkData>, String>,
                        > + Send
                        + 'a,
                >,
            > {
                Box::pin(ready(Ok(Vec::new())))
            }
            fn monthly_archives<'a>(
                &'a self,
                _m: u32,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<Vec<vyasa_themes::MonthArchiveData>, String>,
                        > + Send
                        + 'a,
                >,
            > {
                Box::pin(ready(Ok(Vec::new())))
            }
            fn navigation_menu<'a>(
                &'a self,
                _s: &'a str,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>,
            > {
                Box::pin(ready(Ok(String::new())))
            }
            fn approved_comments<'a>(
                &'a self,
                _p: i64,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<Vec<vyasa_themes::CommentNodeData>, String>,
                        > + Send
                        + 'a,
                >,
            > {
                Box::pin(ready(Ok(Vec::new())))
            }
            fn page_tree<'a>(
                &'a self,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<
                            Output = Result<Vec<vyasa_themes::PageNodeData>, String>,
                        > + Send
                        + 'a,
                >,
            > {
                Box::pin(ready(Ok(Vec::new())))
            }
        }

        let blocks = vec![vyasa_core::Block {
            kind: vyasa_core::block::BlockKind::Image,
            plugin_kind: None,
            attrs: serde_json::json!({"url": "https://cdn.example.com/work.jpg", "alt": "case study"}),
            children: Vec::new(),
        }];
        let page = PageContext {
            title: "Case Study".into(),
            post_title: Some("Case Study".into()),
            regions_extra: vec![(
                "content".to_owned(),
                vyasa_themes::render_blocks(&blocks).expect("blocks"),
            )],
            ..PageContext::default()
        };
        let html =
            futures::executor::block_on(vyasa_themes::render_page(vyasa_themes::RenderRequest {
                engine: &engine,
                layout: &layout,
                registry: &vyasa_themes::builtin_registry(),
                tokens: &tokens,
                template: TemplateType::Single,
                site: &SiteMeta::default(),
                page: &page,
                queries: &NullQ,
                post_id: Some(1),
                content_blocks: Some(&blocks),
                reply_to: None,
                editor: false,
            }))
            .expect("renders");
        assert!(html.contains("portfolio-case"), "override template used");
        assert!(html.contains("work.jpg"), "gallery image rendered");
        assert!(
            html.contains("--vy-color-primary: #305a3b"),
            "portfolio tokens applied"
        );
    }
}
