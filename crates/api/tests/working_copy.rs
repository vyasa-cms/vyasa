//! Saving a working copy of a published post must not publish it.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::{models::Role, repo::NewUser, repo::UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

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
        .take(1_000_000)
        .read_to_string(&mut s)
        .unwrap();
    serde_json::from_str(&s).unwrap()
}

fn login(base: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "wc-admin@example.com", "password": "pw-secret-1"})),
    );
    assert_eq!(resp.status(), 200);
    let token = resp
        .all("set-cookie")
        .join("; ")
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .unwrap()
        .to_string();
    format!("vy_session={token}")
}

async fn seed(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: 82_001,
            email: "wc-admin@example.com",
            username: "wcadmin",
            display_name: "WC Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
}

fn doc(text: &str) -> Value {
    json!({"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": text}}]})
}

#[tokio::test]
async fn saving_a_working_copy_leaves_the_published_post_alone() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);

    // A published post with known content.
    let created = json_body(http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &cookie)
            .send_json(
                json!({"title": "Live", "content": doc("live text"), "status": "published"}),
            ),
    ));
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["status"], "published");

    // The editor autosaves before the author presses Save. That autosave
    // must not stand in for the explicit save.
    let auto = http(
        ureq::put(format!("{base}/api/v1/posts/{id}/autosave").as_str())
            .set("cookie", &cookie)
            .send_json(json!({"title": "Live (edited)", "content": doc("edited text")})),
    );
    assert_eq!(auto.status(), 200);

    // Save a working copy: the post must not change.
    let saved = http(
        ureq::post(format!("{base}/api/v1/posts/{id}/revisions").as_str())
            .set("cookie", &cookie)
            .send_json(json!({"title": "Live (edited)", "content": doc("edited text")})),
    );
    assert_eq!(saved.status(), 200);
    let saved = json_body(saved);
    assert_eq!(saved["title"], "Live (edited)");
    assert_eq!(
        saved["is_autosave"], false,
        "the autosave was returned instead of a save"
    );

    let post = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/{id}").as_str())
            .set("cookie", &cookie)
            .call(),
    ));
    assert_eq!(
        post["title"], "Live",
        "saving a working copy published the title"
    );
    assert_eq!(
        post["content"]["blocks"][0]["attrs"]["text"], "live text",
        "saving a working copy published the content"
    );

    // Saving the same working copy again records nothing new.
    let before = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/{id}/revisions").as_str())
            .set("cookie", &cookie)
            .call(),
    ))
    .as_array()
    .unwrap()
    .len();
    let again = http(
        ureq::post(format!("{base}/api/v1/posts/{id}/revisions").as_str())
            .set("cookie", &cookie)
            .send_json(json!({"title": "Live (edited)", "content": doc("edited text")})),
    );
    assert_eq!(again.status(), 200);
    let after = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/{id}/revisions").as_str())
            .set("cookie", &cookie)
            .call(),
    ))
    .as_array()
    .unwrap()
    .len();
    assert_eq!(
        before, after,
        "an unchanged working copy was recorded twice"
    );

    // Invalid content is rejected, not recorded.
    let bad = http(
        ureq::post(format!("{base}/api/v1/posts/{id}/revisions").as_str())
            .set("cookie", &cookie)
            .send_json(json!({"title": "x", "content": {"schema_version": 1, "blocks": [{"kind": "heading", "attrs": {"level": 2}}]}})),
    );
    assert_eq!(bad.status(), 400);

    // Updating is what publishes.
    let updated = json_body(http(
        ureq::put(format!("{base}/api/v1/posts/{id}").as_str())
            .set("cookie", &cookie)
            .send_json(json!({"title": "Live (edited)", "content": doc("edited text")})),
    ));
    assert_eq!(
        updated["content"]["blocks"][0]["attrs"]["text"],
        "edited text"
    );

    // Logged out, nobody can save a working copy.
    let anon = http(
        ureq::post(format!("{base}/api/v1/posts/{id}/revisions").as_str())
            .send_json(json!({"title": "x", "content": doc("y")})),
    );
    assert_eq!(anon.status(), 401);
}
