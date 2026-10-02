//! Synced patterns over HTTP: a post that references one renders its
//! blocks, and editing the pattern changes the page.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use vyasa_testkit::{TestDb, TestServer};

const TOKEN: &str = "stp_test_token_for_patterns_01";

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn body(r: ureq::Response) -> String {
    use std::io::Read;
    let mut s = String::new();
    r.into_reader()
        .take(5_000_000)
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn cookie_of(r: &ureq::Response, name: &str) -> Option<String> {
    r.all("set-cookie")
        .iter()
        .filter_map(|c| c.split(';').next())
        .find(|kv| kv.starts_with(name))
        .map(std::string::ToString::to_string)
}

#[tokio::test]
async fn a_synced_pattern_renders_in_a_post_and_follows_edits() {
    let db = TestDb::new().await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SETUP_TOKEN", TOKEN)
        .start();
    let base = server.base();
    let claimed = http(
        ureq::post(format!("{base}/api/v1/setup/claim").as_str())
            .send_json(json!({"token": TOKEN})),
    );
    let setup_cookie = cookie_of(&claimed, "vy_setup").unwrap();
    let created = http(
        ureq::post(format!("{base}/api/v1/setup/account").as_str())
            .set("cookie", &setup_cookie)
            .send_json(json!({"email": "x@example.com", "password": "twelve-char-password"})),
    );
    let session = cookie_of(&created, "vy_session").unwrap();
    http(
        ureq::post(format!("{base}/api/v1/setup/content").as_str())
            .set("cookie", &session)
            .send_json(json!({"theme": "blog"})),
    );

    // A synced pattern; a nested synced pattern is refused.
    let pattern: Value = serde_json::from_str(&body(http(
        ureq::post(format!("{base}/api/v1/patterns").as_str())
            .set("cookie", &session)
            .send_json(json!({"name": "Call to action", "synced": true, "blocks": [{"kind": "paragraph", "attrs": {"text": "Subscribe today"}, "children": []}]})),
    )))
    .unwrap();
    let pid = pattern["id"].as_i64().unwrap();
    assert_eq!(pattern["slug"], "call-to-action");
    let nested = http(
        ureq::post(format!("{base}/api/v1/patterns").as_str())
            .set("cookie", &session)
            .send_json(json!({"name": "Outer", "synced": true, "blocks": [{"kind": "pattern", "attrs": {"pattern_id": pid.to_string()}, "children": []}]})),
    );
    assert_eq!(nested.status(), 400);

    // A post referencing it renders the pattern's text.
    let post = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &session)
            .send_json(json!({
                "post_type": "post", "status": "published", "title": "Hello", "slug": "hello",
                "content": {"schema_version": 1, "blocks": [
                    {"kind": "paragraph", "attrs": {"text": "Intro"}, "children": []},
                    {"kind": "pattern", "attrs": {"pattern_id": pid.to_string(), "name": "Call to action"}, "children": []}
                ]}
            })),
    );
    assert_eq!(post.status(), 201, "{}", body(post));
    let page = body(http(
        ureq::get(format!("{base}/post/hello").as_str()).call(),
    ));
    assert!(
        page.contains("Subscribe today"),
        "pattern resolved into the page"
    );
    assert!(page.contains("vy-pattern"));

    // Editing the pattern changes the page without touching the post.
    let updated = http(
        ureq::put(format!("{base}/api/v1/patterns/{pid}").as_str())
            .set("cookie", &session)
            .send_json(json!({"name": "Call to action", "synced": true, "blocks": [{"kind": "paragraph", "attrs": {"text": "Join the list"}, "children": []}]})),
    );
    assert_eq!(updated.status(), 200);
    let page = body(http(
        ureq::get(format!("{base}/post/hello").as_str()).call(),
    ));
    assert!(
        page.contains("Join the list") && !page.contains("Subscribe today"),
        "{page}"
    );
}
