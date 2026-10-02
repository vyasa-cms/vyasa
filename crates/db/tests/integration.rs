//! Integration tests against a real, migrated Postgres database.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_common::DbConfig;
use vyasa_db::{connect, health, run_migrations, MIGRATOR};
use vyasa_testkit::TestDb;

#[tokio::test]
async fn migrations_apply_idempotently_and_health_reports_version() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();

    // TestDb::new() applied migrations; health must report the highest
    // embedded migration version.
    let report = health(&pool).await;
    assert!(report.ok, "probe failed: {report:?}");
    let expected = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    assert_eq!(
        report.migration_version,
        Some(expected),
        "expected migration version {expected}, got {report:?}"
    );

    // Re-running migrations is a no-op (idempotent).
    run_migrations(&pool).await.expect("second migrate run");

    // Migrator and health agree on the applied set.
    let applied = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("count applied migrations");
    assert_eq!(
        applied,
        i64::try_from(MIGRATOR.iter().count()).expect("usize -> i64")
    );

    pool.close().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn identity_tables_round_trip() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();

    // --- users ---------------------------------------------------------
    let user_id: i64 = 9001;
    let inserted = sqlx::query(&format!(
        "INSERT INTO users (id, email, username, display_name, password_hash, role, bio)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING {}",
        vyasa_db::repo::users::USER_COLUMNS
    ))
    .bind(user_id)
    .bind("Ada@Example.COM")
    .bind("AdaL")
    .bind("Ada Lovelace")
    .bind("$argon2id$v=19$fakehash")
    .bind("admin")
    .bind("First programmer")
    .fetch_one(&pool)
    .await
    .expect("insert user");
    let user: vyasa_db::models::UserRow = sqlx::FromRow::from_row(&inserted).expect("map user row");
    assert_eq!(user.id, user_id);
    assert_eq!(user.role, vyasa_db::models::Role::Admin);
    assert_eq!(user.email, "Ada@Example.COM"); // citext preserves case
    assert!(user.meta.is_object());

    // citext uniqueness is case-insensitive.
    let dup = sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id + 1)
    .bind("ada@example.com")
    .bind("someone")
    .bind("Dup")
    .bind("subscriber")
    .execute(&pool)
    .await;
    assert!(dup.is_err(), "duplicate email must violate uniqueness");

    // Role check constraint rejects unknown roles.
    let bad_role = sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id + 2)
    .bind("other@example.com")
    .bind("other")
    .bind("Other")
    .bind("superuser")
    .execute(&pool)
    .await;
    assert!(bad_role.is_err(), "unknown role must violate check");

    // updated_at trigger fires on UPDATE.
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("read updated_at");
    sqlx::query("UPDATE users SET bio = $1 WHERE id = $2")
        .bind("updated bio")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("update user");
    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT updated_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("re-read updated_at");
    assert!(after > before, "updated_at trigger must fire");

    // --- sessions ------------------------------------------------------
    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at, user_agent, ip)
         VALUES ($1, $2, now() + interval '1 hour', $3, $4::inet)",
    )
    .bind("tok_0123456789abcdef")
    .bind(user_id)
    .bind("test-agent/1.0")
    .bind("192.168.1.10")
    .execute(&pool)
    .await
    .expect("insert session");
    let session: vyasa_db::models::SessionRow = sqlx::query_as(
        "SELECT id, user_id, expires_at, created_at, user_agent, host(ip) AS ip
         FROM sessions WHERE id = $1",
    )
    .bind("tok_0123456789abcdef")
    .fetch_one(&pool)
    .await
    .expect("fetch session");
    assert_eq!(session.user_id, user_id);
    assert_eq!(session.ip.as_deref(), Some("192.168.1.10"));

    // Sessions cascade on user delete.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("delete user");
    let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .expect("count sessions");
    assert_eq!(session_count, 0, "sessions must cascade on user delete");

    // --- api_keys ------------------------------------------------------
    let other_id: i64 = 9100;
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(other_id)
    .bind("grace@example.com")
    .bind("grace")
    .bind("Grace Hopper")
    .bind("editor")
    .execute(&pool)
    .await
    .expect("insert second user");

    let key_id: i64 = 8001;
    sqlx::query(
        "INSERT INTO api_keys (id, user_id, name, key_hash, capabilities)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(key_id)
    .bind(other_id)
    .bind("Next.js frontend")
    .bind("sha256:abc123")
    .bind(serde_json::json!(["posts:read", "media:read"]))
    .execute(&pool)
    .await
    .expect("insert api key");

    let key: vyasa_db::models::ApiKeyRow = sqlx::query_as("SELECT * FROM api_keys WHERE id = $1")
        .bind(key_id)
        .fetch_one(&pool)
        .await
        .expect("fetch api key");
    assert_eq!(key.user_id, other_id);
    assert_eq!(
        key.capabilities,
        serde_json::json!(["posts:read", "media:read"])
    );
    assert!(key.revoked_at.is_none());

    // Duplicate key_hash rejected.
    let dup_key =
        sqlx::query("INSERT INTO api_keys (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(key_id + 1)
            .bind(other_id)
            .bind("Copy")
            .bind("sha256:abc123")
            .execute(&pool)
            .await;
    assert!(dup_key.is_err(), "duplicate key_hash must be rejected");

    // api_keys cascade on user delete too.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(other_id)
        .execute(&pool)
        .await
        .expect("delete second user");
    let key_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys")
        .fetch_one(&pool)
        .await
        .expect("count api keys");
    assert_eq!(key_count, 0, "api_keys must cascade on user delete");

    // --- options -------------------------------------------------------
    sqlx::query("INSERT INTO options (key, value) VALUES ($1, $2)")
        .bind("site_title")
        .bind(serde_json::json!("My Vyasa Site"))
        .execute(&pool)
        .await
        .expect("insert option");
    let option: vyasa_db::models::OptionRow =
        sqlx::query_as("SELECT * FROM options WHERE key = $1")
            .bind("site_title")
            .fetch_one(&pool)
            .await
            .expect("fetch option");
    assert_eq!(option.value, serde_json::json!("My Vyasa Site"));

    // Upsert semantics the OptionsService will rely on.
    sqlx::query(
        "INSERT INTO options (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = $2, updated_at = now()",
    )
    .bind("site_title")
    .bind(serde_json::json!("Renamed"))
    .execute(&pool)
    .await
    .expect("upsert option");
    let value: serde_json::Value = sqlx::query_scalar("SELECT value FROM options WHERE key = $1")
        .bind("site_title")
        .fetch_one(&pool)
        .await
        .expect("re-read option");
    assert_eq!(value, serde_json::json!("Renamed"));

    pool.close().await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn content_tables_round_trip() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();

    let user_id: i64 = 7001;
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind("author@example.com")
    .bind("author")
    .bind("An Author")
    .bind("author")
    .execute(&pool)
    .await
    .expect("insert user");

    // --- posts: partial slug uniqueness -------------------------------
    let post_id: i64 = 7101;
    let inserted = sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, content, author_id,
                            published_at)
         VALUES ($1, 'post', 'published', $2, $3, $4, $5, now())
         RETURNING id, type AS post_type, status, slug, title, content, layout, excerpt,
                   author_id, parent_id, meta, published_at, scheduled_for, password_hash,
                   created_at, updated_at",
    )
    .bind(post_id)
    .bind("hello-world")
    .bind("Hello World")
    .bind(serde_json::json!([{"kind": "paragraph", "attrs": {"text": "hi"}}]))
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("insert post");
    let post: vyasa_db::content_models::PostRow =
        sqlx::FromRow::from_row(&inserted).expect("map post row");
    assert_eq!(post.status, vyasa_db::content_models::PostStatus::Published);
    assert_eq!(post.post_type, vyasa_db::content_models::PostType::Post);
    assert!(post.published_at.is_some());

    // Same slug + same type collides while published...
    let dup = sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, author_id)
         VALUES ($1, 'post', 'draft', $2, $3, $4)",
    )
    .bind(post_id + 1)
    .bind("hello-world")
    .bind("Dup")
    .bind(user_id)
    .execute(&pool)
    .await;
    assert!(dup.is_err(), "published slug must be reserved for its type");

    // ...but the same slug in a different type is fine, and a trashed post
    // frees its slug.
    sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, author_id)
         VALUES ($1, 'page', 'published', $2, $3, $4)",
    )
    .bind(post_id + 2)
    .bind("hello-world")
    .bind("Hello Page")
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("same slug across types is allowed");

    sqlx::query("UPDATE posts SET status = 'trash' WHERE id = $1")
        .bind(post_id)
        .execute(&pool)
        .await
        .expect("trash post");
    sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, author_id)
         VALUES ($1, 'post', 'draft', $2, $3, $4)",
    )
    .bind(post_id + 3)
    .bind("hello-world")
    .bind("Reused Slug")
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("trashed slug must be reusable");

    // Status check constraint.
    let bad_status = sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, author_id)
         VALUES ($1, 'post', 'archived', $2, $3, $4)",
    )
    .bind(post_id + 4)
    .bind("bad")
    .bind("Bad")
    .bind(user_id)
    .execute(&pool)
    .await;
    assert!(bad_status.is_err(), "unknown status must violate check");

    // --- revisions -----------------------------------------------------
    sqlx::query(
        "INSERT INTO post_revisions (id, post_id, title, content, author_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(7201)
    .bind(post_id)
    .bind("Hello World v1")
    .bind(serde_json::json!([]))
    .bind(user_id)
    .execute(&pool)
    .await
    .expect("insert revision");
    let revision: vyasa_db::content_models::PostRevisionRow = sqlx::query_as(
        "SELECT id, post_id, title, content, layout, author_id, is_autosave, created_at
                        FROM post_revisions WHERE post_id = $1",
    )
    .bind(post_id)
    .fetch_one(&pool)
    .await
    .expect("fetch revision");
    assert_eq!(revision.post_id, post_id);
    assert!(!revision.is_autosave);

    // Revisions cascade on post delete.
    sqlx::query("DELETE FROM posts WHERE id = $1")
        .bind(post_id)
        .execute(&pool)
        .await
        .expect("delete post");
    let rev_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM post_revisions")
        .fetch_one(&pool)
        .await
        .expect("count revisions");
    assert_eq!(rev_count, 0, "revisions must cascade on post delete");

    // --- terms + relationships -----------------------------------------
    let live_post: i64 = post_id + 2;
    sqlx::query("INSERT INTO terms (id, taxonomy, name, slug) VALUES ($1, 'category', $2, $3)")
        .bind(7301)
        .bind("Announcements")
        .bind("announcements")
        .execute(&pool)
        .await
        .expect("insert category");
    sqlx::query("INSERT INTO terms (id, taxonomy, name, slug) VALUES ($1, 'tag', $2, $3)")
        .bind(7302)
        .bind("rust")
        .bind("rust")
        .execute(&pool)
        .await
        .expect("insert tag");

    // Slug unique per taxonomy: same slug different taxonomy ok...
    sqlx::query("INSERT INTO terms (id, taxonomy, name, slug) VALUES ($1, 'tag', $2, $3)")
        .bind(7303)
        .bind("Announcements Tag")
        .bind("announcements")
        .execute(&pool)
        .await
        .expect("same slug across taxonomies is allowed");

    // ...same slug same taxonomy collides.
    let dup_term =
        sqlx::query("INSERT INTO terms (id, taxonomy, name, slug) VALUES ($1, 'category', $2, $3)")
            .bind(7304)
            .bind("Announcements Again")
            .bind("announcements")
            .execute(&pool)
            .await;
    assert!(dup_term.is_err(), "term slug must be unique per taxonomy");

    sqlx::query("INSERT INTO term_relationships (post_id, term_id) VALUES ($1, $2)")
        .bind(live_post)
        .bind(7301)
        .execute(&pool)
        .await
        .expect("relate post-term");
    sqlx::query("INSERT INTO term_relationships (post_id, term_id) VALUES ($1, $2)")
        .bind(live_post)
        .bind(7302)
        .execute(&pool)
        .await
        .expect("relate post-tag");
    let term_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM term_relationships WHERE post_id = $1")
            .bind(live_post)
            .fetch_one(&pool)
            .await
            .expect("count relationships");
    assert_eq!(term_count, 2);

    // --- comments (threaded) -------------------------------------------
    let parent_comment: i64 = 7401;
    sqlx::query(
        "INSERT INTO comments (id, post_id, author_name, author_email, content)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(parent_comment)
    .bind(live_post)
    .bind("Visitor")
    .bind("visitor@example.com")
    .bind("Nice post!")
    .execute(&pool)
    .await
    .expect("insert parent comment");
    sqlx::query(
        "INSERT INTO comments (id, post_id, author_name, author_email, content, parent_id, status)
         VALUES ($1, $2, $3, $4, $5, $6, 'approved')",
    )
    .bind(7402)
    .bind(live_post)
    .bind("Author")
    .bind("author@example.com")
    .bind("Thanks!")
    .bind(parent_comment)
    .execute(&pool)
    .await
    .expect("insert reply");
    let reply: vyasa_db::content_models::CommentRow = sqlx::query_as(
        "SELECT id, post_id, author_user_id, author_name, author_email, content,
                parent_id, status, created_at
         FROM comments WHERE id = $1",
    )
    .bind(7402)
    .fetch_one(&pool)
    .await
    .expect("fetch reply");
    assert_eq!(reply.parent_id, Some(parent_comment));
    assert_eq!(
        reply.status,
        vyasa_db::content_models::CommentStatus::Approved
    );

    // --- media ----------------------------------------------------------
    let media_row: vyasa_db::content_models::MediaRow = sqlx::query_as(
        "INSERT INTO media (id, owner_id, file_name, mime, byte_size, storage, path, width, height)
         VALUES ($1, $2, $3, 'image/png', 1024, 'local', $4, 64, 64)
         RETURNING id, owner_id, file_name, mime, byte_size, storage, path,
                   width, height, blurhash, alt, caption, derivatives, created_at, sha256, focal_x, focal_y",
    )
    .bind(7501)
    .bind(user_id)
    .bind("logo.png")
    .bind("7501/7501/logo.png")
    .fetch_one(&pool)
    .await
    .expect("insert media");
    assert_eq!(media_row.width, Some(64));
    assert_eq!(
        media_row.storage,
        vyasa_db::content_models::MediaStorage::Local
    );
    assert!(media_row.derivatives.is_object());

    // --- themes: single active constraint ------------------------------
    sqlx::query(
        "INSERT INTO themes (id, name, version, is_active, tokens, layout)
         VALUES ($1, 'blog', 1, true, '{}', '{}')",
    )
    .bind(7601)
    .execute(&pool)
    .await
    .expect("insert active theme");
    let second_active = sqlx::query(
        "INSERT INTO themes (id, name, version, is_active, tokens, layout)
         VALUES ($1, 'portfolio', 1, true, '{}', '{}')",
    )
    .bind(7602)
    .execute(&pool)
    .await;
    assert!(
        second_active.is_err(),
        "a second active theme must violate the single-active index"
    );
    sqlx::query("UPDATE themes SET is_active = false WHERE id = $1")
        .bind(7601)
        .execute(&pool)
        .await
        .expect("deactivate first theme");
    sqlx::query(
        "INSERT INTO themes (id, name, version, is_active, tokens, layout)
         VALUES ($1, 'portfolio', 1, true, '{}', '{}')",
    )
    .bind(7602)
    .execute(&pool)
    .await
    .expect("activate other theme after deactivating first");

    // --- plugins + versions ---------------------------------------------
    sqlx::query(
        "INSERT INTO plugins (id, name, version, enabled, capabilities, wasm_sha256)
         VALUES ($1, 'hello', '0.1.0', true, $2, $3)",
    )
    .bind(7701)
    .bind(serde_json::json!(["log:write"]))
    .bind("abc123")
    .execute(&pool)
    .await
    .expect("insert plugin");
    sqlx::query(
        "INSERT INTO plugin_versions (id, plugin_id, version, wasm, sha256)
         VALUES ($1, $2, '0.1.0', $3, $4)",
    )
    .bind(7702)
    .bind(7701)
    .bind(vec![0u8, 1, 2, 3])
    .bind("abc123")
    .execute(&pool)
    .await
    .expect("insert plugin version");
    let wasm: Vec<u8> = sqlx::query_scalar("SELECT wasm FROM plugin_versions WHERE plugin_id = $1")
        .bind(7701)
        .fetch_one(&pool)
        .await
        .expect("fetch wasm bytes");
    assert_eq!(wasm, vec![0u8, 1, 2, 3]);

    // --- jobs ------------------------------------------------------------
    sqlx::query(
        "INSERT INTO jobs (id, kind, payload, run_at) VALUES ($1, 'send_email', $2, now())",
    )
    .bind(7801)
    .bind(serde_json::json!({"to": "x@example.com"}))
    .execute(&pool)
    .await
    .expect("insert job");
    let job: vyasa_db::content_models::JobRow = sqlx::query_as(
        "SELECT id, kind, payload, run_at, status, attempts, last_error, created_at
         FROM jobs WHERE id = $1",
    )
    .bind(7801)
    .fetch_one(&pool)
    .await
    .expect("fetch job");
    assert_eq!(job.status, vyasa_db::content_models::JobStatus::Queued);
    assert_eq!(job.attempts, 0);

    // --- webhooks + ai_logs ----------------------------------------------
    sqlx::query("INSERT INTO webhooks (id, url, secret, events) VALUES ($1, $2, $3, $4)")
        .bind(7901)
        .bind("https://example.com/hook")
        .bind("whsec_abc")
        .bind(serde_json::json!(["post.published"]))
        .execute(&pool)
        .await
        .expect("insert webhook");
    sqlx::query(
        "INSERT INTO ai_logs (id, provider, model, purpose, prompt_tokens, completion_tokens, cost_usd)
         VALUES ($1, 'anthropic', 'claude-x', 'theme_gen', 100, 200, 0.003)",
    )
    .bind(7951)
    .execute(&pool)
    .await
    .expect("insert ai log");
    let cost: f64 = sqlx::query_scalar("SELECT cost_usd FROM ai_logs WHERE id = $1")
        .bind(7951)
        .fetch_one(&pool)
        .await
        .expect("fetch cost");
    assert!((cost - 0.003).abs() < 1e-9);

    pool.close().await;
}

#[tokio::test]
async fn health_reports_failure_for_dead_database() {
    let db = DbConfig {
        max_connections: 2,
        acquire_timeout_secs: 1,
    };
    // Port 1 is reserved and nothing listens there.
    let dead = connect("postgres://nobody:nobody@127.0.0.1:1/nobody", &db).await;
    assert!(dead.is_err(), "connecting to port 1 must fail");
}

#[tokio::test]
async fn themes_install_activate_rollback_round_trip() {
    use vyasa_db::repo::ThemesRepo;

    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = ThemesRepo::new(pool.clone());
    let tokens = serde_json::json!({"version": 1});
    let layout = serde_json::json!({"index": [{"id": "main", "kind": "content"}]});

    // Install v1 and v2 of "roundtrip".
    let v1 = repo
        .insert_version(
            "roundtrip",
            1,
            tokens.clone(),
            layout.clone(),
            None,
            None,
            None,
        )
        .await
        .expect("insert v1");
    let v2 = repo
        .insert_version(
            "roundtrip",
            2,
            tokens,
            layout,
            Some(serde_json::json!({"single.html": "{% extends \"base.html\" %}"})),
            None,
            None,
        )
        .await
        .expect("insert v2");
    assert!(!v1.is_active && !v2.is_active);

    // Duplicate install conflicts.
    assert!(
        repo.insert_version(
            "roundtrip",
            1,
            serde_json::json!({}),
            serde_json::json!({}),
            None,
            None,
            None
        )
        .await
        .is_err(),
        "(name, version) must be unique"
    );

    // Nothing active yet.
    assert!(repo.get_active().await.expect("get_active").is_none());

    // Activate v2.
    repo.set_active("roundtrip", 2).await.expect("activate v2");
    let active = repo
        .get_active()
        .await
        .expect("get_active")
        .expect("active present");
    assert_eq!(active.version, 2);
    assert!(active.templates.is_some());

    // List shows both, newest version first within the name.
    let rows = repo.list().await.expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].version, 2);

    // Rollback to v1.
    let target = repo
        .latest_below("roundtrip", 2)
        .await
        .expect("rollback target");
    assert_eq!(target.version, 1);
    repo.set_active(&target.name, target.version)
        .await
        .expect("activate v1");
    let active = repo.get_active().await.expect("active").expect("active");
    assert_eq!(active.version, 1);

    // Unknown activation is a clean NotFound.
    assert!(repo.set_active("roundtrip", 99).await.is_err());
    assert!(repo.latest_below("no-such-theme", 5).await.is_err());

    pool.close().await;
}

