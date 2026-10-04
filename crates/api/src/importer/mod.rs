//! WordPress → Vyasa importer (feature `importer`).
//!
//! Reads a source WP database **read-only** through [`wp::WpSource`],
//! converts post HTML to block documents ([`html_to_blocks`]), maps
//! entities while remembering `source-id → new-id` pairs in row meta for
//! idempotent re-runs, and downloads media attachments with bounded
//! concurrency.

pub mod html_to_blocks;
pub mod media;
pub mod pg_source;
mod rcdom;
pub mod wp;

use std::collections::HashMap;

use vyasa_common::AppError;

/// Knobs for one import run.
#[derive(Debug, Clone, Default)]
pub struct ImportOptions {
    /// Base URL of the source uploads directory (joins with attachment
    /// `_wp_attached_file` meta values).
    pub media_url: Option<String>,
    /// Parse + report only; write nothing.
    pub dry_run: bool,
    /// Concurrent media downloads (bounded, default 4).
    pub media_concurrency: usize,
}

/// Counters returned by a completed run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// Users created.
    pub users: usize,
    /// Posts + pages imported.
    pub posts: usize,
    /// Terms imported.
    pub terms: usize,
    /// Comments imported.
    pub comments: usize,
    /// Media items fetched.
    pub media: usize,
    /// Posts whose conversion emitted warnings (need manual review).
    pub warnings: Vec<String>,
    /// Entities skipped because already imported (idempotency).
    pub skipped: usize,
}

impl ImportReport {
    /// Human-readable summary printed by the CLI.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "import complete: {} users, {} posts, {} terms, {} comments, \
             {} media, {} skipped",
            self.users, self.posts, self.terms, self.comments, self.media, self.skipped,
        )
    }
}

/// Source-id map key stored in row meta:
/// `_wp_import:{kind}:{source_id}`.
#[must_use]
pub fn import_meta_key(kind: &str, source_id: i64) -> String {
    format!("_wp_import:{kind}:{source_id}")
}

/// Convenience: build meta containing the import pointer.
#[must_use]
pub fn meta_with(kind: &str, source_id: i64) -> serde_json::Value {
    serde_json::json!({ import_meta_key(kind, source_id): source_id })
}

