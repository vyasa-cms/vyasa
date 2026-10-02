//! Demo mode (`VYASA_DEMO__ENABLED=true`) turns an install into a public
//! sandbox: visitors share one administrator account and may edit freely,
//! but nothing that runs code, sends mail, reaches other servers, stores
//! credentials or locks the next visitor out is allowed.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use serde_json::{json, Value};
use vyasa_db::models::Role;
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn call(base: &str, cookie: &str, method: &str, path: &str, body: Option<Value>) -> ureq::Response {
    let request = ureq::request(method, &format!("{base}/api/v1{path}"))
        .set("cookie", cookie)
        .set("origin", base);
    http(match body {
        Some(body) => request.send_json(body),
        None => request.call(),
    })
}

/// Sends `head` as-is (headers only) and returns the whole response.
fn raw_request(base: &str, head: &str) -> String {
    use std::io::{Read, Write};
    let addr = base.trim_start_matches("http://");
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream.write_all(head.as_bytes()).expect("write");
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
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

/// A server in demo mode whose shared account is `email`.
fn demo_server(db: &TestDb, email: &str) -> TestServer {
    TestServer::builder(common::BIN, db)
        .env("VYASA_DEMO__ENABLED", "true")
        .env("VYASA_DEMO__EMAIL", email)
        .env("VYASA_DEMO__PASSWORD", common::PASSWORD)
        .start()
}

#[tokio::test]
async fn a_demo_refuses_what_would_harm_the_site_or_the_next_visitor() {
    let db = TestDb::new().await;
    let demo = common::seed_user(db.pool(), Role::Admin).await;
    let other = common::seed_user(db.pool(), Role::Author).await;
    let server = demo_server(&db, &demo.email);
    let base = server.base();
    let cookie = common::login_cookie(base, &demo.email, &demo.password);

    let refused: Vec<(&str, String, Option<Value>)> = vec![
        // Full-administrator actions.
        ("POST", "/updates/apply".into(), Some(json!({}))),
        (
            "PUT",
            "/ai/providers/openai".into(),
            Some(json!({"api_key": "sk-demo", "base_url": ""})),
        ),
        ("DELETE", "/ai/providers/openai".into(), None),
        ("PUT", "/mail/settings".into(), Some(json!({}))),
        (
            "POST",
            "/mail/test".into(),
            Some(json!({"to": "a@example.com"})),
        ),
        (
            "PUT",
            "/options/site_url".into(),
            Some(json!("https://elsewhere.example")),
        ),
        ("POST", "/import".into(), Some(json!({}))),
        // Code and packages.
        ("POST", "/plugins".into(), Some(json!({}))),
        ("POST", "/plugins/inspect".into(), Some(json!({}))),
        ("POST", "/registry/install".into(), Some(json!({}))),
        ("POST", "/themes".into(), Some(json!({}))),
        // Credentials.
        (
            "POST",
            "/api-keys".into(),
            Some(json!({"name": "mine", "capabilities": ["edit_posts"]})),
        ),
        ("POST", "/auth/mfa/setup".into(), Some(json!({}))),
        (
            "POST",
            "/auth/mfa/confirm".into(),
            Some(json!({"code": "123456"})),
        ),
        // The shared account.
        (
            "PUT",
            "/users/me".into(),
            Some(json!({"password": "a-new-pass-123", "current_password": common::PASSWORD})),
        ),
        (
            "POST",
            format!("/users/{}/sessions/revoke", demo.id),
            Some(json!({})),
        ),
        (
            "POST",
            format!("/users/{}/reset-link", demo.id),
            Some(json!({})),
        ),
        // Reaching other servers.
        (
            "GET",
            "/setup/verify-url?url=http://127.0.0.1:1/".into(),
            None,
        ),
        (
            "POST",
            "/webhooks".into(),
            Some(json!({"url": "https://example.com/hook", "events": ["post.published"]})),
        ),
        (
            "PATCH",
            "/webhooks/1".into(),
            Some(json!({"url": "https://example.com/other"})),
        ),
        ("POST", "/webhooks/1/test".into(), Some(json!({}))),
        (
            "POST",
            "/webhooks/1/deliveries/1/redeliver".into(),
            Some(json!({})),
        ),
        ("POST", "/webhooks/1/rotate-secret".into(), Some(json!({}))),
        ("POST", "/posts/1/check-links".into(), Some(json!({}))),
        // Copying the whole site out in memory.
        ("GET", "/export".into(), None),
        // Erasing the shared account's data (and the account).
        (
            "POST",
            "/privacy/erase".into(),
            Some(json!({"email": demo.email})),
        ),
        // Escalation.
        (
            "POST",
            "/roles".into(),
            Some(
                json!({"slug": "boss", "name": "Boss", "description": "", "capabilities": ["manage_users"]}),
            ),
        ),
        (
            "PUT",
            format!("/users/{}/role", other.id),
            Some(json!({"role": "admin"})),
        ),
        (
            "POST",
            "/users".into(),
            Some(
                json!({"email": "new@example.com", "password": "a-strong-pass-1", "role": "admin"}),
            ),
        ),
    ];
    for (method, path, body) in refused {
        let (status, message) = outcome(call(base, &cookie, method, &path, body));
        assert_eq!(status, 403, "{method} {path}: {message}");
        assert!(message.contains("demo"), "{method} {path}: {message}");
    }

    // Acting on one's own account is refused by the ordinary rules before
    // demo mode is asked; what matters is that the shared account survives.
    for (method, path, body) in [
        ("DELETE", format!("/users/{}", demo.id), None),
        (
            "POST",
            format!("/users/{}/suspend", demo.id),
            Some(json!({"suspended": true})),
        ),
        (
            "PUT",
            format!("/users/{}/role", demo.id),
            Some(json!({"role": "editor"})),
        ),
    ] {
        let (status, message) = outcome(call(base, &cookie, method, &path, body));
        assert!(
            (400..500).contains(&status),
            "{method} {path}: {status} {message}"
        );
    }
    // Another manager (one that predates demo mode) cannot touch the shared
    // account either: that is the demo rule, not the self-account rule.
    let manager = common::seed_user(db.pool(), Role::Admin).await;
    let theirs = common::login_cookie(base, &manager.email, &manager.password);
    for (method, path, body) in [
        ("DELETE", format!("/users/{}", demo.id), None),
        (
            "POST",
            format!("/users/{}/suspend", demo.id),
            Some(json!({"suspended": true})),
        ),
        (
            "PUT",
            format!("/users/{}/role", demo.id),
            Some(json!({"role": "editor"})),
        ),
        (
            "PATCH",
            format!("/users/{}", demo.id),
            Some(json!({"display_name": "Mine now"})),
        ),
    ] {
        let (status, message) = outcome(call(base, &theirs, method, &path, body));
        assert_eq!(status, 403, "{method} {path}: {message}");
        assert!(message.contains("demo"), "{method} {path}: {message}");
    }

    // A custom role cannot be widened into account management later.
    let made = call(
        base,
        &cookie,
        "POST",
        "/roles",
        Some(
            json!({"slug": "helper", "name": "Helper", "description": "", "capabilities": ["edit_posts"]}),
        ),
    );
    assert!(
        made.status() < 300,
        "create ordinary role: {}",
        made.status()
    );
    let (status, message) = outcome(call(
        base,
        &cookie,
        "PATCH",
        "/roles/helper",
        Some(json!({"capabilities": ["edit_posts", "manage_users"]})),
    ));
    assert_eq!(status, 403, "widen role: {message}");
    assert!(message.contains("demo"), "widen role: {message}");

    let me = common::login_cookie(base, &demo.email, &demo.password);
    let caps: Value = call(base, &me, "GET", "/auth/me/caps", None)
        .into_json()
        .unwrap();
    assert!(
        caps.as_array().unwrap().iter().any(|c| c == "manage_users"),
        "still an administrator"
    );

    // Uploads are capped well below the usual limit, judged by the declared
    // length before any of the body is read.
    let response = raw_request(
        base,
        &format!(
            "POST /api/v1/media HTTP/1.1\r\nHost: x\r\nCookie: {cookie}\r\nOrigin: {base}\r\n\
             Content-Type: multipart/form-data; boundary=x\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n",
            3 * 1024 * 1024
        ),
    );
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(response.contains("demo"), "{response}");

    // Ordinary editing still works.
    let created = call(
        base,
        &cookie,
        "POST",
        "/posts",
        Some(
            json!({"type": "post", "title": "Hello demo", "content": {"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": "Hi"}}]}}),
        ),
    );
    assert!(created.status() < 300, "create post: {}", created.status());
    let promoted = call(
        base,
        &cookie,
        "PUT",
        &format!("/users/{}/role", other.id),
        Some(json!({"role": "editor"})),
    );
    assert!(
        promoted.status() < 300,
        "ordinary role change: {}",
        promoted.status()
    );
}

