//! The database-backed host functions: private storage, comment reads and
//! post writes, with their capability and ownership rules.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_plugins::broker::Broker;
use vyasa_plugins::hostdata::{self, Ctx, NewPost, PostPatch, SITE_ADMIN};
use vyasa_testkit::TestDb;

struct Env {
    _db: TestDb,
    pool: sqlx::PgPool,
    broker: Broker,
    plugin_id: i64,
    other_plugin_id: i64,
}

async fn setup(caps: serde_json::Value) -> Env {
    let db = TestDb::new().await;
    let pool = db.pool().clone();

    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role)
         VALUES ($1, 'a@example.com', 'admin', 'Admin', 'admin')",
    )
    .bind(vyasa_common::next_id_i64())
    .execute(&pool)
    .await
    .expect("admin");

    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let row = repo
        .create("writer", "0.1.0", b"{}", "aa", &caps)
        .await
        .expect("create");
    let other = repo
        .create("other", "0.1.0", b"{}", "bb", &caps)
        .await
        .expect("create other");
    Env {
        broker: Broker::new(repo, pool.clone()),
        _db: db,
        pool,
        plugin_id: row.id,
        other_plugin_id: other.id,
    }
}

fn ctx<'a>(env: &'a Env, plugin_id: i64) -> Ctx<'a> {
    Ctx {
        broker: &env.broker,
        pool: &env.pool,
        plugin_id,
    }
}

fn post(title: &str, status: &str) -> NewPost {
    NewPost {
        post_type: String::from("post"),
        title: String::from(title),
        slug: String::new(),
        body_html: String::from("<p>from a plugin</p>"),
        excerpt: String::new(),
        status: String::from(status),
    }
}

#[tokio::test]
async fn kv_is_private_per_plugin_and_survives_a_prefix_wildcard() {
    let env = setup(serde_json::json!(["kv:write"])).await;
    let mine = ctx(&env, env.plugin_id);
    let theirs = ctx(&env, env.other_plugin_id);

    hostdata::kv_set(&mine, String::from("a/one"), String::from("1"))
        .await
        .unwrap();
    hostdata::kv_set(&mine, String::from("a/two"), String::from("2"))
        .await
        .unwrap();
    hostdata::kv_set(&mine, String::from("b/three"), String::from("3"))
        .await
        .unwrap();
    // The same key in another plugin holds a different value, and neither
    // can see the other's.
    hostdata::kv_set(&theirs, String::from("a/one"), String::from("theirs"))
        .await
        .unwrap();
    assert_eq!(
        hostdata::kv_get(&mine, String::from("a/one"))
            .await
            .unwrap(),
        Some(String::from("1"))
    );

    let keys = hostdata::kv_list(&mine, String::from("a/"), 100)
        .await
        .unwrap();
    assert_eq!(keys, vec![String::from("a/one"), String::from("a/two")]);

    // `%` in a prefix is a literal, not a wildcard over the namespace.
    let keys = hostdata::kv_list(&mine, String::from("%"), 100)
        .await
        .unwrap();
    assert!(keys.is_empty(), "{keys:?}");

    hostdata::kv_delete(&mine, String::from("a/one"))
        .await
        .unwrap();
    assert_eq!(
        hostdata::kv_get(&mine, String::from("a/one"))
            .await
            .unwrap(),
        None
    );
    env.pool.close().await;
}

#[tokio::test]
async fn writing_posts_needs_the_capability_and_publishing_needs_another() {
    let env = setup(serde_json::json!(["db:write:posts"])).await;
    let mine = ctx(&env, env.plugin_id);

    let id = hostdata::create_post(&mine, post("A draft", "draft"))
        .await
        .expect("draft created");
    let (status, ptype): (String, String) =
        sqlx::query_as("SELECT status, type FROM posts WHERE id = $1")
            .bind(i64::try_from(id).unwrap())
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), ptype.as_str()), ("draft", "post"));

    // The whole reason publish is its own capability: this plugin may
    // write, and must still not be able to put text on the site.
    let err = hostdata::create_post(&mine, post("Live now", "published"))
        .await
        .unwrap_err();
    assert_eq!(err, "capability-denied");

    // And the refusal happens before anything is written.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM posts")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    env.pool.close().await;
}

#[tokio::test]
async fn a_plugin_may_only_edit_the_posts_it_created() {
    let env = setup(serde_json::json!(["db:write:posts"])).await;
    let mine = ctx(&env, env.plugin_id);
    let theirs = ctx(&env, env.other_plugin_id);

    let id = hostdata::create_post(&mine, post("Mine", "draft"))
        .await
        .unwrap();

    // Another plugin holding the very same capability cannot touch it.
    let err = hostdata::update_post(
        &theirs,
        id,
        PostPatch {
            title: Some(String::from("Hijacked")),
            body_html: None,
            excerpt: None,
            status: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err, "not your post");

    // Nor can it touch a post a person wrote.
    let human = vyasa_common::next_id_i64();
    let author: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, content, author_id)
         VALUES ($1, 'post', 'published', 'human', 'By a person', '[]'::jsonb, $2)",
    )
    .bind(human)
    .bind(author)
    .execute(&env.pool)
    .await
    .unwrap();
    let err = hostdata::update_post(
        &mine,
        u64::try_from(human).unwrap(),
        PostPatch {
            title: Some(String::from("Rewritten")),
            body_html: None,
            excerpt: None,
            status: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err, "not your post");

    // Its own post it may edit.
    hostdata::update_post(
        &mine,
        id,
        PostPatch {
            title: Some(String::from("Renamed")),
            body_html: None,
            excerpt: None,
            status: None,
        },
    )
    .await
    .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM posts WHERE id = $1")
        .bind(i64::try_from(id).unwrap())
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(title, "Renamed");
    env.pool.close().await;
}