/// Runs a full import from `source` into the destination pool.
///
/// # Errors
/// Returns [`AppError::Db`] on repository failures; per-entity problems are
/// collected into [`ImportReport::warnings`] instead of aborting.
pub async fn run<S: wp::WpSource>(
    source: &S,
    dest: &sqlx::PgPool,
    opts: &ImportOptions,
) -> Result<ImportReport, AppError> {
    let mut report = ImportReport::default();
    let dest_pool = vyasa_db::repo::PostsRepo::new(dest.clone());
    let users_repo = vyasa_db::repo::UsersRepo::new(dest.clone());
    let terms_repo = vyasa_db::repo::TermsRepo::new(dest.clone());
    let comments_repo = vyasa_db::repo::CommentsRepo::new(dest.clone());

    // ---- users ----
    let mut user_map: HashMap<i64, i64> = HashMap::new();
    for wu in source.users().await? {
        // Idempotency by email (unique in both systems).
        match users_repo.get_by_email(&wu.email).await {
            // An account that has not confirmed its address is not this
            // author: their posts go to the fallback administrator.
            Ok(row) if row.email_verified_at.is_none() => {
                report.warnings.push(format!(
                    "user {}: the account here with this address is not confirmed; \
                     their posts are attributed to an administrator",
                    wu.login
                ));
                continue;
            }
            Ok(row) => {
                user_map.insert(wu.id, row.id);
                report.skipped += 1;
                continue;
            }
            Err(AppError::NotFound { .. }) => {}
            Err(e) => return Err(e),
        }
        if opts.dry_run {
            report.users += 1;
            continue;
        }
        // Random unusable password: hashes of random bytes; reset flow is
        // external to this phase.
        let hash = vyasa_core::user::password::hash_password(&crate::importer::random_password())
            .unwrap_or_default();
        match users_repo
            .insert(&vyasa_db::repo::NewUser {
                id: vyasa_common::next_id_i64(),
                email: &wu.email,
                username: &wu.login,
                display_name: &wu.display_name,
                password_hash: Some(&hash),
                role: vyasa_db::models::Role::Author,
                bio: "",
            })
            .await
        {
            Ok(row) => {
                user_map.insert(wu.id, row.id);
                report.users += 1;
            }
            Err(e) => report.warnings.push(format!("user {}: {e}", wu.login)),
        }
    }

    // ---- terms ----
    let mut term_map: HashMap<i64, i64> = HashMap::new();
    for wt in source.terms().await? {
        let key = import_meta_key("term", wt.id);
        if let Some(existing) = terms_repo.find_id_by_import_key(&key).await? {
            term_map.insert(wt.id, existing);
            report.skipped += 1;
            continue;
        }
        if opts.dry_run {
            report.terms += 1;
            continue;
        }
        let taxonomy = match wt.taxonomy.as_str() {
            "category" => vyasa_db::content_models::Taxonomy::Category,
            _ => vyasa_db::content_models::Taxonomy::Tag,
        };
        let meta = meta_with("term", wt.id);
        match terms_repo
            .insert(&vyasa_db::repo::NewTerm {
                id: vyasa_common::next_id_i64(),
                taxonomy,
                name: &wt.name,
                slug: &wt.slug,
                parent_id: None,
                meta,
            })
            .await
        {
            Ok(row) => {
                term_map.insert(wt.id, row.id);
                report.terms += 1;
            }
            Err(e) => report.warnings.push(format!("term {}: {e}", wt.slug)),
        }
    }

    // ---- posts (+ pages), newest last so parents pre-exist ----
    let author_fallback = first_admin_id(dest).await?;
    let mut post_map: HashMap<i64, i64> = HashMap::new();
    for wp in source.posts().await? {
        let key = import_meta_key("post", wp.id);
        if let Some(existing) = dest_pool.find_id_by_import_key(&key).await? {
            post_map.insert(wp.id, existing);
            report.skipped += 1;
            continue;
        }
        let (doc, warnings) = html_to_blocks::convert(&wp.content_html);
        if !warnings.is_empty() && !opts.dry_run {
            for w in warnings {
                report
                    .warnings
                    .push(format!("post {} ({}): {w}", wp.id, wp.slug));
            }
        }
        if opts.dry_run {
            report.posts += 1;
            continue;
        }
        let author = *user_map.get(&wp.author_id).unwrap_or(&author_fallback);
        let published = wp.status == "publish";
        let ptype = if wp.post_type == "page" {
            vyasa_db::content_models::PostType::Page
        } else {
            vyasa_db::content_models::PostType::Post
        };
        let mut meta = meta_with("post", wp.id);
        if let Some(obj) = meta.as_object_mut() {
            for (k, v) in &wp.meta {
                // `meta.fields` holds custom field values, which are only
                // ever written through the fields' validation; a WordPress
                // post meta key of that name is dropped, and said so.
                if k == "fields" {
                    report.warnings.push(format!(
                        "post {} ({}): the post meta key \"fields\" was not imported (reserved \
                         for custom field values)",
                        wp.id, wp.slug
                    ));
                    continue;
                }
                obj.insert(k.clone(), v.clone());
            }
        }
        match dest_pool
            .insert(&vyasa_db::repo::NewPost {
                id: vyasa_common::next_id_i64(),
                layout: None,
                post_type: ptype,
                status: if published {
                    vyasa_db::content_models::PostStatus::Published
                } else {
                    vyasa_db::content_models::PostStatus::Draft
                },
                slug: wp.slug.clone(),
                title: wp.title.clone(),
                content: serde_json::to_value(&doc).unwrap_or_default(),
                excerpt: Some(wp.excerpt.clone()),
                author_id: author,
                parent_id: None,
                meta,
                published_at: published.then_some(chrono::Utc::now()),
                scheduled_for: None,
                password_hash: None,
            })
            .await
        {
            Ok(row) => {
                post_map.insert(wp.id, row.id);
                report.posts += 1;
            }
            Err(e) => report.warnings.push(format!("post {}: {e}", wp.slug)),
        }
    }

    // ---- comments ----
    for wc in source.comments().await? {
        let key = import_meta_key("comment", wc.id);
        if comments_repo.has_import_key(&key).await? {
            report.skipped += 1;
            continue;
        }
        if opts.dry_run {
            report.comments += 1;
            continue;
        }
        let Some(post_id) = post_map.get(&wc.post_id) else {
            continue;
        };
        let import_key = import_meta_key("comment", wc.id);
        let parent = wc.parent_id.and_then(|p| post_map.get(&p).copied());
        match comments_repo
            .insert_imported(
                *post_id,
                parent,
                &wc.author_name,
                &wc.author_email,
                &wc.content_html,
                &import_key,
            )
            .await
        {
            Ok(_) => report.comments += 1,
            Err(e) => report.warnings.push(format!("comment {}: {e}", wc.id)),
        }
    }

    // ---- media ----
    if opts.dry_run {
        report.media = source.attachments().await?.len();
    } else if let Some(base) = &opts.media_url {
        let media_dir = std::env::var("VYASA_MEDIA_DIR").unwrap_or_else(|_| "media".to_owned());
        let service = vyasa_core::media::MediaService::new(
            vyasa_db::repo::MediaRepo::new(dest.clone()),
            std::sync::Arc::new(vyasa_core::media::storage::LocalFsBackend::new(
                std::path::PathBuf::from(media_dir),
            )),
            dest.clone(),
        );
        let attachments = source.attachments().await?;
        let items: Vec<(String, String)> = attachments
            .iter()
            .map(|(id, name, path)| {
                let _ = id;
                (
                    name.clone(),
                    format!("{}/{}", base.trim_end_matches('/'), path),
                )
            })
            .collect();
        for (name, result) in crate::importer::media::download_many(
            &service,
            author_fallback,
            items,
            opts.media_concurrency,
        )
        .await
        {
            match result {
                Ok(_) => report.media += 1,
                Err(e) => report.warnings.push(format!("media {name}: {e}")),
            }
        }
    }

    Ok(report)
}

