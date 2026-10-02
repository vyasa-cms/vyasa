//! Export and import over HTTP: a JSON archive round-trips, a second
//! import adds nothing, and the WXR rendering carries the post's HTML.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use vyasa_testkit::{TestDb, TestServer};

const TOKEN: &str = "stp_test_token_for_export_0001";

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
        .take(50_000_000)
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
async fn an_archive_round_trips_and_a_second_import_adds_nothing() {
    let db = TestDb::new().await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SETUP_TOKEN", TOKEN)
        .start();
    let base = server.base();

    // A site with an admin and the wizard's sample content.
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
            .send_json(json!({"site_title": "Exported", "site_url": base})),
    );
    http(
        ureq::post(format!("{base}/api/v1/setup/content").as_str())
            .set("cookie", &session)
            .send_json(json!({"sample_content": true})),
    );

    // JSON export carries the sample post and the About page.
    let exported = body(http(
        ureq::get(format!("{base}/api/v1/export?format=json").as_str())
            .set("cookie", &session)
            .call(),
    ));
    let archive: Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(archive["version"], 1);
    let slugs: Vec<&str> = archive["posts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["slug"].as_str().unwrap())
        .collect();
    assert!(
        slugs.contains(&"welcome") && slugs.contains(&"about"),
        "{slugs:?}"
    );
    assert_eq!(archive["site"]["site_title"], "Exported");
    assert!(
        archive["site"].get("smtp_password").is_none(),
        "secrets never leave"
    );

    // WXR carries the rendered HTML and the WordPress status vocabulary.
    let wxr = body(http(
        ureq::get(format!("{base}/api/v1/export?format=wxr").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert!(wxr.contains("<wp:wxr_version>1.2</wp:wxr_version>"));
    assert!(wxr.contains("<wp:post_name><![CDATA[welcome]]></wp:post_name>"));
    assert!(wxr.contains("<wp:status>publish</wp:status>"));
    assert!(
        wxr.contains("<p class=\"vy-p\">") && wxr.contains("first post"),
        "blocks are rendered to HTML"
    );

    // Wipe the content, import, and the posts are back.
    sqlx::query("DELETE FROM posts")
        .execute(db.pool())
        .await
        .unwrap();
    let report: Value = serde_json::from_str(&body(http(
        ureq::post(format!("{base}/api/v1/import").as_str())
            .set("cookie", &session)
            .set("content-type", "application/json")
            .send_string(&exported),
    )))
    .unwrap();
    assert_eq!(report["posts"], 2, "{report}");
    let again: Value = serde_json::from_str(&body(http(
        ureq::post(format!("{base}/api/v1/import").as_str())
            .set("cookie", &session)
            .set("content-type", "application/json")
            .send_string(&exported),
    )))
    .unwrap();
    assert_eq!(again["posts"], 0, "a second import adds nothing: {again}");
    assert!(again["skipped"].as_u64().unwrap() >= 2);
    let posts = body(http(
        ureq::get(format!("{base}/api/v1/posts?per_page=10").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert!(posts.contains("welcome") && posts.contains("about"));
}