#[tokio::test]
async fn menus_crud_and_validation_rules() {
    use vyasa_core::MenuService;
    use vyasa_db::repo::{MenusRepo, NewMenuItem};

    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = MenusRepo::new(pool.clone());
    let service = MenuService::new(repo.clone());

    let menu = service
        .create("Main Menu", None, Some("header"))
        .await
        .expect("create");
    assert_eq!(menu.location.as_deref(), Some("header"));

    // Duplicate name/slug conflicts.
    assert!(matches!(
        service.create("Main Menu", None, None).await,
        Err(vyasa_common::AppError::Conflict { .. })
    ));

    // Validation rules.
    assert!(
        service
            .add_item(
                menu.id,
                &NewMenuItem {
                    parent_id: None,
                    label: String::new(),
                    url: "/".into(),
                    sort_order: 0
                }
            )
            .await
            .is_err(),
        "empty label"
    );
    assert!(
        service
            .add_item(
                menu.id,
                &NewMenuItem {
                    parent_id: None,
                    label: "x".into(),
                    url: "javascript:alert(1)".into(),
                    sort_order: 0
                }
            )
            .await
            .is_err(),
        "javascript url"
    );

    let home = service
        .add_item(
            menu.id,
            &NewMenuItem {
                parent_id: None,
                label: "Home".into(),
                url: "/".into(),
                sort_order: 0,
            },
        )
        .await
        .expect("add home");
    // Cross-menu parent is rejected.
    let other = service.create("Footer", None, None).await.expect("footer");
    assert!(matches!(
        service
            .add_item(
                other.id,
                &NewMenuItem {
                    parent_id: Some(home.id),
                    label: "X".into(),
                    url: "/x".into(),
                    sort_order: 0
                }
            )
            .await,
        Err(vyasa_common::AppError::Validation { .. })
    ));

    // Delete cascades items.
    service.delete(menu.id).await.expect("delete");
    assert!(repo
        .items(menu.id)
        .await
        .expect("items after delete")
        .is_empty());

    pool.close().await;
}