#[tokio::test]
async fn without_demo_mode_the_same_requests_are_not_refused_as_demo() {
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);
    for (method, path, body) in [
        (
            "POST",
            "/api-keys",
            Some(json!({"name": "mine", "capabilities": ["edit_posts"]})),
        ),
        ("POST", "/auth/mfa/setup", Some(json!({}))),
    ] {
        let (status, message) = outcome(call(base, &cookie, method, path, body));
        assert!(
            !message.contains("demo"),
            "{method} {path}: {status} {message}"
        );
        assert!(status < 300, "{method} {path}: {status} {message}");
    }
    // Every route a demo refuses answers for its own reasons here.
    let routes = demo_refused_routes();
    assert!(
        routes.len() > 20,
        "read the refusal table: {}",
        routes.len()
    );
    for (method, path) in routes {
        let path = path
            .replace("{provider}", "openai")
            .replace("{delivery_id}", "1")
            .replace("{id}", "1");
        let relative = path.strip_prefix("/api/v1").expect("an /api/v1 path");
        let (status, message) = outcome(call(base, &cookie, &method, relative, Some(json!({}))));
        assert!(
            !message.contains("disabled in the demo"),
            "{method} {path}: {status} {message}"
        );
    }
}

/// The demo refusal table, read from the source so this test cannot drift
/// from it (the API crate is a binary and cannot be imported).
fn demo_refused_routes() -> Vec<(String, String)> {
    let source = include_str!("../src/policy.rs");
    let start = source
        .find("pub const DEMO_REFUSED_ROUTES")
        .expect("the table");
    let end = start + source[start..].find("];").expect("the end of the table");
    // Formatting may split a row across lines: compare without whitespace.
    let flat: String = source[start..end].split_whitespace().collect();
    flat.split("(\"")
        .skip(1)
        .filter_map(|row| {
            let mut parts = row.split("\",\"");
            Some((parts.next()?.to_owned(), parts.next()?.to_owned()))
        })
        .collect()
}

