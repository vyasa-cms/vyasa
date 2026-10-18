//! Managed entries: idempotent, parent-first, validated, owned, detachable.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_plugins::broker::Broker;
use vyasa_plugins::hostdata::{self, Ctx, EntryInput};
use vyasa_testkit::TestDb;

struct Env {
    _db: TestDb,
    pool: sqlx::PgPool,
    broker: Broker,
    plugin: i64,
    other: i64,
    bare: i64,
}

async fn setup() -> Env {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    sqlx::query("INSERT INTO users (id, email, username, display_name, role, email_verified_at) VALUES ($1, 'a@example.com', 'admin', 'Admin', 'admin', now())")
        .bind(vyasa_common::next_id_i64()).execute(&pool).await.unwrap();
    vyasa_db::content_models::PostType::register_plugin_types(&["managed-doc"]).unwrap();
    vyasa_db::content_models::PostType::set_traits("managed-doc", true, None).unwrap();
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let caps = serde_json::json!(["entries:sync", "db:publish:posts"]);
    let plugin = repo
        .create("syncer", "0.1.0", b"{}", "aa", &caps)
        .await
        .unwrap()
        .id;
    let other = repo
        .create("rival", "0.1.0", b"{}", "bb", &caps)
        .await
        .unwrap()
        .id;
    let bare = repo
        .create(
            "bare",
            "0.1.0",
            b"{}",
            "cc",
            &serde_json::json!(["log:write"]),
        )
        .await
        .unwrap()
        .id;
    Env {
        broker: Broker::new(repo, pool.clone()),
        _db: db,
        pool,
        plugin,
        other,
        bare,
    }
}

fn ctx<'a>(e: &'a Env, id: i64) -> Ctx<'a> {
    Ctx {
        broker: &e.broker,
        pool: &e.pool,
        plugin_id: id,
    }
}

fn entry(key: &str, parent: Option<&str>, title: &str, hash: &str) -> EntryInput {
    EntryInput {
        post_type: "managed-doc".into(),
        key: key.into(),
        parent_key: parent.map(str::to_owned),
        slug: String::new(),
        title: title.into(),
        excerpt: String::new(),
        blocks_json: r#"[{"kind":"paragraph","attrs":{"text":"hello"},"children":[]}]"#.into(),
        position: 10,
        status: "published".into(),
        terms: vec![],
        fields_json: "{}".into(),
        seo_json: "{}".into(),
        source_hash: hash.into(),
        edit_url: "https://github.com/o/r/edit/main/docs/a.md".into(),
    }
}

#[tokio::test]
async fn the_first_put_creates_the_same_hash_changes_nothing_a_new_hash_updates() {
    let e = setup().await;
    let c = ctx(&e, e.plugin);
    let a = hostdata::put_entry(&c, entry("a.md", None, "Alpha", "h1"))
        .await
        .unwrap();
    assert_eq!(a.outcome, "created");
    let again = hostdata::put_entry(&c, entry("a.md", None, "Alpha", "h1"))
        .await
        .unwrap();
    assert_eq!((again.outcome.as_str(), again.id), ("unchanged", a.id));
    let changed = hostdata::put_entry(&c, entry("a.md", None, "Alpha v2", "h2"))
        .await
        .unwrap();
    assert_eq!((changed.outcome.as_str(), changed.id), ("updated", a.id));
    let title: String = sqlx::query_scalar("SELECT title FROM posts WHERE id = $1")
        .bind(i64::try_from(a.id).unwrap())
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(title, "Alpha v2");
}

#[tokio::test]
async fn children_need_their_parent_first_and_get_paths_and_positions() {
    let e = setup().await;
    let c = ctx(&e, e.plugin);
    let err = hostdata::put_entry(&c, entry("g/i.md", Some("g"), "Install", "h"))
        .await
        .unwrap_err();
    assert!(err.contains("parent"), "{err}");
    hostdata::put_entry(&c, entry("g", None, "Guides", "h"))
        .await
        .unwrap();
    let child = hostdata::put_entry(&c, entry("g/i.md", Some("g"), "Install", "h"))
        .await
        .unwrap();
    let path: Option<String> = sqlx::query_scalar("SELECT path FROM posts WHERE id = $1")
        .bind(i64::try_from(child.id).unwrap())
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(path.as_deref(), Some("guides/install"));
}