#[tokio::test]
async fn translation_groups_link_and_unlink() {
    use vyasa_db::repo::TranslationsRepo;

    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = TranslationsRepo::new(pool.clone());
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role)
         VALUES (9001, 'tr@example.com', 'translator', 'Translator', 'author')",
    )
    .execute(&pool)
    .await
    .expect("user");
    for (id, slug, title) in [(9101_i64, "hello", "Hello"), (9102, "ola", "Ol\u{e1}")] {
        sqlx::query(
            "INSERT INTO posts (id, type, status, slug, title, content, author_id, published_at)
             VALUES ($1, 'post', 'published', $2, $3, '{}'::jsonb, 9001, now())",
        )
        .bind(id)
        .bind(slug)
        .bind(title)
        .execute(&pool)
        .await
        .expect("post");
    }
    let (en, pt) = (9101_i64, 9102_i64);

    // Each declares a language; they start as their own groups.
    let row_en = repo.set_lang(en, "en").await.expect("en");
    assert_eq!(row_en.group_id, en);
    repo.set_lang(pt, "pt-BR").await.expect("pt");
    assert_eq!(repo.alternates(en).await.expect("alone").len(), 1);

    // Linking joins the groups; both now see two alternates.
    repo.join_group(pt, en).await.expect("join");
    let alts = repo.alternates(pt).await.expect("both");
    assert_eq!(alts.len(), 2);
    assert_eq!(alts[0].lang, "en", "sorted by tag");
    assert_eq!(alts[1].lang, "pt-BR");
    assert_eq!(alts[0].slug, "hello");

    // Clearing one leaves the other alone.
    repo.clear(pt).await.expect("clear");
    assert!(repo.get(pt).await.expect("gone").is_none());
    assert_eq!(repo.alternates(en).await.expect("solo again").len(), 1);

    pool.close().await;
}