#[tokio::test]
async fn a_demo_tells_search_engines_to_stay_away_and_shows_its_account() {
    let db = TestDb::new().await;
    let demo = common::seed_user(db.pool(), Role::Admin).await;
    let server = demo_server(&db, &demo.email);
    let base = server.base();

    let home = http(ureq::get(&format!("{base}/")).call());
    assert_eq!(home.header("x-robots-tag"), Some("noindex, nofollow"));
    let body = home.into_string().unwrap();
    assert!(
        body.contains("data-vyasa-demo-banner"),
        "public banner missing"
    );

    let robots = http(ureq::get(&format!("{base}/robots.txt")).call())
        .into_string()
        .unwrap();
    assert!(robots.contains("Disallow: /"), "robots.txt: {robots}");

    let status: Value = http(ureq::get(&format!("{base}/api/v1/setup/status")).call())
        .into_json()
        .unwrap();
    assert_eq!(status["demo"]["email"], demo.email.as_str());
    assert_eq!(status["demo"]["password"], common::PASSWORD);
}

#[tokio::test]
async fn outside_demo_mode_nothing_about_the_demo_shows() {
    let db = TestDb::new().await;
    common::seed_user(db.pool(), Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let home = http(ureq::get(&format!("{base}/")).call());
    assert_eq!(home.header("x-robots-tag"), None);
    assert!(!home
        .into_string()
        .unwrap()
        .contains("data-vyasa-demo-banner"));
    let status: Value = http(ureq::get(&format!("{base}/api/v1/setup/status")).call())
        .into_json()
        .unwrap();
    assert!(status["demo"].is_null(), "{status}");
}
