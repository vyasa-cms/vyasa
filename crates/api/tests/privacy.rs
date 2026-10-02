//! Personal data by email: exported, then erased.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use vyasa_testkit::{TestDb, TestServer};

const TOKEN: &str = "stp_test_token_for_privacy_001";

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn json_body(r: ureq::Response) -> Value {
    use std::io::Read;
    let mut s = String::new();
    r.into_reader()
        .take(5_000_000)
        .read_to_string(&mut s)
        .unwrap();
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

fn cookie_of(r: &ureq::Response, name: &str) -> Option<String> {
    r.all("set-cookie")
        .iter()
        .filter_map(|c| c.split(';').next())
        .find(|kv| kv.starts_with(name))
        .map(std::string::ToString::to_string)
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn personal_data_is_exported_and_then_erased() {
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
            .send_json(json!({"email": "admin@example.com", "password": "twelve-char-password"})),
    );
    let session = cookie_of(&created, "vy_session").unwrap();
    http(
        ureq::post(format!("{base}/api/v1/setup/content").as_str())
            .set("cookie", &session)
            .send_json(json!({"theme": "blog", "sample_content": true})),
    );

    // A second account, a comment and a message under one address.
    http(
        ureq::post(format!("{base}/api/v1/users").as_str())
            .set("cookie", &session)
            .send_json(json!({"email": "ada@example.com", "password": "another-long-password", "role": "subscriber"})),
    );
    let post_id = json_body(http(
        ureq::get(format!("{base}/api/v1/posts?per_page=5").as_str())
            .set("cookie", &session)
            .call(),
    ))["items"][0]["id"]
        .as_i64()
        .unwrap();
    sqlx::query("INSERT INTO comments (id, post_id, author_name, author_email, content, status) VALUES ($1, $2, 'Ada', 'ada@example.com', 'Nice post', 'approved')")
        .bind(vyasa_common::next_id_i64())
        .bind(post_id)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO form_submissions (id, form, name, email, message) VALUES ($1, 'contact', 'Ada', 'ada@example.com', 'Hello')")
        .bind(vyasa_common::next_id_i64())
        .execute(db.pool())
        .await
        .unwrap();

    let data = json_body(http(
        ureq::get(format!("{base}/api/v1/privacy/export?email=Ada%40example.com").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert_eq!(data["email"], "ada@example.com");
    assert!(data["account"].is_object(), "{data}");
    assert!(data["account"].get("password_hash").is_none());
    assert_eq!(data["comments"].as_array().unwrap().len(), 1);
    assert_eq!(data["submissions"].as_array().unwrap().len(), 1);

    let done = json_body(http(
        ureq::post(format!("{base}/api/v1/privacy/erase").as_str())
            .set("cookie", &session)
            .send_json(json!({"email": "ada@example.com"})),
    ));
    assert_eq!(done["comments_anonymised"], 1);
    assert_eq!(done["submissions_deleted"], 1);
    assert_eq!(done["account_deleted"], true, "{done}");

    let after = json_body(http(
        ureq::get(format!("{base}/api/v1/privacy/export?email=ada%40example.com").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert!(after["account"].is_null());
    assert_eq!(
        after["comments"].as_array().unwrap().len(),
        0,
        "anonymised, no longer findable"
    );
    assert_eq!(after["submissions"].as_array().unwrap().len(), 0);
    let name: String = sqlx::query_scalar("SELECT author_name FROM comments WHERE post_id = $1")
        .bind(post_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(name, "Anonymous", "the comment stays, without the person");

    // The audit log knows.
    let log = json_body(http(
        ureq::get(format!("{base}/api/v1/audit-log?limit=20").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert!(
        log.as_array()
            .unwrap()
            .iter()
            .any(|r| r["action"] == "privacy.erase"),
        "{log}"
    );
}
