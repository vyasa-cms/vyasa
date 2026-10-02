//! Two entries that translate each other: one group, hreflang both ways,
//! the right html lang.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use vyasa_testkit::{TestDb, TestServer};

const TOKEN: &str = "stp_test_token_for_translat_1";

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
async fn translations_share_a_group_and_link_each_other() {
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
        ureq::post(format!("{base}/api/v1/setup/site").as_str())
            .set("cookie", &session)
            .send_json(json!({"site_title": "T", "site_url": base, "site_language": "en"})),
    );
    http(
        ureq::post(format!("{base}/api/v1/setup/content").as_str())
            .set("cookie", &session)
            .send_json(json!({"theme": "blog"})),
    );

    let en: Value = serde_json::from_str(&body(http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &session)
            .send_json(json!({"status": "published", "title": "Hello", "slug": "hello", "lang": "en",
                "content": {"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": "Hi"}, "children": []}]}})),
    )))
    .unwrap();
    let en_id = en["id"].as_i64().unwrap();
    let hi: Value = serde_json::from_str(&body(http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &session)
            .send_json(json!({"status": "published", "title": "नमस्ते", "slug": "namaste", "lang": "hi", "translation_of": en_id,
                "content": {"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": "नमस्ते"}, "children": []}]}})),
    )))
    .unwrap();
    assert_eq!(hi["lang"], "hi");
    assert_eq!(
        hi["translation_group"], en_id,
        "the first entry's id seeds the group: {hi}"
    );
    assert_eq!(hi["translations"][0]["slug"], "hello");

    let en_again: Value = serde_json::from_str(&body(http(
        ureq::get(format!("{base}/api/v1/posts/{en_id}").as_str())
            .set("cookie", &session)
            .call(),
    )))
    .unwrap();
    assert_eq!(en_again["translations"][0]["lang"], "hi");

    // A published translation behind a password is not advertised
    // (created before the pages are first rendered).
    let fr = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &session)
            .send_json(json!({"status": "published", "title": "Bonjour", "slug": "bonjour", "lang": "fr",
                "translation_of": en_id, "password": "mot-de-passe",
                "content": {"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": "Salut"}, "children": []}]}})),
    );
    assert_eq!(fr.status(), 201);

    let page_hi = body(http(
        ureq::get(format!("{base}/post/namaste").as_str()).call(),
    ));
    assert!(page_hi.contains("<html lang=\"hi\""), "{}", &page_hi[..200]);
    assert!(
        page_hi.contains("hreflang=\"en\"") && page_hi.contains("/post/hello"),
        "{page_hi}"
    );
    assert!(!page_hi.contains("hreflang=\"fr\""), "{page_hi}");
    let page_en = body(http(
        ureq::get(format!("{base}/post/hello").as_str()).call(),
    ));
    assert!(page_en.contains("hreflang=\"hi\"") && page_en.contains("/post/namaste"));
    assert!(
        !page_en.contains("hreflang=\"fr\"") && !page_en.contains("/post/bonjour"),
        "{page_en}"
    );

    // A bad tag is refused.
    let bad = http(
        ureq::put(format!("{base}/api/v1/posts/{en_id}").as_str())
            .set("cookie", &session)
            .send_json(json!({"lang": "not a tag!"})),
    );
    assert_eq!(bad.status(), 400);
}