#[tokio::test]
async fn list_managed_returns_only_this_plugins_entries_with_their_hash() {
    let e = setup().await;
    hostdata::put_entry(&ctx(&e, e.plugin), entry("a.md", None, "A", "ha"))
        .await
        .unwrap();
    hostdata::put_entry(&ctx(&e, e.other), entry("a.md", None, "Rival A", "hr"))
        .await
        .unwrap();
    let mine = hostdata::list_managed(&ctx(&e, e.plugin), "managed-doc".into())
        .await
        .unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(
        (mine[0].key.as_str(), mine[0].source_hash.as_str()),
        ("a.md", "ha")
    );
    // The same key under another plugin is another entry, with another slug.
    let slugs: Vec<String> =
        sqlx::query_scalar("SELECT slug FROM posts WHERE type = 'managed-doc' ORDER BY id")
            .fetch_all(&e.pool)
            .await
            .unwrap();
    assert_eq!(slugs.len(), 2);
}

#[tokio::test]
async fn a_plugin_without_the_capability_is_denied() {
    let e = setup().await;
    let err = hostdata::put_entry(&ctx(&e, e.bare), entry("a.md", None, "A", "h"))
        .await
        .unwrap_err();
    assert_eq!(err, "capability-denied");
    assert_eq!(
        hostdata::list_managed(&ctx(&e, e.bare), "managed-doc".into())
            .await
            .unwrap_err(),
        "capability-denied"
    );
    assert_eq!(
        hostdata::delete_entry(&ctx(&e, e.bare), "a.md".into())
            .await
            .unwrap_err(),
        "capability-denied"
    );
}