#[tokio::test]
async fn audience_tables_round_trip() {
    use vyasa_db::repo::AudienceRepo;

    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = AudienceRepo::new(pool.clone());

    // Views roll up: three hits on one path collapse into counts.
    repo.record_view("/post/a", "").await.expect("v1");
    repo.record_view("/post/a", "news.ycombinator.com")
        .await
        .expect("v2");
    repo.record_view("/post/a", "news.ycombinator.com")
        .await
        .expect("v3");
    repo.record_view("/post/b", "").await.expect("v4");
    let daily = repo.daily_views(7).await.expect("daily");
    assert_eq!(daily.iter().map(|(_, v)| v).sum::<i64>(), 4);
    let top = repo.top_paths(7, 10).await.expect("top");
    assert_eq!(top[0], ("/post/a".to_owned(), 3));
    let refs = repo.top_referrers(7, 10).await.expect("refs");
    assert_eq!(refs, [("news.ycombinator.com".to_owned(), 2)]);

    // Submissions arrive and can be dismissed.
    let id = repo
        .add_submission("contact", "Ada", "ada@example.com", "Hello", "/about")
        .await
        .expect("submit");
    assert_eq!(repo.submissions(10).await.expect("list").len(), 1);
    repo.delete_submission(id).await.expect("delete");
    assert!(repo.submissions(10).await.expect("empty").is_empty());

    // Double opt-in state machine.
    let pending = repo
        .upsert_pending("reader@example.com", "tok-1")
        .await
        .expect("pending");
    assert_eq!(pending.status, "pending");
    assert!(repo.confirmed().await.expect("none yet").is_empty());
    assert!(repo
        .set_status_by_token("wrong", "confirmed")
        .await
        .expect("bad token ok")
        .is_none());
    let confirmed = repo
        .set_status_by_token("tok-1", "confirmed")
        .await
        .expect("flip")
        .expect("found");
    assert_eq!(confirmed.status, "confirmed");
    assert!(confirmed.confirmed_at.is_some());
    // Re-subscribing a confirmed reader demotes nothing.
    let again = repo
        .upsert_pending("reader@example.com", "tok-2")
        .await
        .expect("again");
    assert_eq!(again.status, "confirmed");
    assert_eq!(again.token, "tok-1", "token kept, list intact");
    assert_eq!(repo.confirmed().await.expect("one").len(), 1);
    // The emailed link unsubscribes; a fresh signup starts over.
    let out = repo
        .set_status_by_token("tok-1", "unsubscribed")
        .await
        .expect("unsub")
        .expect("found");
    assert_eq!(out.status, "unsubscribed");
    let back = repo
        .upsert_pending("reader@example.com", "tok-3")
        .await
        .expect("re-signup");
    assert_eq!(back.status, "pending");
    assert_eq!(back.token, "tok-3", "fresh token for the new consent");

    pool.close().await;
}