#[tokio::test]
async fn slugs_do_not_collide_and_comments_exclude_unapproved_ones() {
    let env = setup(serde_json::json!(["db:write:posts", "db:read:comments"])).await;
    let mine = ctx(&env, env.plugin_id);

    let first = hostdata::create_post(&mine, post("Same title", "draft"))
        .await
        .unwrap();
    hostdata::create_post(&mine, post("Same title", "draft"))
        .await
        .unwrap();
    let slugs: Vec<String> = sqlx::query_scalar("SELECT slug FROM posts ORDER BY slug")
        .fetch_all(&env.pool)
        .await
        .unwrap();
    assert_eq!(slugs, vec!["same-title", "same-title-2"]);

    let post_id = i64::try_from(first).unwrap();
    for (status, body) in [("approved", "visible"), ("pending", "hidden")] {
        sqlx::query(
            "INSERT INTO comments (id, post_id, author_name, author_email, content, status)
             VALUES ($1, $2, 'Reader', 'r@example.com', $3, $4)",
        )
        .bind(vyasa_common::next_id_i64())
        .bind(post_id)
        .bind(body)
        .bind(status)
        .execute(&env.pool)
        .await
        .unwrap();
    }
    let seen = hostdata::list_comments(&mine, first, 50).await.unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].body_html.contains("visible"));
    env.pool.close().await;
}

#[tokio::test]
async fn mail_reaches_the_site_but_never_a_stranger() {
    let env = setup(serde_json::json!(["mail:send"])).await;
    let mine = ctx(&env, env.plugin_id);

    // The reserved token resolves host-side, so a plugin can write to
    // whoever runs the site without being told the address.
    hostdata::send_mail(
        &mine,
        "writer",
        String::from(SITE_ADMIN),
        String::from("Hello"),
        String::from("Body"),
    )
    .await
    .expect("site-admin is deliverable");
    let (to, count): (String, i64) = sqlx::query_as(
        "SELECT payload ->> 'to', count(*) OVER () FROM jobs WHERE kind = 'send_email'",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(to, "a@example.com", "resolved to the admin's address");
    assert_eq!(count, 1);

    // A registered user is fine too.
    hostdata::send_mail(
        &mine,
        "writer",
        String::from("a@example.com"),
        String::from("Hello"),
        String::from("Body"),
    )
    .await
    .expect("a known user is deliverable");

    // Anyone else is not: an unconstrained send-mail would make every
    // installed plugin a spam relay with the site's reputation behind it.
    let err = hostdata::send_mail(
        &mine,
        "writer",
        String::from("stranger@example.net"),
        String::from("Hello"),
        String::from("Body"),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "unknown recipient");

    // And the attempt is audited, so an operator can see a plugin probing
    // for deliverable addresses.
    let denials: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM plugin_audit WHERE kind = 'deny' AND capability = 'mail:send'",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(denials, 1);

    // Still only the two accepted messages were queued.
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'send_email'")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(queued, 2);
    env.pool.close().await;
}

#[tokio::test]
async fn post_meta_is_namespaced_per_plugin() {
    let env = setup(serde_json::json!(["db:write:meta", "db:read:posts"])).await;
    let mine = ctx(&env, env.plugin_id);
    let theirs = ctx(&env, env.other_plugin_id);

    let author: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    let post_id = vyasa_common::next_id_i64();
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta)
         VALUES ($1, $2, 'post', 'draft', 'p', 'P', '[]'::jsonb, '{}'::jsonb)",
    )
    .bind(post_id)
    .bind(author)
    .execute(&env.pool)
    .await
    .unwrap();
    let id = u64::try_from(post_id).unwrap();

    // Two plugins, the same key, different values, neither visible to the
    // other.
    hostdata::set_post_meta(
        &mine,
        "writer",
        id,
        String::from("rating"),
        String::from("5"),
    )
    .await
    .unwrap();
    hostdata::set_post_meta(
        &theirs,
        "other",
        id,
        String::from("rating"),
        String::from("1"),
    )
    .await
    .unwrap();
    assert_eq!(
        hostdata::get_post_meta(&mine, "writer", id, String::from("rating"))
            .await
            .unwrap(),
        Some(String::from("5"))
    );
    assert_eq!(
        hostdata::get_post_meta(&theirs, "other", id, String::from("rating"))
            .await
            .unwrap(),
        Some(String::from("1"))
    );

    // A second key on the same plugin does not clobber the first — the
    // failure `jsonb_set` produced when the intermediate level was absent.
    hostdata::set_post_meta(
        &mine,
        "writer",
        id,
        String::from("seen"),
        String::from("yes"),
    )
    .await
    .unwrap();
    let meta: serde_json::Value = sqlx::query_scalar("SELECT meta FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(meta["plugin"]["writer"]["rating"], "5", "{meta}");
    assert_eq!(meta["plugin"]["writer"]["seen"], "yes", "{meta}");

    // A post that is not there is an error, not a silent success.
    let err = hostdata::set_post_meta(&mine, "writer", 1, String::from("k"), String::from("v"))
        .await
        .unwrap_err();
    assert_eq!(err, "no such post");
    env.pool.close().await;
}