async fn first_admin_id(pool: &sqlx::PgPool) -> Result<i64, AppError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM users WHERE role = 'admin' AND email_verified_at IS NOT NULL
         ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::db(e.to_string()))?
    .ok_or_else(|| AppError::validation("destination has no admin user; create one first"))
}

/// Cryptographically-random ASCII password (unusable-by-design accounts).
fn random_password() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("!wp-import-{nanos}-disabled")
}

/// CLI entry: `vyasa import wp --database-url … [--media-url …] [--dry-run]`.
///
/// # Errors
/// Returns [`AppError`] on connect/validation failure; entity-level issues
/// are reported in the summary instead of failing the run.
pub async fn run_cli(
    config: &vyasa_common::VyasaConfig,
    wp_database_url: &str,
    media_url: Option<String>,
    dry_run: bool,
) -> Result<ImportReport, AppError> {
    let source = crate::importer::pg_source::PgWpSource::connect(wp_database_url).await?;
    let pool = vyasa_db::pool(config).await?;
    let opts = ImportOptions {
        media_url,
        dry_run,
        media_concurrency: 4,
    };
    let report = run(&source, &pool, &opts).await?;
    eprintln!("{}", report.summary());
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
    Ok(report)
}

#[cfg(all(test, feature = "importer"))]
mod wp_import_tests {
    #![allow(clippy::pedantic, clippy::unwrap_used)]

    use super::{run, ImportOptions};
    use crate::importer::wp::WpSource;
    use vyasa_common::AppError;
    use vyasa_testkit::TestDb;

    struct FixtureSource {
        users: Vec<crate::importer::wp::WpUser>,
        posts: Vec<crate::importer::wp::WpPost>,
        terms: Vec<crate::importer::wp::WpTerm>,
        comments: Vec<crate::importer::wp::WpComment>,
    }