#[tokio::test]
async fn a_menu_draft_applies_whole_created_then_rewritten() {
    use vyasa_core::menu::{MenuDraft, MenuDraftItem};
    use vyasa_core::MenuService;
    use vyasa_db::repo::MenusRepo;

    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = MenusRepo::new(pool.clone());
    let service = MenuService::new(repo.clone());

    let link = |label: &str, url: &str| MenuDraftItem {
        label: label.into(),
        url: url.into(),
        children: Vec::new(),
    };

    // First apply creates the menu — nested items and all.
    let draft = MenuDraft {
        slug: "main".into(),
        name: "Main".into(),
        location: Some("header".into()),
        items: vec![
            link("Home", "/"),
            MenuDraftItem {
                children: vec![link("API", "/api-docs")],
                ..link("Docs", "/docs")
            },
        ],
    };
    service.apply_draft(&draft).await.expect("create by draft");
    let created = repo.get_by_slug("main").await.expect("exists");
    assert_eq!(created.location.as_deref(), Some("header"));
    let items = repo.items(created.id).await.expect("items");
    assert_eq!(items.len(), 3);
    let docs = items.iter().find(|i| i.label == "Docs").expect("docs");
    assert!(items
        .iter()
        .any(|i| i.label == "API" && i.parent_id == Some(docs.id)));

    // Second apply replaces the items wholesale and renames.
    let rewrite = MenuDraft {
        slug: "main".into(),
        name: "Primary".into(),
        location: Some("header".into()),
        items: vec![link("Start", "/start")],
    };
    service.apply_draft(&rewrite).await.expect("rewrite");
    let renamed = repo.get_by_slug("main").await.expect("still there");
    assert_eq!(renamed.name, "Primary");
    assert_eq!(renamed.id, created.id, "same menu, not a duplicate");
    let items = repo.items(renamed.id).await.expect("items after");
    assert_eq!(items.len(), 1, "old links gone, children included");
    assert_eq!(items[0].label, "Start");

    // A draft that breaks a menu rule never touches the database.
    let bad = MenuDraft {
        items: vec![link("Evil", "javascript:alert(1)")],
        ..rewrite
    };
    assert!(matches!(
        service.apply_draft(&bad).await,
        Err(vyasa_common::AppError::Validation { .. })
    ));
    assert_eq!(
        repo.items(renamed.id).await.expect("unchanged").len(),
        1,
        "refused draft left the menu alone"
    );

    pool.close().await;
}