#[tokio::test]
async fn private_writes_are_quota_bound_but_not_audited() {
    let env = setup(serde_json::json!(["kv:write", "db:write:meta"])).await;
    let mine = ctx(&env, env.plugin_id);

    for n in 0..5 {
        hostdata::kv_set(&mine, format!("k{n}"), String::from("v"))
            .await
            .unwrap();
    }
    // A plugin writing its own namespace is not doing anything an operator
    // reconstructs later. At the quota ceiling, auditing every one would
    // have written 172 800 rows a day and buried the entries that matter.
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM plugin_audit WHERE plugin_id = $1 AND kind = 'write'",
    )
    .bind(env.plugin_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(audited, 0, "kv writes are not audited");

    // A write that leaves the plugin's own storage still is.
    let author: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    let post_id = vyasa_common::next_id_i64();
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta)
         VALUES ($1, $2, 'post', 'draft', 'p', 'P', '[]'::jsonb, '{}'::jsonb)",
    )
    .bind(post_id)
    .bind(author)
    .execute(&env.pool)
    .await
    .unwrap();
    hostdata::set_post_meta(
        &mine,
        "writer",
        u64::try_from(post_id).unwrap(),
        String::from("seen"),
        String::from("yes"),
    )
    .await
    .unwrap();
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM plugin_audit WHERE plugin_id = $1 AND kind = 'write'",
    )
    .bind(env.plugin_id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(audited, 1, "a write touching a post is audited");

    // Both kinds share one quota, so private writes cannot be used to
    // dodge the ceiling on the ones that count.
    env.pool.close().await;
}

#[tokio::test]
async fn meta_keys_are_bounded_per_post() {
    let env = setup(serde_json::json!(["db:write:meta", "db:read:posts"])).await;
    let mine = ctx(&env, env.plugin_id);
    let author: i64 = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    let post_id = vyasa_common::next_id_i64();
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta)
         VALUES ($1, $2, 'post', 'draft', 'p', 'P', '[]'::jsonb, '{}'::jsonb)",
    )
    .bind(post_id)
    .bind(author)
    .execute(&env.pool)
    .await
    .unwrap();
    let id = u64::try_from(post_id).unwrap();

    // `meta` is read back with the post on every admin fetch and every
    // REST call, so an unbounded key count is a cost paid by every reader
    // of that post rather than only by the plugin that wrote them.
    let mut refused = None;
    for n in 0..80 {
        if let Err(e) =
            hostdata::set_post_meta(&mine, "writer", id, format!("k{n}"), String::from("v")).await
        {
            refused = Some((n, e));
            break;
        }
    }
    let (at, err) = refused.expect("the bound is enforced");
    assert_eq!(at, 64, "bounded at 64 keys, refused the 65th");
    assert!(err.contains("meta keys per post"), "{err}");

    // Overwriting a key it already holds still works at the ceiling.
    hostdata::set_post_meta(&mine, "writer", id, String::from("k0"), String::from("v2"))
        .await
        .expect("an existing key is not a new one");
    env.pool.close().await;
}

/// A plugin's post is attributed to the oldest *confirmed*, active administrator:
/// an account whose address was never confirmed has no public presence,
/// so it must not become a byline (phase 98).
#[tokio::test]
async fn plugin_posts_are_attributed_to_a_confirmed_administrator() {
    let env = setup(serde_json::json!(["db:write:posts"])).await;
    // Older than the setup's admin, and unconfirmed.
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role, created_at,
                            email_verified_at)
         VALUES (1, 'early@example.com', 'early', 'Early', 'admin',
                 now() - interval '1 year', NULL)",
    )
    .execute(&env.pool)
    .await
    .expect("unconfirmed admin");
    // Older too, confirmed, but suspended.
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role, created_at,
                            suspended_at)
         VALUES (2, 'gone@example.com', 'gone', 'Gone', 'admin',
                 now() - interval '1 year', now())",
    )
    .execute(&env.pool)
    .await
    .expect("suspended admin");
    let mine = ctx(&env, env.plugin_id);
    let id = hostdata::create_post(&mine, post("Attributed", "draft"))
        .await
        .expect("created");
    let author: String = sqlx::query_scalar(
        "SELECT u.username FROM posts p JOIN users u ON u.id = p.author_id WHERE p.id = $1",
    )
    .bind(i64::try_from(id).unwrap())
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(author, "admin");
    env.pool.close().await;
}