    impl WpSource for FixtureSource {
        async fn users(&self) -> Result<Vec<crate::importer::wp::WpUser>, AppError> {
            Ok(self.users.clone())
        }
        async fn posts(&self) -> Result<Vec<crate::importer::wp::WpPost>, AppError> {
            Ok(self.posts.clone())
        }
        async fn terms(&self) -> Result<Vec<crate::importer::wp::WpTerm>, AppError> {
            Ok(self.terms.clone())
        }
        async fn comments(&self) -> Result<Vec<crate::importer::wp::WpComment>, AppError> {
            Ok(self.comments.clone())
        }
    }

    fn fixture() -> FixtureSource {
        use crate::importer::wp::{WpComment, WpPost, WpTerm, WpUser};
        FixtureSource {
            users: vec![WpUser {
                id: 1,
                login: "alice".into(),
                email: "alice@example.com".into(),
                display_name: "Alice Author".into(),
            }],
            posts: vec![
                WpPost {
                    id: 100,
                    post_type: "post".into(),
                    status: "publish".into(),
                    slug: "wp-hello".into(),
                    title: "Hello from WP".into(),
                    content_html: "<h2>Welcome</h2><p>This is <strong>bold</strong> text \
                                   with a <a href=\"https://example.com\">link</a>.</p>\
                                   <ul><li>one</li><li>two</li></ul>\
                                   <figure><img src=\"https://cdn.example.com/pic.jpg\" alt=\"pic\">\
                                   <figcaption>a pic</figcaption></figure>\
                                   <blockquote><p>quoted words</p></blockquote>"
                        .into(),
                    excerpt: "an excerpt".into(),
                    author_id: 1,
                    meta: Vec::new(),
                    term_ids: Vec::new(),
                },
                WpPost {
                    id: 101,
                    post_type: "page".into(),
                    status: "publish".into(),
                    slug: "about-page".into(),
                    title: "About".into(),
                    content_html: "<p>About us page body.</p>".into(),
                    excerpt: String::new(),
                    author_id: 1,
                    meta: Vec::new(),
                    term_ids: Vec::new(),
                },
            ],
            terms: vec![
                WpTerm { id: 10, taxonomy: "category".into(), name: "News".into(), slug: "news".into() },
                WpTerm { id: 11, taxonomy: "post_tag".into(), name: "rust".into(), slug: "rust".into() },
            ],
            comments: vec![WpComment {
                id: 500,
                post_id: 100,
                parent_id: None,
                author_name: "Bob".into(),
                author_email: "bob@example.com".into(),
                content_html: "<p>Nice site!</p>".into(),
            }],
        }
    }

    fn opts(dry_run: bool) -> ImportOptions {
        ImportOptions {
            media_url: None,
            dry_run,
            media_concurrency: 4,
        }
    }

    #[tokio::test]
    async fn wp_import_counts_blocks_and_idempotency() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let users = vyasa_db::repo::UsersRepo::new(pool.clone());
        let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
        users
            .insert(&vyasa_db::repo::NewUser {
                id: 83_001,
                email: "admin-import@example.com",
                username: "adminimport",
                display_name: "AdminImport",
                password_hash: Some(&hash),
                role: vyasa_db::models::Role::Admin,
                bio: "",
            })
            .await
            .expect("seed admin");

        let source = fixture();

        // Dry run: counts only.
        let dry = run(&source, &pool, &opts(true)).await.expect("dry run");
        assert_eq!(dry.posts, 2);
        assert_eq!(dry.terms, 2);
        assert_eq!(dry.comments, 1);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(count, 0, "dry-run must not write");