#[tokio::test]
async fn invalid_input_is_refused_before_anything_is_written() {
    let e = setup().await;
    let c = ctx(&e, e.plugin);
    let mut bad = entry("a.md", None, "A", "h");
    bad.blocks_json = r#"[{"kind":"heading","attrs":{"level":1,"text":"x"},"children":[]}]"#.into();
    assert!(hostdata::put_entry(&c, bad)
        .await
        .unwrap_err()
        .contains("heading"));
    let mut bad = entry("b.md", None, "B", "h");
    bad.blocks_json = "not json".into();
    assert!(hostdata::put_entry(&c, bad).await.is_err());
    let mut bad = entry("c.md", None, "C", "h");
    bad.post_type = "post".into();
    assert!(
        hostdata::put_entry(&c, bad)
            .await
            .unwrap_err()
            .contains("custom"),
        "only plugin-registered custom types"
    );
    let mut bad = entry("d.md", None, "D", "h");
    bad.status = "scheduled".into();
    assert!(hostdata::put_entry(&c, bad).await.is_err());
    let mut bad = entry("e.md", None, "", "h");
    bad.title = String::new();
    assert!(hostdata::put_entry(&c, bad).await.is_err());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM posts WHERE type = 'managed-doc'")
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn publishing_needs_the_publish_capability() {
    let e = setup().await;
    let repo = vyasa_db::repo::PluginsRepo::new(e.pool.clone());
    let drafty = repo
        .create(
            "drafty",
            "0.1.0",
            b"{}",
            "dd",
            &serde_json::json!(["entries:sync"]),
        )
        .await
        .unwrap()
        .id;
    let err = hostdata::put_entry(&ctx(&e, drafty), entry("a.md", None, "A", "h"))
        .await
        .unwrap_err();
    assert_eq!(err, "capability-denied");
    let mut draft = entry("a.md", None, "A", "h");
    draft.status = "draft".into();
    assert_eq!(
        hostdata::put_entry(&ctx(&e, drafty), draft)
            .await
            .unwrap()
            .outcome,
        "created"
    );
}

#[tokio::test]
async fn delete_trashes_and_a_changed_put_restores_an_unchanged_one_does_not() {
    let e = setup().await;
    let c = ctx(&e, e.plugin);
    let a = hostdata::put_entry(&c, entry("a.md", None, "A", "h1"))
        .await
        .unwrap();
    hostdata::delete_entry(&c, "a.md".into()).await.unwrap();
    let status = |id: u64| {
        let pool = e.pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT status FROM posts WHERE id = $1")
                .bind(i64::try_from(id).unwrap())
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(status(a.id).await, "trash");
    assert_eq!(
        hostdata::put_entry(&c, entry("a.md", None, "A", "h1"))
            .await
            .unwrap()
            .outcome,
        "unchanged"
    );
    assert_eq!(status(a.id).await, "trash", "the same source stays removed");
    assert_eq!(
        hostdata::put_entry(&c, entry("a.md", None, "A", "h2"))
            .await
            .unwrap()
            .outcome,
        "updated"
    );
    assert_eq!(
        status(a.id).await,
        "published",
        "a changed source brings it back"
    );
}

#[tokio::test]
async fn a_detached_key_is_never_written_again() {
    let e = setup().await;
    let c = ctx(&e, e.plugin);
    let a = hostdata::put_entry(&c, entry("a.md", None, "A", "h1"))
        .await
        .unwrap();
    vyasa_db::repo::ManagedRepo::new(e.pool.clone())
        .set_detached(i64::try_from(a.id).unwrap(), true)
        .await
        .unwrap();
    let out = hostdata::put_entry(&c, entry("a.md", None, "Overwritten?", "h2"))
        .await
        .unwrap();
    assert_eq!(out.outcome, "detached");
    let title: String = sqlx::query_scalar("SELECT title FROM posts WHERE id = $1")
        .bind(i64::try_from(a.id).unwrap())
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(title, "A");
    hostdata::delete_entry(&c, "a.md".into()).await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM posts WHERE id = $1")
        .bind(i64::try_from(a.id).unwrap())
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(
        status, "published",
        "deleting a detached key leaves the page alone"
    );
    assert!(hostdata::list_managed(&c, "managed-doc".into())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn terms_fields_and_seo_land_where_they_belong() {
    let e = setup().await;
    let c = ctx(&e, e.plugin);
    sqlx::query(
        "INSERT INTO terms (id, taxonomy, name, slug) VALUES (900, 'category', 'News', 'news')",
    )
    .execute(&e.pool)
    .await
    .unwrap();
    let mut en = entry("a.md", None, "A", "h");
    en.terms = vec!["category:news".into(), "category:created-on-demand".into()];
    en.seo_json = r#"{"canonical-path":"/docs/a","noindex":false,"description":"d"}"#.into();
    let out = hostdata::put_entry(&c, en).await.unwrap();
    let id = i64::try_from(out.id).unwrap();
    let terms: Vec<String> = sqlx::query_scalar("SELECT t.taxonomy || ':' || t.slug FROM term_relationships r JOIN terms t ON t.id = r.term_id WHERE r.post_id = $1 ORDER BY 1").bind(id).fetch_all(&e.pool).await.unwrap();
    assert_eq!(terms, ["category:created-on-demand", "category:news"]);
    let meta: serde_json::Value = sqlx::query_scalar("SELECT meta FROM posts WHERE id = $1")
        .bind(id)
        .fetch_one(&e.pool)
        .await
        .unwrap();
    assert_eq!(meta["seo_canonical_path"], "/docs/a");
    assert_eq!(meta["seo_description"], "d");
    let owner: Option<i64> =
        sqlx::query_scalar("SELECT plugin_id FROM plugin_posts WHERE post_id = $1")
            .bind(id)
            .fetch_optional(&e.pool)
            .await
            .unwrap();
    assert_eq!(
        owner,
        Some(e.plugin),
        "ownership recorded as for create-post"
    );
    let row: (String, String, bool) = sqlx::query_as(
        "SELECT ext_key, edit_url, detached FROM managed_entries WHERE post_id = $1",
    )
    .bind(id)
    .fetch_one(&e.pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "a.md".to_owned(),
            "https://github.com/o/r/edit/main/docs/a.md".to_owned(),
            false
        )
    );
}