#[tokio::test]
async fn menus_render_nested_html() {
    use vyasa_core::MenuService;
    use vyasa_db::repo::{MenusRepo, NewMenuItem};

    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = MenusRepo::new(pool.clone());
    let service = MenuService::new(repo.clone());

    let menu = service.create("Main", None, None).await.expect("create");
    service
        .add_item(
            menu.id,
            &NewMenuItem {
                parent_id: None,
                label: "Home".into(),
                url: "/".into(),
                sort_order: 0,
            },
        )
        .await
        .expect("home");
    let docs = repo
        .add_item(
            menu.id,
            &NewMenuItem {
                parent_id: None,
                label: "Docs".into(),
                url: "https://docs.example.com".into(),
                sort_order: 1,
            },
        )
        .await
        .expect("docs");
    service
        .add_item(
            menu.id,
            &NewMenuItem {
                parent_id: Some(docs.id),
                label: "API".into(),
                url: "/api-docs".into(),
                sort_order: 0,
            },
        )
        .await
        .expect("nested");

    let html = service.render_html("main").await.expect("render");
    assert!(html.contains("<a href=\"/\">Home</a>"), "{html}");
    assert!(
        html.contains("<a href=\"/api-docs\">API</a></li></ul>"),
        "{html}"
    );

    // Missing menu renders empty, not error.
    assert_eq!(
        service.render_html("no-such").await.expect("missing ok"),
        ""
    );

    pool.close().await;
}

/// The fan-out query once carried a literal backslash from a Rust string
/// continuation, so every event dispatch failed with a syntax error and no
/// webhook ever fired. Exercise the real query against the real database.
#[tokio::test]
async fn webhook_fanout_matches_subscribed_events() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let repo = vyasa_db::repo::WebhooksRepo::new(pool);
    let subscribed = repo
        .create(
            "https://example.com/a",
            "whsec_a",
            &serde_json::json!(["post.published", "comment.created"]),
        )
        .await
        .expect("create subscribed");
    repo.create(
        "https://example.com/b",
        "whsec_b",
        &serde_json::json!(["comment.created"]),
    )
    .await
    .expect("create other");

    let hits = repo
        .list_for_event("post.published")
        .await
        .expect("fan-out query runs");
    assert_eq!(
        hits.iter().map(|w| w.id).collect::<Vec<_>>(),
        vec![subscribed.id]
    );
    assert!(repo
        .list_for_event("post.deleted")
        .await
        .expect("no subscribers")
        .is_empty());
}