        // Real run.
        let report = run(&source, &pool, &opts(false)).await.expect("import");
        assert_eq!(report.users, 1);
        assert_eq!(report.posts, 2);
        assert_eq!(report.terms, 2);
        assert_eq!(report.comments, 1);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        // Converted blocks for the sample post.
        let row: (serde_json::Value,) =
            sqlx::query_as("SELECT content FROM posts WHERE slug = 'wp-hello'")
                .fetch_one(&pool)
                .await
                .expect("row");
        let doc = row.0;
        assert_eq!(doc["schema_version"], 1);
        let kinds: Vec<&str> = doc["blocks"]
            .as_array()
            .expect("array")
            .iter()
            .map(|b| b["kind"].as_str().expect("kind"))
            .collect();
        assert!(kinds.contains(&"heading"), "{kinds:?}");
        assert!(kinds.contains(&"list"), "{kinds:?}");
        assert!(kinds.contains(&"image"), "{kinds:?}");
        assert!(kinds.contains(&"quote"), "{kinds:?}");
        let para = doc["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["kind"] == "paragraph")
            .expect("paragraph");
        let para_s = para.to_string();
        assert!(para_s.contains("<strong>bold</strong>"), "{para_s}");
        assert!(para_s.contains("https://example.com"), "{para_s}");

        // Re-run is idempotent.
        let rerun = run(&source, &pool, &opts(false)).await.expect("rerun");
        assert_eq!(rerun.posts, 0);
        assert_eq!(rerun.users, 0);
        assert_eq!(
            rerun.skipped, 6,
            "2 posts + 1 user + 2 terms + 1 comment skipped"
        );
        pool.close().await;
    }

    /// A WordPress author whose address belongs to an account here that
    /// has not confirmed it does not become that account: the posts go to
    /// the administrator, and the report says so.
    #[tokio::test]
    async fn an_unconfirmed_account_does_not_inherit_imported_posts() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        let users = vyasa_db::repo::UsersRepo::new(pool.clone());
        users
            .insert(&vyasa_db::repo::NewUser {
                id: 83_001,
                email: "admin-import@example.com",
                username: "adminimport",
                display_name: "AdminImport",
                password_hash: None,
                role: vyasa_db::models::Role::Admin,
                bio: "",
            })
            .await
            .expect("seed admin");
        users
            .insert_unconfirmed(
                &vyasa_db::repo::NewUser {
                    id: 83_002,
                    email: "Alice@example.com",
                    username: "alice-a2b3c4",
                    display_name: "Pending Alice",
                    password_hash: None,
                    role: vyasa_db::models::Role::Subscriber,
                    bio: "",
                },
                None,
            )
            .await
            .expect("unconfirmed account");

        let report = run(&fixture(), &pool, &opts(false)).await.expect("import");
        assert_eq!(report.posts, 2);
        assert_eq!(report.users, 0);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("alice") && w.contains("not confirmed")),
            "{:?}",
            report.warnings
        );
        let authors: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT author_id FROM posts")
            .fetch_all(&pool)
            .await
            .expect("authors");
        assert_eq!(authors, vec![83_001]);
        pool.close().await;
    }

    /// A WordPress post meta key named `fields` would land in
    /// `meta.fields`, where custom field values live, without passing
    /// their validation. It is dropped and reported; other keys import.
    #[tokio::test]
    async fn a_meta_key_named_fields_is_not_imported_unchecked() {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        vyasa_db::repo::UsersRepo::new(pool.clone())
            .insert(&vyasa_db::repo::NewUser {
                id: 83_001,
                email: "admin-import@example.com",
                username: "adminimport",
                display_name: "AdminImport",
                password_hash: None,
                role: vyasa_db::models::Role::Admin,
                bio: "",
            })
            .await
            .expect("seed admin");
        let mut source = fixture();
        source.posts[0].meta = vec![
            (
                "fields".into(),
                serde_json::json!({"link": "javascript:alert(1)"}),
            ),
            ("subtitle".into(), serde_json::json!("kept")),
        ];
        let report = run(&source, &pool, &opts(false)).await.expect("import");
        assert_eq!(report.posts, 2);
        assert!(
            report.warnings.iter().any(|w| w.contains("\"fields\"")),
            "{:?}",
            report.warnings
        );
        let meta: serde_json::Value =
            sqlx::query_scalar("SELECT meta FROM posts WHERE slug = 'wp-hello'")
                .fetch_one(&pool)
                .await
                .expect("row");
        assert!(meta.get("fields").is_none(), "{meta}");
        assert_eq!(meta["subtitle"], "kept");
        pool.close().await;
    }
}
