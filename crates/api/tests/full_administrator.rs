//! With custom roles, `manage_options` can be held without the rest. On
//! its own it must not be a way to take the site over: whatever decides
//! where mail goes, which address links carry, where updates and packages
//! come from and whose signature is trusted, and loading a whole archive,
//! takes a full administrator (every capability).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{ApiKeysRepo, NewRole, RolesRepo, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

const ALL_CAPS: [&str; 12] = [
    "edit_posts",
    "publish_posts",
    "edit_others",
    "delete_posts",
    "manage_categories",
    "moderate_comments",
    "upload_media",
    "manage_users",
    "manage_themes",
    "manage_plugins",
    "manage_options",
    "view_admin",
];

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

/// How a request is authenticated.
#[derive(Clone, Copy)]
enum As<'a> {
    Cookie(&'a str),
    Key(&'a str),
}

fn call(
    base: &str,
    caller: As<'_>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> ureq::Response {
    let request = ureq::request(method, &format!("{base}/api/v1{path}"));
    let request = match caller {
        As::Cookie(cookie) => request.set("cookie", cookie),
        As::Key(raw) => request.set("authorization", &format!("Bearer {raw}")),
    };
    http(match body {
        Some(body) => request.send_json(body),
        None => request.call(),
    })
}

/// The status and the error message.
fn outcome(r: ureq::Response) -> (u16, String) {
    let status = r.status();
    let message = r
        .into_json::<Value>()
        .ok()
        .and_then(|body| body["message"].as_str().map(str::to_owned))
        .unwrap_or_default();
    (status, message)
}

/// A user holding a custom role with exactly `caps`.
async fn seed_with_caps(pool: &PgPool, slug: &str, caps: &[&str]) -> common::Seeded {
    let capabilities: Vec<String> = caps.iter().map(|c| (*c).to_owned()).collect();
    RolesRepo::new(pool.clone())
        .insert(&NewRole {
            slug,
            name: slug,
            description: "",
            capabilities: &capabilities,
        })
        .await
        .expect("role");
    let user = common::seed_user(pool, Role::Subscriber).await;
    UsersRepo::new(pool.clone())
        .set_custom_role(user.id, Some(slug))
        .await
        .expect("custom role");
    user
}

async fn option(pool: &PgPool, key: &str) -> Option<Value> {
    sqlx::query_scalar("SELECT value FROM options WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .expect("option")
}

/// Hashes a raw key the way the server does (sha256 hex).
fn key_hash(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(raw.as_bytes())
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn empty_archive() -> Value {
    json!({
        "version": 1, "exported_at": "", "site": {}, "terms": [], "posts": [],
        "comments": [], "media": [], "menus": [], "users": [],
    })
}

/// A relay nothing listens on, so a test message fails at once instead of
/// leaving the machine.
const RELAY: &str = r#"{"host": "127.0.0.1", "port": 1, "from": "a@attacker.invalid"}"#;

/// Every option that redirects mail, links, updates or package trust,
/// with a value its validation accepts.
fn sensitive_options() -> Vec<(&'static str, Value)> {
    vec![
        ("site_url", json!("https://attacker.invalid")),
        ("smtp_host", json!("127.0.0.1")),
        ("smtp_port", json!("1")),
        ("smtp_username", json!("attacker")),
        ("smtp_password", json!("hunter22")),
        ("smtp_from", json!("a@attacker.invalid")),
        // Phase 98: who may make their own account, and what they become.
        ("registration_enabled", json!(true)),
        ("registration_default_role", json!("subscriber")),
    ]
}

/// Every operation that takes a full administrator, as (what the refusal
/// names, method, path, body).
fn guarded_operations() -> Vec<(&'static str, &'static str, String, Value)> {
    let relay: Value = serde_json::from_str(RELAY).unwrap();
    let mut out = vec![
        (
            "the mail relay",
            "PUT",
            "/mail/settings".to_owned(),
            relay.clone(),
        ),
        (
            "send a test message",
            "POST",
            "/mail/test".to_owned(),
            json!({ "to": "a@attacker.invalid" }),
        ),
        (
            "\"site_url\"",
            "PUT",
            "/options".to_owned(),
            json!({ "site_title": "Taken", "site_url": "https://attacker.invalid" }),
        ),
        ("upgrade", "POST", "/updates/apply".to_owned(), json!({})),
        ("import", "POST", "/import".to_owned(), empty_archive()),
        (
            "site address",
            "POST",
            "/setup/site".to_owned(),
            json!({ "site_title": "Taken", "site_url": "https://attacker.invalid" }),
        ),
        (
            "the mail relay",
            "POST",
            "/setup/mail".to_owned(),
            json!({ "smtp": relay }),
        ),
        (
            "send a test message",
            "POST",
            "/setup/mail/test".to_owned(),
            json!({ "to": "a@attacker.invalid" }),
        ),
        // A provider's address with the key left out keeps the stored key
        // and sends it to the new host.
        (
            "AI provider",
            "PUT",
            "/ai/providers/anthropic".to_owned(),
            json!({ "base_url": "https://attacker.invalid/v1" }),
        ),
        (
            "AI provider",
            "POST",
            "/setup/assistants".to_owned(),
            json!({ "provider": "anthropic", "api_key": "sk-attacker" }),
        ),
        (
            "AI provider",
            "DELETE",
            "/ai/providers/anthropic".to_owned(),
            json!({}),
        ),
    ];
    for (key, value) in sensitive_options() {
        out.push((key, "PUT", format!("/options/{key}"), value));
    }
    out
}

#[tokio::test]
async fn manage_options_alone_does_not_reach_what_could_take_the_site_over() {
    let db = TestDb::new().await;
    let pool = db.pool();
    common::seed_user(pool, Role::Admin).await;
    let keeper = seed_with_caps(pool, "settings-only", &["view_admin", "manage_options"]).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &keeper.email, &keeper.password);
    let me = As::Cookie(&cookie);

    for (names, method, path, body) in guarded_operations() {
        let (status, message) = outcome(call(base, me, method, &path, Some(body)));
        assert_eq!(status, 403, "{method} {path}: {message}");
        assert!(
            message.contains("only a full administrator can"),
            "{method} {path}: {message}"
        );
        assert!(message.contains(names), "{method} {path}: {message}");
    }
    // Nothing was written: not even the ordinary key that travelled in
    // the same bulk request or the same wizard step as a guarded one.
    for (key, _) in sensitive_options() {
        assert_eq!(option(pool, key).await, None, "{key}");
    }
    assert_eq!(option(pool, "site_title").await, None);

    // Ordinary settings are still theirs: one at a time, in bulk, and
    // through the wizard steps that redirect nothing.
    let one = call(base, me, "PUT", "/options/site_title", Some(json!("Mine")));
    assert_eq!(one.status(), 204);
    let many = call(
        base,
        me,
        "PUT",
        "/options",
        Some(json!({ "site_tagline": "Still mine", "posts_per_page": 7 })),
    );
    assert_eq!(many.status(), 204);
    assert_eq!(option(pool, "site_title").await, Some(json!("Mine")));
    assert_eq!(
        option(pool, "site_tagline").await,
        Some(json!("Still mine"))
    );
    assert_eq!(call(base, me, "GET", "/mail/settings", None).status(), 200);
    assert_eq!(call(base, me, "GET", "/updates", None).status(), 200);
    assert_eq!(call(base, me, "GET", "/export", None).status(), 200);
    let moderation = call(
        base,
        me,
        "POST",
        "/setup/mail",
        Some(json!({ "comment_moderation": "require_all" })),
    );
    assert_eq!(moderation.status(), 204);
    assert_eq!(
        option(pool, "comment_moderation").await,
        Some(json!("require_all"))
    );
}

#[tokio::test]
async fn an_administrator_is_a_full_administrator() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    let me = As::Cookie(&cookie);

    for (_, method, path, body) in guarded_operations() {
        let (status, message) = outcome(call(base, me, method, &path, Some(body)));
        assert_ne!(status, 403, "{method} {path}: {message}");
        assert_ne!(status, 401, "{method} {path}: {message}");
    }
    assert_eq!(
        option(pool, "site_url").await,
        Some(json!("https://attacker.invalid"))
    );
    assert_eq!(option(pool, "smtp_host").await, Some(json!("127.0.0.1")));
    assert_eq!(option(pool, "site_title").await, Some(json!("Taken")));
}

#[tokio::test]
async fn retired_options_are_refused() {
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    for key in [
        "registry_url",
        "registry_trusted_keys",
        "update_channel_url",
        "update_trusted_keys",
    ] {
        let (status, message) = outcome(call(
            base,
            As::Cookie(&cookie),
            "PUT",
            &format!("/options/{key}"),
            Some(json!("https://x.invalid/i")),
        ));
        assert_eq!(status, 400, "{key}: {message}");
        assert!(message.contains("unknown option"), "{key}: {message}");
    }
}

#[tokio::test]
async fn a_key_is_a_full_administrator_only_with_every_grant() {
    const NARROW: &str = "vy_full_admin_narrow_key_0000001";
    const WIDE: &str = "vy_full_admin_wide_key_000000002";
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    // Every capability through a custom role is a full administrator too:
    // the rule is about what the caller holds, not what they are called.
    let everything = seed_with_caps(pool, "everything", &ALL_CAPS).await;
    let keys = ApiKeysRepo::new(pool.clone());
    keys.insert(
        97_301,
        admin.id,
        "settings",
        &key_hash(NARROW),
        &json!(["manage_options", "view_admin"]),
    )
    .await
    .expect("narrow key");
    keys.insert(97_302, admin.id, "all", &key_hash(WIDE), &json!(ALL_CAPS))
        .await
        .expect("wide key");
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();

    // An administrator's key that was granted only the options capability
    // is held to its grants.
    let narrow = As::Key(NARROW);
    for (path, body) in [
        ("/options/site_url", json!("https://attacker.invalid")),
        ("/mail/settings", serde_json::from_str(RELAY).unwrap()),
    ] {
        let (status, message) = outcome(call(base, narrow, "PUT", path, Some(body)));
        assert_eq!(status, 403, "{path}: {message}");
        assert!(
            message.contains("only a full administrator can"),
            "{message}"
        );
    }
    let (status, _) = outcome(call(base, narrow, "POST", "/import", Some(empty_archive())));
    assert_eq!(status, 403);
    assert_eq!(
        call(base, narrow, "PUT", "/options/site_title", Some(json!("K"))).status(),
        204
    );
    assert_eq!(option(pool, "site_url").await, None);

    let wide = As::Key(WIDE);
    let put = call(
        base,
        wide,
        "PUT",
        "/options/site_url",
        Some(json!("https://site.example")),
    );
    assert_eq!(put.status(), 204);
    assert_eq!(
        call(base, wide, "POST", "/import", Some(empty_archive())).status(),
        200
    );

    let cookie = common::login_cookie(base, &everything.email, &everything.password);
    let put = call(
        base,
        As::Cookie(&cookie),
        "PUT",
        "/options/site_url",
        Some(json!("https://everything.example")),
    );
    assert_eq!(put.status(), 204);
    assert_eq!(
        option(pool, "site_url").await,
        Some(json!("https://everything.example"))
    );
}
